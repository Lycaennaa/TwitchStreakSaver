const NATIVE_HOST = "com.twitchstreaksaver.bridge";
const NATIVE_HOST_SYNC_ALARM = "twitch-streak-saver-native-host-sync";
// MV3 alarms wake suspended workers; active workers retry failed native connections after 5 seconds.
const NATIVE_HOST_SYNC_INTERVAL_MINUTES = 0.5;
const NATIVE_HOST_RETRY_DELAY_MS = 5000;
importScripts("twitch_tabs.js");

let nativePort = null;
let syncedNativePort = null;
let nativeBridgeReadyPort = null;
let nativeHostError = null;
let reconnectTimer = null;
let managedTabs = new Map();
const closingManagedTabIds = new Set();
let channelsByLogin = new Map();
let externalScanTimer = null;
let externalScanGeneration = 0;
let nativeCommandQueue = Promise.resolve();
const MAX_CONCURRENT_CHANNEL_OPENS = 2;
let managedTabsSaveQueue = Promise.resolve();
let nativeSyncPromise = null;

const restorePromise = restoreManagedTabs();
function synchronizeNativeHost() {
  if (nativePort) return Promise.resolve(true);
  if (nativeSyncPromise === null) {
    nativeSyncPromise = synchronizeNativeHostOnce().finally(() => {
      nativeSyncPromise = null;
    });
  }
  return nativeSyncPromise;
}

async function synchronizeNativeHostOnce() {
  if (nativePort) return true;
  connectNativeHost();
  return nativePort !== null;
}

function initializeNativeHostSync() {
  chrome.alarms.create(NATIVE_HOST_SYNC_ALARM, {
    periodInMinutes: NATIVE_HOST_SYNC_INTERVAL_MINUTES,
  });
  return synchronizeNativeHost();
}

function handleNativeHostSyncAlarm(alarm) {
  if (alarm?.name !== NATIVE_HOST_SYNC_ALARM) return Promise.resolve(false);
  resetPendingNativeHost();
  return synchronizeNativeHost();
}

function nativeHostStatus() {
  return {
    nativeConnected: nativePort !== null,
    bridgeReady: nativePort !== null && nativeBridgeReadyPort === nativePort,
    error: nativeHostError,
  };
}

function resetPendingNativeHost() {
  const port = nativePort;
  if (!port || nativeBridgeReadyPort === port) return;
  handleNativePortDisconnect(port);
  try {
    port.disconnect();
  } catch (error) {
    console.debug("Could not close a stalled native host connection.", error);
  }
}

function handleNativeHostWakeRequest(forceReconnect, sendResponse) {
  if (forceReconnect) resetPendingNativeHost();
  Promise.resolve()
    .then(initializeNativeHostSync)
    .then(
      () => sendResponse(nativeHostStatus()),
      (error) => {
        nativeHostError = error?.message || String(error);
        console.debug("Could not wake the Twitch Streak Saver native host.", error);
        sendResponse(nativeHostStatus());
      },
    );
  return true;
}

async function restoreManagedTabs() {
  const stored = await chrome.storage.local.get("managedTabs");
  managedTabs = new Map(stored.managedTabs || []);
  const tabs = await chrome.tabs.query({});
  const tabsById = new Map(tabs.map((tab) => [tab.id, tab]));
  for (const [channelId, record] of managedTabs) {
    if (record.tabId && !record.playerTabId) {
      record.playerTabId = record.tabId;
      delete record.tabId;
    }
    for (const surface of ["player", "chat"]) {
      const key = `${surface}TabId`;
      const tab = tabsById.get(record[key]);
      const routeLogins = [
        record.login,
        ...(Array.isArray(record.previousLogins) ? record.previousLogins : []),
      ];
      const currentPage = tab && routeLogins.some(
        (login) => urlMatchesManagedSurface(tab.url, login, surface, record.layout),
      );
      const pendingPage = tab?.url === "about:blank"
        && urlMatchesManagedSurface(record.pendingUrl, record.login, surface, record.layout);
      if (!currentPage && !pendingPage) record[key] = null;
    }
    if (!record.playerTabId && !record.chatTabId) managedTabs.delete(channelId);
  }
  await saveManagedTabs();
}

