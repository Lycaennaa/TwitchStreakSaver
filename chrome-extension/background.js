const NATIVE_HOST = "com.twitchstreaksaver.bridge";
importScripts("twitch_tabs.js");

let nativePort = null;
let reconnectTimer = null;
let reconnectDelay = 1000;
let managedTabs = new Map();
const closingManagedTabIds = new Set();
let channelsByLogin = new Map();
let externalScanTimer = null;
let externalScanGeneration = 0;
let nativeCommandQueue = Promise.resolve();

const restorePromise = restoreManagedTabs();

async function restoreManagedTabs() {
  const stored = await chrome.storage.session.get("managedTabs");
  managedTabs = new Map(stored.managedTabs || []);
  const tabs = await chrome.tabs.query({});
  const openTabIds = new Set(tabs.map((tab) => tab.id));
  for (const [channelId, record] of managedTabs) {
    if (record.tabId && !record.playerTabId) {
      record.playerTabId = record.tabId;
      delete record.tabId;
    }
    for (const surface of ["player", "chat"]) {
      const key = `${surface}TabId`;
      if (!openTabIds.has(record[key])) record[key] = null;
    }
    if (!record.playerTabId && !record.chatTabId) managedTabs.delete(channelId);
  }
  await saveManagedTabs();
}

async function saveManagedTabs() {
  await chrome.storage.session.set({ managedTabs: Array.from(managedTabs.entries()) });
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
  sendNative({ type: "external_streams", ...inventory });
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
  try {
    const port = chrome.runtime.connectNative(NATIVE_HOST);
    nativePort = port;
    port.onMessage.addListener((message) => {
      nativeCommandQueue = nativeCommandQueue
        .then(() => handleNativeCommand(message))
        .catch((error) => {
          console.warn("Could not handle Twitch Streak Saver command.", error);
        });
    });
    port.onDisconnect.addListener(() => {
      nativePort = null;
      for (const record of managedTabs.values()) {
        for (const [, tabId] of managedSurfaceTabs(record)) {
          setMonitoring(tabId, "stop_monitoring");
        }
      }
      scheduleReconnect();
    });
    port.postMessage({ type: "ready" });
  } catch (error) {
    console.warn("Twitch Streak Saver native host is unavailable.", error);
    scheduleReconnect();
  }
}

function scheduleReconnect() {
  if (reconnectTimer !== null) return;
  const delay = reconnectDelay;
  reconnectDelay = Math.min(reconnectDelay * 2, 10000);
  reconnectTimer = setTimeout(() => {
    reconnectTimer = null;
    connectNativeHost();
  }, delay);
}

