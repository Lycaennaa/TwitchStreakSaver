const NATIVE_HOST = "com.twitchstreaksaver.bridge";
const NATIVE_HOST_SYNC_ALARM = "twitch-streak-saver-native-host-sync";
// MV3 alarms wake suspended workers; active workers retry failed native connections after 5 seconds.
const NATIVE_HOST_SYNC_INTERVAL_MINUTES = 0.5;
const NATIVE_HOST_RETRY_DELAY_MS = 5000;
const MANAGED_TAB_IDENTITY_POLICY_VERSION = 1;
importScripts("twitch_tabs.js");

let nativePort = null;
let syncedNativePort = null;
let nativeBridgeReadyPort = null;
let nativeHostError = null;
let reconnectTimer = null;
let managedTabs = new Map();
let closeOnRaidRedirect = false;
let raidRedirectedChannelIds = new Set();
const closingManagedTabIds = new Set();
let channelsByLogin = new Map();
let externalScanTimer = null;
let externalScanGeneration = 0;
let nativeCommandQueue = Promise.resolve();
const MAX_CONCURRENT_CHANNEL_OPENS = 2;
let managedTabsSaveQueue = Promise.resolve();
let raidRedirectedChannelIdsSaveQueue = Promise.resolve();
let managedTabOwners = new Map();
let managedTabOwnersSaveQueue = Promise.resolve();
const pendingStartupIdentityChecks = new Map();
let nativeSyncPromise = null;
const BROWSER_STARTUP_EVENT_GRACE_PERIOD_MS = 100;
let browserStartupOwnershipReset = Promise.resolve();
let browserStartupOwnershipInvalidated = false;
chrome.runtime.onStartup.addListener(() => {
  browserStartupOwnershipReset = invalidateManagedTabOwners();
});
const restorePromise = new Promise((resolve) =>
  setTimeout(resolve, BROWSER_STARTUP_EVENT_GRACE_PERIOD_MS),
).then(() => browserStartupOwnershipReset).then(restoreManagedTabs);
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
  const stored = await chrome.storage.local.get([
    "managedTabs",
    "closeOnRaidRedirect",
    "raidRedirectedChannelIds",
    "managedTabOwners",
  ]);
  managedTabs = new Map(stored.managedTabs || []);
  closeOnRaidRedirect = stored.closeOnRaidRedirect === true;
  raidRedirectedChannelIds = new Set(
    Array.isArray(stored.raidRedirectedChannelIds)
      ? stored.raidRedirectedChannelIds.filter((channelId) => typeof channelId === "string")
      : [],
  );
  managedTabOwners = browserStartupOwnershipInvalidated
    ? new Map()
    : new Map(stored.managedTabOwners || []);
  if (browserStartupOwnershipInvalidated) {
    for (const record of managedTabs.values()) delete record.playerWindowId;
  }
  const tabs = await chrome.tabs.query({});
  const tabsById = new Map(tabs.map((tab) => [tab.id, tab]));
  const raidRedirectedChannels = new Set();
  const claimedTabIds = new Set();
  for (const [channelId, record] of managedTabs) {
    if (record.tabId && !record.playerTabId) {
      record.playerTabId = record.tabId;
      delete record.tabId;
    }
    for (const surface of ["player", "chat"]) {
      const key = `${surface}TabId`;
      const tab = tabsById.get(record[key]);
      const owner = tab && managedTabOwners.get(tab.id);
      const raidPlayerRoute = surface === "player"
        && closeOnRaidRedirect
        && hasRaidReferrer(tab?.url || "")
        && TwitchTabUtils.channelRoute(tab?.url || "").surface === "player";
      if (
        surface === "player"
        && record.pendingRaidOwnershipVerification === true
        && !raidPlayerRoute
      ) delete record.pendingRaidOwnershipVerification;
      const tabIdentity = raidPlayerRoute
        ? await readManagedTabIdentity(tab.id)
        : null;
      const identityUnsafe = Number.isInteger(tab?.openerTabId);
      const identityMatches = !identityUnsafe
        && record.identityPolicyVersion === MANAGED_TAB_IDENTITY_POLICY_VERSION
        && typeof record.identityToken === "string"
        && tabIdentity === record.identityToken;
      const identityUnavailable = tabIdentity === undefined
        || tabIdentity === null
        || identityUnsafe;
      if (
        raidPlayerRoute
        && (
          owner === channelId
          || (
            !browserStartupOwnershipInvalidated
            && Number.isInteger(record.playerWindowId)
            && record.playerWindowId === tab.windowId
          )
          || identityMatches
        )
      ) {
        raidRedirectedChannels.add(channelId);
        claimedTabIds.add(tab.id);
        continue;
      }
      if (
        raidPlayerRoute
        && (
          record.pendingRaidOwnershipVerification === true
          || identityUnsafe
          || (
            browserStartupOwnershipInvalidated
            && (
              typeof record.identityToken !== "string"
              || record.identityPolicyVersion !== MANAGED_TAB_IDENTITY_POLICY_VERSION
              || (record.identityPersisted !== true && identityUnavailable)
            )
          )
          || (record.identityPersisted === false && identityUnavailable)
        )
      ) {
        record.pendingRaidOwnershipVerification = true;
        pendingStartupIdentityChecks.set(tab.id, channelId);
        claimedTabIds.add(tab.id);
        continue;
      }
      if (
        raidPlayerRoute
        && typeof record.identityToken === "string"
        && tabIdentity === undefined
      ) {
        pendingStartupIdentityChecks.set(tab.id, channelId);
        claimedTabIds.add(tab.id);
        continue;
      }
      const routeLogins = [
        record.login,
        ...(Array.isArray(record.previousLogins) ? record.previousLogins : []),
      ];
      const ownershipMatches = tab && (
        owner === channelId
        || (owner === undefined && (
          surface !== "player"
          || !Number.isInteger(record.playerWindowId)
          || record.playerWindowId === tab.windowId
        ))
      );
      const currentPage = ownershipMatches && routeLogins.some(
        (login) => urlMatchesManagedSurface(tab.url, login, surface, record.layout),
      );
      const pendingPage = ownershipMatches
        && tab.url === "about:blank"
        && urlMatchesManagedSurface(record.pendingUrl, record.login, surface, record.layout);
      if ((currentPage || pendingPage) && tab) {
        claimedTabIds.add(tab.id);
        managedTabOwners.set(tab.id, channelId);
        if (surface === "player") record.playerWindowId = tab.windowId;
      } else {
        if (
          tab
          && owner === channelId
          && !raidRedirectedChannels.has(channelId)
        ) managedTabOwners.delete(tab.id);
        if (surface === "player") delete record.playerWindowId;
        record[key] = null;
      }
    }
    if (!record.playerTabId && !record.chatTabId) managedTabs.delete(channelId);
  }
  for (const tabId of managedTabOwners.keys()) {
    if (!claimedTabIds.has(tabId)) managedTabOwners.delete(tabId);
  }
  try {
    await saveManagedTabOwners();
  } catch (error) {
    console.warn("Could not persist restored managed tab ownership.", error);
  }
  await saveManagedTabs();
  if (raidRedirectedChannels.size > 0) {
    await closeChannels(Array.from(raidRedirectedChannels), "raid_redirect");
  }
}
async function readManagedTabIdentity(tabId) {
  try {
    const result = await chrome.tabs.sendMessage(tabId, {
      type: "get_managed_tab_identity",
    });
    return typeof result?.identity === "string" ? result.identity : null;
  } catch {
    return undefined;
  }
}