function saveManagedTabs() {
  const snapshot = Array.from(managedTabs.entries());
  const saving = managedTabsSaveQueue.then(() =>
    chrome.storage.local.set({ managedTabs: snapshot }),
  );
  managedTabsSaveQueue = saving.catch(() => {});
  return saving;
}

async function reportExternalStreams() {
  if (!nativePort) return;
  const generation = ++externalScanGeneration;
  await restorePromise;
  const tabs = await chrome.tabs.query({});
  if (generation !== externalScanGeneration || !nativePort) return;
  const managedTabIds = new Set(
    Array.from(managedTabs.values()).flatMap((record) =>
      managedSurfaceTabs(record).map(([, tabId]) => tabId),
    ),
  );
  const inventory = TwitchTabUtils.externalStreamInventory(
    tabs,
    managedTabIds,
    channelsByLogin,
  );
  sendNative({
    type: "external_streams",
    ...inventory,
    managed_channel_ids: Array.from(managedTabs.entries())
      .filter(([, record]) => Number.isInteger(record.playerTabId))
      .map(([channelId]) => channelId),
  });
}

function scheduleExternalStreamReport() {
  if (externalScanTimer !== null) return;
  externalScanTimer = setTimeout(() => {
    externalScanTimer = null;
    void reportExternalStreams();
  }, 100);
}

function connectNativeHost() {
  if (nativePort) return;
  let port = null;
  try {
    port = chrome.runtime.connectNative(NATIVE_HOST);
    nativePort = port;
    syncedNativePort = null;
    nativeBridgeReadyPort = null;
    nativeHostError = null;
    if (reconnectTimer !== null) {
      clearTimeout(reconnectTimer);
      reconnectTimer = null;
    }
    port.onMessage.addListener((message) => {
      if (nativePort !== port) return;
      nativeCommandQueue = nativeCommandQueue
        .then(() => handleNativeCommand(message, port))
        .catch((error) => {
          console.warn("Could not handle Twitch Streak Saver command.", error);
        });
    });
    port.onDisconnect.addListener(() => {
      handleNativePortDisconnect(port, chrome.runtime.lastError);
    });
    port.postMessage({ type: "ready" });
  } catch (error) {
    if (port) {
      if (nativePort === port) handleNativePortDisconnect(port, error);
      return;
    }
    nativeHostError = error?.message || String(error);
    console.warn("Twitch Streak Saver native host is unavailable.", error);
    scheduleReconnect();
  }
}

function handleNativePortDisconnect(port, error) {
  if (nativePort !== port) return;
  nativePort = null;
  syncedNativePort = null;
  nativeBridgeReadyPort = null;
  nativeHostError = error?.message || "Native host disconnected.";
  channelsByLogin = new Map();
  for (const record of managedTabs.values()) {
    for (const [, tabId] of managedSurfaceTabs(record)) {
      setMonitoring(tabId, "stop_monitoring");
    }
  }
  if (error) {
    console.debug("Twitch Streak Saver native host disconnected.", error.message);
  }
  scheduleReconnect();
}

function scheduleReconnect() {
  if (reconnectTimer !== null) return;
  reconnectTimer = setTimeout(() => {
    reconnectTimer = null;
    return synchronizeNativeHost();
  }, NATIVE_HOST_RETRY_DELAY_MS);
}

function sendNative(message) {
  const port = nativePort;
  if (!port) return;
  try {
    port.postMessage(message);
  } catch (error) {
    handleNativePortDisconnect(port, error);
  }
}

function setMonitoring(tabId, type) {
  void chrome.tabs.sendMessage(tabId, { type }).catch(() => {});
}

