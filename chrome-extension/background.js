const NATIVE_HOST = "com.twitchstreaksaver.bridge";
const NATIVE_HOST_SYNC_ALARM = "twitch-streak-saver-native-host-sync";
// MV3 alarms wake suspended workers; active workers retry failed native connections after 5 seconds.
const NATIVE_HOST_SYNC_INTERVAL_MINUTES = 0.5;
const NATIVE_HOST_RETRY_DELAY_MS = 5000;
importScripts("twitch_tabs.js", "quality.js", "managed_tabs.js");

let nativePort = null;
let syncedNativePort = null;
let nativeBridgeReadyPort = null;
let nativeHostError = null;
let reconnectTimer = null;
let channelsByLogin = new Map();
let externalScanTimer = null;
let externalScanGeneration = 0;
let nativeCommandQueue = Promise.resolve();
const MAX_CONCURRENT_CHANNEL_OPENS = 2;
let nativeSyncPromise = null;
const BROWSER_STARTUP_EVENT_GRACE_PERIOD_MS = 100;
let browserStartupOwnershipReset = Promise.resolve();
chrome.runtime.onStartup.addListener(() => {
  browserStartupOwnershipReset = TwitchManagedTabs.invalidateManagedTabOwners();
});
const restorePromise = new Promise((resolve) =>
  setTimeout(resolve, BROWSER_STARTUP_EVENT_GRACE_PERIOD_MS),
).then(() => browserStartupOwnershipReset).then(TwitchManagedTabs.restoreManagedTabs);
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