async function resolvePendingStartupIdentity(tabId, identity) {
  const channelId = pendingStartupIdentityChecks.get(tabId);
  if (!channelId) return false;
  pendingStartupIdentityChecks.delete(tabId);
  const record = managedTabs.get(channelId);
  if (!record || record.playerTabId !== tabId) return false;
  let tab;
  try {
    tab = await chrome.tabs.get(tabId);
  } catch {
    record.playerTabId = null;
    if (record.chatTabId === tabId) record.chatTabId = null;
    delete record.pendingRaidOwnershipVerification;
    if (!record.playerTabId && !record.chatTabId) managedTabs.delete(channelId);
    await saveManagedTabs();
    scheduleExternalStreamReport();
    return false;
  }
  const route = TwitchTabUtils.channelRoute(tab.url || "");
  const identityUnsafe = Number.isInteger(tab.openerTabId);
  const identityUnavailable = identity === undefined || identity === null || identityUnsafe;
  if (
    record.pendingRaidOwnershipVerification === true
    && (
      typeof record.identityToken !== "string"
      || record.identityPolicyVersion !== MANAGED_TAB_IDENTITY_POLICY_VERSION
      || identityUnsafe
      || (record.identityPersisted !== true && identityUnavailable)
    )
    && route.surface === "player"
    && hasRaidReferrer(tab.url || "")
  ) {
    pendingStartupIdentityChecks.set(tabId, channelId);
    return false;
  }
  delete record.pendingRaidOwnershipVerification;
  const routeLogins = [
    record.login,
    ...(Array.isArray(record.previousLogins) ? record.previousLogins : []),
  ];
  if (
    !identityUnsafe
    && record.identityPolicyVersion === MANAGED_TAB_IDENTITY_POLICY_VERSION
    && identity === record.identityToken
  ) {
    if (
      closeOnRaidRedirect
      && route.surface === "player"
      && hasRaidReferrer(tab.url || "")
    ) {
      await closeChannels([channelId], "raid_redirect");
      return true;
    }
    if (routeLogins.some(
      (login) => urlMatchesManagedSurface(tab.url, login, "player", record.layout),
    )) {
      managedTabOwners.set(tabId, channelId);
      record.playerWindowId = tab.windowId;
      await saveManagedTabOwners();
      await saveManagedTabs();
      return true;
    }
  }
  record.playerTabId = null;
  if (record.chatTabId === tabId) record.chatTabId = null;
  delete record.pendingRaidOwnershipVerification;
  managedTabOwners.delete(tabId);
  if (!record.playerTabId && !record.chatTabId) managedTabs.delete(channelId);
  await saveManagedTabOwners();
  await saveManagedTabs();
  scheduleExternalStreamReport();
  return false;
}