function managedSurfaceTabs(record) {
  return [
    ["player", record.playerTabId],
    ["chat", record.chatTabId],
  ].filter(([, tabId]) => Number.isInteger(tabId));
}

function detachManagedTab(channelId, record, surface, tabId) {
  const key = `${surface}TabId`;
  if (record[key] !== tabId) return;
  record[key] = null;
  setMonitoring(tabId, "stop_monitoring");
  if (!record.playerTabId && !record.chatTabId) managedTabs.delete(channelId);
  void saveManagedTabs();
  sendNative({ type: "tab_closed", channel_id: channelId, tab_id: tabId, surface });
}

async function handleNativeCommand(message, sourcePort = nativePort) {
  await restorePromise;
  if (nativePort !== sourcePort) return;
  switch (message?.type) {
    case "bridge_ready":
      nativeBridgeReadyPort = sourcePort;
      nativeHostError = null;
      break;
    case "sync_channels": {
      const channels = (message.channels || []).filter((channel) => isValidChannel(channel));
      channelsByLogin = new Map(
        channels.map((channel) => [channel.login.toLowerCase(), channel.id]),
      );
      syncedNativePort = sourcePort;
      const loginByChannelId = new Map(
        channels.map((channel) => [channel.id, channel.login.toLowerCase()]),
      );
      await openChannels(
        Array.from(managedTabs.keys()).flatMap((channelId) => {
          const login = loginByChannelId.get(channelId);
          return login ? [{ id: channelId, login }] : [];
        }),
        false,
        sourcePort,
      );
      scheduleExternalStreamReport();
      break;
    }
      break;
    case "open_selected":
      await openChannels(
        message.channels || [],
        message.wake_browser === true,
        sourcePort,
      );
      break;
    case "close_all":
      await closeAllChannels();
      break;
    case "close_channels":
      await closeChannels(message.channel_ids || []);
      break;
  }
}

function isValidChannel(channel) {
  return typeof channel?.id === "string"
    && channel.id.length > 0
    && channel.id.length <= 64
    && typeof channel?.login === "string"
    && /^[a-z0-9_]{1,25}$/i.test(channel.login);
}

async function openChannels(channels, wakeBrowser, sourcePort) {
  let nextChannel = 0;
  const workerCount = Math.min(MAX_CONCURRENT_CHANNEL_OPENS, channels.length);
  const openedTabIds = new Array(channels.length);
  await Promise.all(Array.from({ length: workerCount }, async () => {
    while (nextChannel < channels.length) {
      const channelIndex = nextChannel++;
      const channel = channels[channelIndex];
      if (!isValidChannel(channel)) continue;
      try {
        openedTabIds[channelIndex] = await openChannel(
          channel,
          wakeBrowser,
          sourcePort,
        );
      } catch (error) {
        sendNative({
          type: "error",
          channel_id: channel.id,
          message: `Could not open Twitch tabs for ${channel.login.toLowerCase()}: ${String(error).slice(0, 160)}`,
        });
      }
    }
  }));
  if (wakeBrowser && workerCount > 1) {
    for (let index = openedTabIds.length - 1; index >= 0; index -= 1) {
      const tabId = openedTabIds[index];
      if (!Number.isInteger(tabId)) continue;
      try {
        await activateManagedTab(tabId, true);
        break;
      } catch (error) {
        if (index === 0) throw error;
      }
    }
  }
  scheduleExternalStreamReport();
}