async function reportExternalStreams() {
  if (!nativePort) return;
  const generation = ++externalScanGeneration;
  await restorePromise;
  const tabs = await chrome.tabs.query({});
  if (generation !== externalScanGeneration || !nativePort) return;
  const managedTabIds = new Set(
    Array.from(TwitchManagedTabs.recordValues()).flatMap((record) =>
      TwitchManagedTabs.managedSurfaceTabs(record).map(([, tabId]) => tabId),
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
    managed_channel_ids: Array.from(TwitchManagedTabs.recordEntries())
      .filter(([, record]) => Number.isInteger(record.playerTabId))
      .map(([channelId]) => channelId),
    raid_redirected_channel_ids: TwitchManagedTabs.getRaidRedirectedChannelIds(),
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
  for (const record of TwitchManagedTabs.recordValues()) {
    for (const [, tabId] of TwitchManagedTabs.managedSurfaceTabs(record)) {
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

async function handleNativeCommand(message, sourcePort = nativePort) {
  await restorePromise;
  if (nativePort !== sourcePort) return;
  switch (message?.type) {
    case "bridge_ready":
      nativeBridgeReadyPort = sourcePort;
      nativeHostError = null;
      break;
    case "sync_channels": {
      try {
        await TwitchManagedTabs.setCloseOnRaidRedirect(
          message.close_on_raid_redirect === true,
        );
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
      const unmutedByChannelId = new Map(
        channels.map((channel) => [channel.id, channel.unmuted === true]),
      );
      await openChannels(
        Array.from(TwitchManagedTabs.recordKeys()).flatMap((channelId) => {
          const login = loginByChannelId.get(channelId);
          const unmuted = unmutedByChannelId.get(channelId) === true;
          return login ? [{ id: channelId, login, unmuted }] : [];
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
      await TwitchManagedTabs.clearRaidRedirectedChannelIds(message.channel_ids || []);
      break;
    case "set_preferred_quality": {
      const preferredQuality = TwitchQuality.normalizeQuality(message.preferred_quality);
      try {
        await chrome.storage.local.set({ preferredQuality });
      } catch (error) {
        console.warn("Could not persist the preferred Twitch quality.", error);
      }
      break;
    }
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
        await TwitchManagedTabs.activateManagedTab(tabId, true);
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
  if (!await TwitchManagedTabs.prepareChannelOpen(channel)) return;
  try {
    const url = `https://www.twitch.tv/${encodeURIComponent(login)}`;
    const playerTabId = await TwitchManagedTabs.openManagedTab(
      channel.id,
      url,
      wakeBrowser,
      channel.unmuted === true,
    );
    if (nativePort === sourcePort) setMonitoring(playerTabId, "start_monitoring");
    sendNative({
      type: "tab_opened",
      channel_id: channel.id,
      tab_id: playerTabId,
      surface: "player",
    });
    const chatTabId = await TwitchManagedTabs.finishChannelOpen(channel.id);
    sendNative({
      type: "tab_opened",
      channel_id: channel.id,
      tab_id: chatTabId,
      surface: "chat",
    });
    return playerTabId;
  } catch (error) {
    sendNative({
      type: "error",
      channel_id: channel.id,
      message: `Could not open Twitch tabs for ${login}: ${String(error).slice(0, 160)}`,
    });
  }
}




async function closeAllChannels() {
  await closeChannels(Array.from(TwitchManagedTabs.recordKeys()));
}

async function closeChannels(channelIds, closeReason = null) {
  for (const channelId of new Set(channelIds.filter((id) => typeof id === "string"))) {
    const record = TwitchManagedTabs.getRecord(channelId);
    if (!record) continue;
    const tabSurfaces = new Map();
    for (const [surface, tabId] of TwitchManagedTabs.managedSurfaceTabs(record)) {
      if (!tabSurfaces.has(tabId)) tabSurfaces.set(tabId, []);
      tabSurfaces.get(tabId).push(surface);
    }
    if (tabSurfaces.size === 0) {
      TwitchManagedTabs.deleteRecord(channelId);
      continue;
    }
    if (closeReason === "raid_redirect") {
      await TwitchManagedTabs.markRaidRedirectedChannel(channelId);
    }
    for (const [tabId, surfaces] of tabSurfaces) {
      TwitchManagedTabs.markClosingTab(tabId);
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
        TwitchManagedTabs.unmarkClosingTab(tabId);
      }
      if (!removed) continue;
      TwitchManagedTabs.deletePendingIdentityCheck(tabId);
      await TwitchManagedTabs.forgetManagedTabOwner(tabId);
      const closedSurfaces = TwitchManagedTabs.removeClosedManagedSurfaces(
        channelId,
        tabId,
        surfaces,
      );
      for (const surface of closedSurfaces) {
        sendNative({
          type: "tab_closed",
          channel_id: channelId,
          tab_id: tabId,
          surface,
          ...(closeReason ? { close_reason: closeReason } : {}),
        });
      }
    }
  }
  await TwitchManagedTabs.saveManagedTabs();
  scheduleExternalStreamReport();
}
TwitchManagedTabs.bindWorkerCallbacks({
  closeChannels,
  scheduleExternalStreamReport,
  setMonitoring,
});
async function forwardChannelState(message, sender) {
  const tabId = sender.tab?.id;
  const surface = message?.surface;
  if (!nativePort || syncedNativePort !== nativePort) return;
  if (
    sender.frameId !== 0
    || tabId === undefined
    || !["player", "chat"].includes(surface)
    || typeof message?.login !== "string"
  ) return;
  for (const [channelId, record] of TwitchManagedTabs.recordEntries()) {
    if (record[`${surface}TabId`] !== tabId) continue;
    const login = message.login.toLowerCase();
    const previousLogins = Array.isArray(record.previousLogins) ? record.previousLogins : [];
    if (record.login !== login && !previousLogins.includes(login)) {
      const detachedSurfaces = await TwitchManagedTabs.detachManagedTab(channelId, tabId);
      for (const detachedSurface of detachedSurfaces) {
        sendNative({
          type: "tab_closed",
          channel_id: channelId,
          tab_id: tabId,
          surface: detachedSurface,
        });
      }
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
    void forwardChannelState(message, sender).catch((error) => {
      console.warn("Could not process a browser channel status.", error);
    });
  }
  if (message?.type === "managed_tab_identity_ready") {
    const tabId = sender?.tab?.id;
    if (!Number.isInteger(tabId)) return false;
    Promise.resolve(restorePromise).then(
      async () => {
        const identityAccepted = TwitchManagedTabs.hasPendingIdentityCheck(tabId)
          ? await TwitchManagedTabs.resolvePendingStartupIdentity(tabId, message.identity)
          : false;
        const audioPreferenceAuthorized = await TwitchManagedTabs.applyManagedPlayerAudioPreference(
          tabId,
          message.identity,
        );
        sendResponse(identityAccepted || audioPreferenceAuthorized);
      },
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
  for (const [channelId, record] of TwitchManagedTabs.recordEntries()) {
    if (record.playerTabId !== tabId) continue;
    const owner = TwitchManagedTabs.getOwner(tabId);
    if (
      owner !== channelId
      && !TwitchManagedTabs.matchesManagedRecordRoute(record, tab.url, "player")
    ) continue;
    await TwitchManagedTabs.setPlayerWindow(channelId, tabId, attachInfo.newWindowId);
    break;
  }
});

chrome.tabs.onRemoved.addListener(async (tabId) => {
  TwitchManagedTabs.deletePendingIdentityCheck(tabId);
  if (TwitchManagedTabs.isClosingTab(tabId)) return;
  void TwitchManagedTabs.forgetManagedTabOwner(tabId);
  for (const [channelId, record] of TwitchManagedTabs.recordEntries()) {
    const removedSurfaces = TwitchManagedTabs.managedSurfaceTabs(record)
      .filter(([, managedTabId]) => managedTabId === tabId)
      .map(([surface]) => surface);
    if (removedSurfaces.length === 0) continue;
    const detachedSurfaces = await TwitchManagedTabs.detachManagedTab(channelId, tabId);
    for (const surface of detachedSurfaces) {
      sendNative({ type: "tab_closed", channel_id: channelId, tab_id: tabId, surface });
    }
    break;
  }
  scheduleExternalStreamReport();
});



chrome.tabs.onUpdated.addListener(async (tabId, changeInfo) => {

  await restorePromise;
  if (changeInfo.status === "complete" && TwitchManagedTabs.hasPendingIdentityCheck(tabId)) {
    const identity = await TwitchManagedTabs.readManagedTabIdentity(tabId);
    if (identity !== undefined) await TwitchManagedTabs.resolvePendingStartupIdentity(tabId, identity);
  }
  if (TwitchManagedTabs.hasPendingIdentityCheck(tabId)) {
    const tab = await chrome.tabs.get(tabId).catch(() => null);
    const url = changeInfo.url || tab?.url || "";
    if (
      !tab
      || !TwitchManagedTabs.hasRaidReferrer(url)
      || TwitchTabUtils.channelRoute(url).surface !== "player"
    ) {
      await TwitchManagedTabs.resolvePendingStartupIdentity(tabId, null);
    }
  }
  if (TwitchManagedTabs.hasPendingIdentityCheck(tabId)) return;
  if (nativePort && syncedNativePort === nativePort && changeInfo.status === "complete") {
    for (const record of TwitchManagedTabs.recordValues()) {
      if (TwitchManagedTabs.managedSurfaceTabs(record).some(([, managedTabId]) => managedTabId === tabId)) {
        setMonitoring(tabId, "start_monitoring");
        break;
      }
    }
  }
  if (!changeInfo.url || changeInfo.url === "about:blank") return;
  scheduleExternalStreamReport();
  for (const [channelId, record] of TwitchManagedTabs.recordEntries()) {
    const updatedSurfaces = TwitchManagedTabs.managedSurfaceTabs(record)
      .filter(([, managedTabId]) => managedTabId === tabId)
      .map(([surface]) => surface);
    if (updatedSurfaces.length === 0) continue;
    if (
      TwitchManagedTabs.isCloseOnRaidRedirectEnabled()
      && updatedSurfaces.includes("player")
      && TwitchManagedTabs.hasRaidReferrer(changeInfo.url)
      && TwitchManagedTabs.getOwner(tabId) === channelId
      && TwitchTabUtils.channelRoute(changeInfo.url).surface === "player"
    ) {
      return closeChannels([channelId], "raid_redirect");
    }
    const mismatchedSurfaces = updatedSurfaces.filter(
      (surface) => !TwitchManagedTabs.matchesManagedRecordRoute(record, changeInfo.url, surface),
    );
    if (mismatchedSurfaces.length > 0) {
      const detachedSurfaces = await TwitchManagedTabs.detachManagedTab(channelId, tabId);
      for (const surface of detachedSurfaces) {
        sendNative({ type: "tab_closed", channel_id: channelId, tab_id: tabId, surface });
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