async function invalidateManagedTabOwners() {
  browserStartupOwnershipInvalidated = true;
  managedTabOwners = new Map();
  try {
    const stored = await chrome.storage.local.get("managedTabs");
    const records = new Map(stored.managedTabs || []);
    for (const record of records.values()) delete record.playerWindowId;
    await chrome.storage.local.set({
      managedTabs: Array.from(records.entries()),
      managedTabOwners: [],
    });
  } catch (error) {
    console.warn("Could not clear managed ownership from the prior browser session.", error);
  }
}

function saveManagedTabs() {
  const snapshot = Array.from(managedTabs.entries());
  const saving = managedTabsSaveQueue.then(() =>
    chrome.storage.local.set({ managedTabs: snapshot }),
  );
  managedTabsSaveQueue = saving.catch(() => {});
  return saving;
}

function saveManagedTabOwners() {
  const saving = managedTabOwnersSaveQueue.then(() =>
    chrome.storage.local.set({
      managedTabOwners: Array.from(managedTabOwners.entries()),
    }),
  );
  managedTabOwnersSaveQueue = saving.catch(() => {});
  return saving;
}

async function rememberManagedTabOwner(tabId, channelId) {
  if (managedTabOwners.get(tabId) === channelId) return;
  managedTabOwners.set(tabId, channelId);
  try {
    await saveManagedTabOwners();
  } catch (error) {
    console.warn("Could not persist managed tab ownership.", error);
  }
}