async function openChannel(channel, wakeBrowser, sourcePort) {
  const login = channel.login.toLowerCase();
  let record = managedTabs.get(channel.id);
  if (
    record?.layout === "regular_website"
    && Number.isInteger(record.playerTabId)
    && record.playerTabId === record.chatTabId
  ) {
    record.layout = "combined_window";
    managedTabs.set(channel.id, record);
    await saveManagedTabs();
  }
  if (
    record
    && (record.layout !== "combined_window" || record.chatTabId !== record.playerTabId)
  ) {
    try {
      await closeChannels([channel.id]);
    } catch (error) {
      sendNative({
        type: "error",
        channel_id: channel.id,
        message: `Could not migrate Twitch tabs for ${login}: ${String(error).slice(0, 160)}`,
      });
      return;
    }
    record = managedTabs.get(channel.id) || null;
    if (record) return;
  }
  record = record || {
    login,
    playerTabId: null,
    chatTabId: null,
  };
  if (record.login !== login) {
    const previousLogins = new Set(
      Array.isArray(record.previousLogins) ? record.previousLogins : [],
    );
    previousLogins.add(record.login);
    record.previousLogins = Array.from(previousLogins);
  }
  record.login = login;
  record.layout = "combined_window";
  managedTabs.set(channel.id, record);
  try {
    const url = `https://www.twitch.tv/${encodeURIComponent(login)}`;
    await openManagedTab(channel.id, record, url, wakeBrowser, sourcePort);
    delete record.previousLogins;
    record.chatTabId = record.playerTabId;
    managedTabs.set(channel.id, record);
    await saveManagedTabs();
    sendNative({
      type: "tab_opened",
      channel_id: channel.id,
      tab_id: record.chatTabId,
      surface: "chat",
    });
    return record.playerTabId;
  } catch (error) {
    sendNative({
      type: "error",
      channel_id: channel.id,
      message: `Could not open Twitch tabs for ${login}: ${String(error).slice(0, 160)}`,
    });
  }
}

async function ensureDedicatedTabWindow(tabId) {
  const tab = await chrome.tabs.get(tabId);
  const window = await chrome.windows.get(tab.windowId, { populate: true });
  const windowTabs = window.tabs || await chrome.tabs.query({ windowId: tab.windowId });
  if (windowTabs.length > 1) {
    await chrome.windows.create({ tabId, focused: false, type: "normal" });
  }
}

async function createManagedTab() {
  const window = await chrome.windows.create({
    url: "about:blank",
    focused: false,
    type: "normal",
  });
  if (!Number.isInteger(window?.id)) {
    throw new Error("Could not create a dedicated Twitch window.");
  }
  const [tab] = await chrome.tabs.query({ windowId: window.id });
  if (!Number.isInteger(tab?.id)) {
    throw new Error("Could not create a Twitch tab in its window.");
  }
  return tab;
}

async function activateManagedTab(tabId, wakeBrowser, changes = {}) {
  const tab = await chrome.tabs.get(tabId);
  if (wakeBrowser) {
    await chrome.windows.update(tab.windowId, { state: "normal", focused: true });
  }
  await chrome.tabs.update(tabId, { ...changes, active: true });
}

function waitForTabLoad(tabId) {
  let finish;
  const promise = new Promise((resolve) => {
    let timeout;
    const settle = (loaded) => {
      chrome.tabs.onUpdated.removeListener(onUpdated);
      clearTimeout(timeout);
      resolve(loaded);
    };
    const onUpdated = (updatedTabId, changeInfo) => {
      if (updatedTabId === tabId && changeInfo.status === "complete") settle(true);
    };
    finish = settle;
    chrome.tabs.onUpdated.addListener(onUpdated);
    timeout = setTimeout(() => settle(false), 30000);
  });
  return { promise, cancel: (loaded) => finish(loaded) };
}

async function loadManagedTab(tabId, url, wakeBrowser, forceReload) {
  const tab = await chrome.tabs.get(tabId);
  if (!forceReload && !tab.discarded && tab.url === url && tab.status === "complete") {
    await activateManagedTab(tabId, wakeBrowser);
    return;
  }
  const loading = waitForTabLoad(tabId);
  try {
    const changes = {};
    if (forceReload || tab.url !== url || (!tab.discarded && tab.status !== "loading")) {
      changes.url = url;
    }
    await activateManagedTab(tabId, wakeBrowser, changes);
    const latest = await chrome.tabs.get(tabId);
    if (latest.url === url && latest.status === "complete" && !latest.discarded) {
      loading.cancel(true);
    }
    if (!await loading.promise) {
      throw new Error("Twitch page did not finish loading within 30 seconds.");
    }
  } catch (error) {
    loading.cancel(false);
    throw error;
  }
}