function sendNative(message) {
  if (!nativePort) return;
  nativePort.postMessage(message);
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

async function handleNativeCommand(message) {
  await restorePromise;
  switch (message?.type) {
    case "bridge_ready":
      reconnectDelay = 1000;
      break;
    case "sync_channels":
      channelsByLogin = new Map(
        (message.channels || [])
          .filter((channel) => isValidChannel(channel))
          .map((channel) => [channel.login.toLowerCase(), channel.id]),
      );
      for (const record of managedTabs.values()) {
        for (const [, tabId] of managedSurfaceTabs(record)) {
          setMonitoring(tabId, "start_monitoring");
        }
      }
      scheduleExternalStreamReport();
      break;
    case "open_selected":
      await openChannels(
        message.channels || [],
        message.layout,
        message.wake_browser === true,
      );
      break;
    case "apply_layout": {
      const requestedLayout = message.layout === "regular_website"
        ? "regular_website"
        : "separate_player_chat";
      const channels = Array.from(managedTabs.entries())
        .filter(([, record]) => record.layout !== requestedLayout)
        .map(([id, record]) => ({ id, login: record.login }));
      await openChannels(channels, requestedLayout, message.wake_browser === true);
      break;
    }
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

async function openChannels(channels, layout, wakeBrowser) {
  const requestedLayout = layout === "regular_website" ? layout : "separate_player_chat";
  for (const channel of channels) {
    if (!isValidChannel(channel)) continue;
    const login = channel.login.toLowerCase();
    let record = managedTabs.get(channel.id);
    if (record && record.layout !== requestedLayout) {
      try {
        await closeChannels([channel.id]);
      } catch (error) {
        sendNative({
          type: "error",
          channel_id: channel.id,
          message: `Could not migrate Twitch tabs for ${login}: ${String(error).slice(0, 160)}`,
        });
        continue;
      }
      record = managedTabs.get(channel.id) || null;
      if (record) continue;
    }
    record = record || {
      login,
      layout: requestedLayout,
      playerTabId: null,
      chatTabId: null,
    };
    record.login = login;
    record.layout = requestedLayout;
    managedTabs.set(channel.id, record);
    try {
      if (requestedLayout === "regular_website") {
        const url = `https://www.twitch.tv/${encodeURIComponent(login)}`;
        await openManagedTab(channel.id, record, "player", url, wakeBrowser);
        record.chatTabId = record.playerTabId;
        managedTabs.set(channel.id, record);
        await saveManagedTabs();
        sendNative({
          type: "tab_opened",
          channel_id: channel.id,
          tab_id: record.chatTabId,
          surface: "chat",
        });
      } else {
        const urls = {
          player: `https://player.twitch.tv/?channel=${encodeURIComponent(login)}&parent=twitch.tv`,
          chat: `https://www.twitch.tv/popout/${encodeURIComponent(login)}/chat`,
        };
        for (const [surface, url] of Object.entries(urls)) {
          await openManagedTab(channel.id, record, surface, url, wakeBrowser);
        }
      }
    } catch (error) {
      sendNative({
        type: "error",
        channel_id: channel.id,
        message: `Could not open Twitch tabs for ${login}: ${String(error).slice(0, 160)}`,
      });
    }
  }
  scheduleExternalStreamReport();
}

async function focusManagedTab(tabId) {
  const tab = await chrome.tabs.get(tabId);
  await chrome.windows.update(tab.windowId, { state: "normal", focused: true });
  await chrome.tabs.update(tabId, { active: true });
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

async function loadManagedTab(tabId, url) {
  const tab = await chrome.tabs.get(tabId);
  if (tab.url === url && tab.status === "complete") return;
  const loading = waitForTabLoad(tabId);
  try {
    if (tab.url === url && tab.status === "loading") {
      const latest = await chrome.tabs.get(tabId);
      if (latest.url === url && latest.status === "complete") {
        loading.cancel(true);
        return;
      }
    } else {
      await chrome.tabs.update(tabId, { url });
    }
    if (!await loading.promise) {
      throw new Error("Twitch page did not finish loading within 30 seconds.");
    }
  } catch (error) {
    loading.cancel(false);
    throw error;
  }
}

async function openManagedTab(channelId, record, surface, url, wakeBrowser) {
  const key = `${surface}TabId`;
  let tabId = record[key];
  if (tabId !== null && tabId !== undefined) {
    try {
      await chrome.tabs.get(tabId);
    } catch {
      record[key] = null;
      tabId = null;
    }
  }
  if (tabId === null || tabId === undefined) {
    const tab = await chrome.tabs.create({ url: "about:blank", active: false });
    tabId = tab.id;
    record[key] = tabId;
    managedTabs.set(channelId, record);
    await saveManagedTabs();
  }
  if (surface === "player") {
    await chrome.tabs.update(tabId, { muted: true });
    if (wakeBrowser) await focusManagedTab(tabId);
  }
  await loadManagedTab(tabId, url);
  managedTabs.set(channelId, record);
  await saveManagedTabs();
  setMonitoring(tabId, "start_monitoring");
  sendNative({ type: "tab_opened", channel_id: channelId, tab_id: tabId, surface });
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
  if (
    sender.frameId !== 0
    || tabId === undefined
    || !["player", "chat"].includes(surface)
    || typeof message?.login !== "string"
  ) return;
  for (const [channelId, record] of managedTabs) {
    if (record[`${surface}TabId`] !== tabId) continue;
    if (record.login !== message.login.toLowerCase()) {
      detachManagedTab(channelId, record, surface, tabId);
      return;
    }
    if (channelsByLogin.get(record.login) !== channelId) return;
    sendNative({
      type: "channel_status",
      channel_id: channelId,
      tab_id: tabId,
      surface,
      ...(surface === "player"
        ? { player_state: message.player_state }
        : { chat_state: message.chat_state }),
      ...(surface === "player" && record.layout === "regular_website"
        ? { chat_state: message.chat_state }
        : {}),
    });
    return;
  }
}

chrome.runtime.onMessage.addListener((message, sender) => {
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
  if (layout === "regular_website") {
    return route.website && ["player", "chat"].includes(surface);
  }
  return !route.website && route.surface === surface;
}

chrome.tabs.onUpdated.addListener((tabId, changeInfo) => {
  if (nativePort && changeInfo.status === "complete") {
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
      if (!urlMatchesManagedSurface(changeInfo.url, record.login, surface, record.layout)) {
        detachManagedTab(channelId, record, surface, tabId);
      }
    }
    return;
  }
});

chrome.runtime.onStartup.addListener(connectNativeHost);
chrome.runtime.onInstalled.addListener(connectNativeHost);
connectNativeHost();