async function forgetManagedTabOwner(tabId) {
  if (!managedTabOwners.delete(tabId)) return;
  try {
    await saveManagedTabOwners();
  } catch (error) {
    console.warn("Could not clear managed tab ownership.", error);
  }
}

function saveRaidRedirectedChannelIds() {
  const saving = raidRedirectedChannelIdsSaveQueue.then(() =>
    chrome.storage.local.set({
      raidRedirectedChannelIds: Array.from(raidRedirectedChannelIds).sort(),
    }),
  );
  raidRedirectedChannelIdsSaveQueue = saving.catch(() => {});
  return saving;
}

async function clearRaidRedirectedChannelIds(channelIds) {
  const removedChannelIds = [];
  for (const channelId of new Set(channelIds.filter((id) => typeof id === "string"))) {
    if (raidRedirectedChannelIds.delete(channelId)) removedChannelIds.push(channelId);
  }
  if (removedChannelIds.length === 0) return;
  try {
    await saveRaidRedirectedChannelIds();
  } catch (error) {
    for (const channelId of removedChannelIds) raidRedirectedChannelIds.add(channelId);
    console.warn("Could not persist cleared raid redirect channels.", error);
    return;
  }
  scheduleExternalStreamReport();
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
    raid_redirected_channel_ids: Array.from(raidRedirectedChannelIds).sort(),
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
async function setManagedTabIdentity(tabId, identity) {
  try {
    const tab = await chrome.tabs.get(tabId);
    if (Number.isInteger(tab.openerTabId)) return false;
    const result = await chrome.tabs.sendMessage(tabId, {
      type: "set_managed_tab_identity",
      identity,
    });
    return result?.identity === identity;
  } catch {
    return false;
  }
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
  if (surface === "player") {
    delete record.pendingRaidOwnershipVerification;
    pendingStartupIdentityChecks.delete(tabId);
  }
  setMonitoring(tabId, "stop_monitoring");
  if (record.playerTabId !== tabId && record.chatTabId !== tabId) {
    void forgetManagedTabOwner(tabId);
  }
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
      closeOnRaidRedirect = message.close_on_raid_redirect === true;
      try {
        await chrome.storage.local.set({ closeOnRaidRedirect });
      } catch (error) {
        console.warn("Could not persist the raid redirect setting.", error);
      }
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
    case "clear_raid_redirected_channels":
      await clearRaidRedirectedChannelIds(message.channel_ids || []);
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
    record
    && (
      record.pendingRaidOwnershipVerification === true
      || pendingStartupIdentityChecks.get(record.playerTabId) === channel.id
    )
  ) return;
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
  record.identityToken = record.identityToken || crypto.randomUUID();
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
  await rememberManagedTabOwner(tabId, channelId);
  await chrome.tabs.update(tabId, { autoDiscardable: false, muted: true });
  const tab = await chrome.tabs.get(tabId);
  record.playerWindowId = tab.windowId;
  managedTabs.set(channelId, record);
  await saveManagedTabs();
  const contentVersion = chrome.runtime.getManifest().version;
  const forceReload = record.contentVersion !== contentVersion;
  if (tab.url !== url || tab.discarded || tab.status !== "complete" || forceReload) {
    record.pendingUrl = url;
    managedTabs.set(channelId, record);
    await saveManagedTabs();
  }
  await loadManagedTab(tabId, url, wakeBrowser, forceReload);
  record.identityPersisted = await setManagedTabIdentity(tabId, record.identityToken);
  if (record.identityPersisted) {
    record.identityPolicyVersion = MANAGED_TAB_IDENTITY_POLICY_VERSION;
  } else {
    delete record.identityPolicyVersion;
  }
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

async function closeChannels(channelIds, closeReason = null) {
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
    if (closeReason === "raid_redirect") {
      raidRedirectedChannelIds.add(channelId);
      try {
        await saveRaidRedirectedChannelIds();
      } catch (error) {
        console.warn("Could not persist a raid-redirected channel.", error);
      }
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
            ...(closeReason ? { close_reason: closeReason } : {}),
          });
        }
      } finally {
        closingManagedTabIds.delete(tabId);
      }
      if (!removed) continue;
      pendingStartupIdentityChecks.delete(tabId);
      await forgetManagedTabOwner(tabId);
      const current = managedTabs.get(channelId);
      if (!current) continue;
      for (const surface of surfaces) {
        const key = `${surface}TabId`;
        if (current[key] !== tabId) continue;
        current[key] = null;
        if (surface === "player") delete current.pendingRaidOwnershipVerification;
        sendNative({
          type: "tab_closed",
          channel_id: channelId,
          tab_id: tabId,
          surface,
          ...(closeReason ? { close_reason: closeReason } : {}),
        });
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
  if (message?.type === "managed_tab_identity_ready") {
    const tabId = sender?.tab?.id;
    if (!Number.isInteger(tabId)) return false;
    Promise.resolve(restorePromise).then(
      () => resolvePendingStartupIdentity(tabId, message.identity).then(sendResponse),
      () => sendResponse(false),
    );
    return true;
  }
});