async function openManagedTab(channelId, record, url, wakeBrowser, sourcePort) {
  let tabId = record.playerTabId;
  if (tabId !== null && tabId !== undefined) {
    try {
      await chrome.tabs.get(tabId);
    } catch {
      record.playerTabId = null;
      record.chatTabId = null;
      tabId = null;
    }
  }
  if (tabId === null || tabId === undefined) {
    const tab = await createManagedTab();
    tabId = tab.id;
    record.playerTabId = tabId;
    record.chatTabId = tabId;
    record.pendingUrl = url;
    managedTabs.set(channelId, record);
    await saveManagedTabs();
  } else {
    await ensureDedicatedTabWindow(tabId);
  }
  await chrome.tabs.update(tabId, { autoDiscardable: false, muted: true });
  const tab = await chrome.tabs.get(tabId);
  const contentVersion = chrome.runtime.getManifest().version;
  const forceReload = record.contentVersion !== contentVersion;
  if (tab.url !== url || tab.discarded || tab.status !== "complete" || forceReload) {
    record.pendingUrl = url;
    managedTabs.set(channelId, record);
    await saveManagedTabs();
  }
  await loadManagedTab(tabId, url, wakeBrowser, forceReload);
  delete record.pendingUrl;
  record.contentVersion = contentVersion;
  managedTabs.set(channelId, record);
  await saveManagedTabs();
  if (nativePort === sourcePort) setMonitoring(tabId, "start_monitoring");
  sendNative({ type: "tab_opened", channel_id: channelId, tab_id: tabId, surface: "player" });
}


async function closeAllChannels() {
  await closeChannels(Array.from(managedTabs.keys()));
}

async function closeChannels(channelIds) {
  for (const channelId of new Set(channelIds.filter((id) => typeof id === "string"))) {
    const record = managedTabs.get(channelId);
    if (!record) continue;
    const tabSurfaces = new Map();
    for (const [surface, tabId] of managedSurfaceTabs(record)) {
      if (!tabSurfaces.has(tabId)) tabSurfaces.set(tabId, []);
      tabSurfaces.get(tabId).push(surface);
    }
    if (tabSurfaces.size === 0) {
      managedTabs.delete(channelId);
      continue;
    }
    for (const [tabId, surfaces] of tabSurfaces) {
      closingManagedTabIds.add(tabId);
      let removed = false;
      try {
        await chrome.tabs.remove(tabId);
        removed = true;
      } catch (error) {
        try {
          removed = !(await chrome.tabs.query({})).some((tab) => tab.id === tabId);
        } catch {
          removed = false;
        }
        if (!removed) {
          sendNative({
            type: "error",
            channel_id: channelId,
            message: `Could not close managed Twitch tab ${tabId}: ${String(error).slice(0, 160)}`,
          });
        }
      } finally {
        closingManagedTabIds.delete(tabId);
      }
      if (!removed) continue;
      const current = managedTabs.get(channelId);
      if (!current) continue;
      for (const surface of surfaces) {
        const key = `${surface}TabId`;
        if (current[key] !== tabId) continue;
        current[key] = null;
        sendNative({ type: "tab_closed", channel_id: channelId, tab_id: tabId, surface });
      }
      if (!current.playerTabId && !current.chatTabId) managedTabs.delete(channelId);
    }
  }
  await saveManagedTabs();
  scheduleExternalStreamReport();
}