chrome.tabs.onCreated.addListener(() => {
  scheduleExternalStreamReport();
});

chrome.tabs.onAttached.addListener(async (tabId, attachInfo) => {
  await restorePromise;
  let tab;
  try {
    tab = await chrome.tabs.get(tabId);
  } catch {
    return;
  }
  const route = TwitchTabUtils.channelRoute(tab.url || "");
  if (route.surface !== "player") return;
  for (const [channelId, record] of managedTabs) {
    if (record.playerTabId !== tabId) continue;
    const owner = managedTabOwners.get(tabId);
    const routeLogins = [
      record.login,
      ...(Array.isArray(record.previousLogins) ? record.previousLogins : []),
    ];
    if (
      owner !== channelId
      && !routeLogins.some(
        (login) => urlMatchesManagedSurface(tab.url, login, "player", record.layout),
      )
    ) continue;
    record.playerWindowId = attachInfo.newWindowId;
    managedTabs.set(channelId, record);
    await saveManagedTabs();
    break;
  }
});

chrome.tabs.onRemoved.addListener((tabId) => {
  pendingStartupIdentityChecks.delete(tabId);
  if (closingManagedTabIds.has(tabId)) return;
  void forgetManagedTabOwner(tabId);
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

function hasRaidReferrer(value) {
  try {
    return new URL(value).searchParams.getAll("referrer").includes("raid");
  } catch {
    return false;
  }
}

chrome.tabs.onUpdated.addListener(async (tabId, changeInfo) => {

  await restorePromise;
  if (changeInfo.status === "complete" && pendingStartupIdentityChecks.has(tabId)) {
    const identity = await readManagedTabIdentity(tabId);
    if (identity !== undefined) await resolvePendingStartupIdentity(tabId, identity);
  }
  if (pendingStartupIdentityChecks.has(tabId)) {
    const tab = await chrome.tabs.get(tabId).catch(() => null);
    const url = changeInfo.url || tab?.url || "";
    if (
      !tab
      || !hasRaidReferrer(url)
      || TwitchTabUtils.channelRoute(url).surface !== "player"
    ) {
      await resolvePendingStartupIdentity(tabId, null);
    }
  }
  if (pendingStartupIdentityChecks.has(tabId)) return;
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
    if (
      closeOnRaidRedirect
      && updatedSurfaces.includes("player")
      && hasRaidReferrer(changeInfo.url)
      && managedTabOwners.get(tabId) === channelId
      && TwitchTabUtils.channelRoute(changeInfo.url).surface === "player"
    ) {
      return closeChannels([channelId], "raid_redirect");
    }
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