function forwardChannelState(message, sender) {
  const tabId = sender.tab?.id;
  const surface = message?.surface;
  if (!nativePort || syncedNativePort !== nativePort) return;
  if (
    sender.frameId !== 0
    || tabId === undefined
    || !["player", "chat"].includes(surface)
    || typeof message?.login !== "string"
  ) return;
  for (const [channelId, record] of managedTabs) {
    if (record[`${surface}TabId`] !== tabId) continue;
    const login = message.login.toLowerCase();
    const previousLogins = Array.isArray(record.previousLogins) ? record.previousLogins : [];
    if (record.login !== login && !previousLogins.includes(login)) {
      detachManagedTab(channelId, record, surface, tabId);
      return;
    }
    if (channelsByLogin.get(login) !== channelId) return;
    sendNative({
      type: "channel_status",
      channel_id: channelId,
      tab_id: tabId,
      surface,
      ...(surface === "player"
        ? { player_state: message.player_state }
        : { chat_state: message.chat_state }),
      ...(surface === "player" && record.layout === "combined_window"
        ? { chat_state: message.chat_state }
        : {}),
    });
    return;
  }
}

chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  if (message?.type === "wake_native_host") {
    return handleNativeHostWakeRequest(message.forceReconnect === true, sendResponse);
  }
  if (message?.type === "native_host_status") {
    sendResponse(nativeHostStatus());
    return false;
  }
  if (message?.type === "channel_status") {
    forwardChannelState(message, sender);
  }
});

chrome.tabs.onCreated.addListener(() => {
  scheduleExternalStreamReport();
});

chrome.tabs.onRemoved.addListener((tabId) => {
  if (closingManagedTabIds.has(tabId)) return;
  for (const [channelId, record] of managedTabs) {
    const removedSurfaces = managedSurfaceTabs(record)
      .filter(([, managedTabId]) => managedTabId === tabId)
      .map(([surface]) => surface);
    if (removedSurfaces.length === 0) continue;
    for (const surface of removedSurfaces) {
      detachManagedTab(channelId, record, surface, tabId);
    }
    break;
  }
  scheduleExternalStreamReport();
});

function urlMatchesManagedSurface(value, login, surface, layout) {
  const route = TwitchTabUtils.channelRoute(value);
  if (route.login !== login) return false;
  if (layout === "combined_window" || layout === "regular_website") {
    return route.website && ["player", "chat"].includes(surface);
  }
  return !route.website && route.surface === surface;
}

chrome.tabs.onUpdated.addListener((tabId, changeInfo) => {
  if (nativePort && syncedNativePort === nativePort && changeInfo.status === "complete") {
    for (const record of managedTabs.values()) {
      if (managedSurfaceTabs(record).some(([, managedTabId]) => managedTabId === tabId)) {
        setMonitoring(tabId, "start_monitoring");
        break;
      }
    }
  }
  if (!changeInfo.url || changeInfo.url === "about:blank") return;
  scheduleExternalStreamReport();
  for (const [channelId, record] of managedTabs) {
    const updatedSurfaces = managedSurfaceTabs(record)
      .filter(([, managedTabId]) => managedTabId === tabId)
      .map(([surface]) => surface);
    if (updatedSurfaces.length === 0) continue;
    for (const surface of updatedSurfaces) {
      const routeLogins = [
        record.login,
        ...(Array.isArray(record.previousLogins) ? record.previousLogins : []),
      ];
      if (!routeLogins.some(
        (login) => urlMatchesManagedSurface(changeInfo.url, login, surface, record.layout),
      )) {
        detachManagedTab(channelId, record, surface, tabId);
      }
    }
    return;
  }
});

chrome.alarms.onAlarm.addListener((alarm) => {
  void handleNativeHostSyncAlarm(alarm);
});
chrome.runtime.onStartup.addListener(() => {
  void initializeNativeHostSync();
});
chrome.runtime.onInstalled.addListener(() => {
  void initializeNativeHostSync();
});
const initialNativeHostSync = initializeNativeHostSync();