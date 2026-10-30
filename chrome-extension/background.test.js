const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const crypto = require("node:crypto");
const trustedExtensionId = fs.readFileSync(path.join(__dirname, "trusted-extension-id"), "utf8");
const qualitySource = fs.readFileSync(path.join(__dirname, "quality.js"), "utf8");
const BROWSER_STARTUP_EVENT_GRACE_PERIOD_MS = 100;
const test = require("node:test");
const vm = require("node:vm");

function event() {
  const listeners = new Set();
  return {
    addListener(listener) {
      listeners.add(listener);
    },
    removeListener(listener) {
      listeners.delete(listener);
    },
    dispatch(...args) {
      return [...listeners].map((listener) => listener(...args));
    },
  };
}

async function createExtension({
  pauseFirstWindow = false,
  pauseFirstNavigation = false,
  storedManagedTabs = [],
  storedCloseOnRaidRedirect = false,
  storedRaidRedirectedChannelIds = [],
  storedManagedTabOwners = [],
  storedAdoptExistingTwitchTabs = false,
  storedCompletedNativeCommandIds = [],
  browserStartup = false,
  initialTabs = [],
  extensionVersion = "0.3.18",
  runtimeId = trustedExtensionId,
  fetchImpl = async () => ({ status: 204 }),
  connectNativeImpl = null,
  failManagedIdentityWrites = false,
  setTimeoutImpl = setTimeout,
  clearTimeoutImpl = clearTimeout,
} = {}) {
  const tabsById = new Map();
  const windowsById = new Map([[17, { id: 17, focused: true, state: "normal", type: "normal" }]]);
  const calls = {
    windowCreates: [],
    windowUpdates: [],
    tabUpdates: [],
    tabReloads: [],
    nativeMessages: [],
    contentMessages: [],
    registrationRequests: [],
    nativeConnections: 0,
    alarms: [],
    logs: [],
  };
  let nextTabId = 1;
  let nextWindowId = 18;
  let storedTabs = JSON.parse(JSON.stringify(storedManagedTabs));
  let storedRaidSetting = storedCloseOnRaidRedirect;
  let storedRaidRedirectedIds = JSON.parse(JSON.stringify(storedRaidRedirectedChannelIds));
  let storedTabOwners = JSON.parse(JSON.stringify(storedManagedTabOwners));
  let storedAdoptionEnabled = storedAdoptExistingTwitchTabs;
  let storedCompletedCommandIds = JSON.parse(JSON.stringify(storedCompletedNativeCommandIds));
  let storedPreferredQuality;
  let managedTabSaveFailures = 0;
  let storedTheme = "dark";
  let managedTabSavePause = null;
  let managedTabIdentityReadPause = null;
  let raidRedirectedSavePause = null;
  let completedCommandSaveFailures = 0;
  const browserMutationWaiters = [];
  function signalBrowserMutation(mutation) {
    for (const resolve of browserMutationWaiters.splice(0)) resolve(mutation);
  }
  for (const initialTab of initialTabs) {
    const tab = {
      status: "complete",
      active: false,
      discarded: false,
      autoDiscardable: true,
      windowId: 17,
      ...initialTab,
    };
    tabsById.set(tab.id, tab);
    nextTabId = Math.max(nextTabId, tab.id + 1);
    if (!windowsById.has(tab.windowId)) {
      windowsById.set(tab.windowId, {
        id: tab.windowId,
        focused: false,
        state: "normal",
        type: "normal",
      });
    }
    nextWindowId = Math.max(nextWindowId, tab.windowId + 1);
  }
  const failedRemovals = new Set();
  let notifyFirstWindow;
  const firstWindowStarted = new Promise((resolve) => {
    notifyFirstWindow = resolve;
  });
  let releaseFirstWindow;
  const firstWindowGate = new Promise((resolve) => {
    releaseFirstWindow = resolve;
  });
  let windowCreateCount = 0;
  let notifyFirstNavigation;
  const firstNavigationStarted = new Promise((resolve) => {
    notifyFirstNavigation = resolve;
  });
  let releaseFirstNavigation;
  const firstNavigationGate = new Promise((resolve) => {
    releaseFirstNavigation = resolve;
  });
  let navigationCount = 0;
  let reloadPause = null;
  const onUpdated = event();
  const onAttached = event();
  const onCreated = event();
  const onRemoved = event();
  let runtimeLastError = null;
  let runtimeLastErrorReads = 0;
  const createPort = () => ({
    onMessage: event(),
    onDisconnect: event(),
    postMessage(message) {
      calls.nativeMessages.push(message);
    },
    disconnect() {
      this.onDisconnect.dispatch();
    },
  });
  let port = createPort();
  const chrome = {
    storage: {
      local: {
        async get(keys) {
          const values = {
            managedTabs: storedTabs,
            closeOnRaidRedirect: storedRaidSetting,
            raidRedirectedChannelIds: storedRaidRedirectedIds,
            managedTabOwners: storedTabOwners,
            adoptExistingTwitchTabs: storedAdoptionEnabled,
            completedNativeCommandIds: storedCompletedCommandIds,
            theme: storedTheme,
          };
          if (storedPreferredQuality !== undefined) {
            values.preferredQuality = storedPreferredQuality;
          } else if (keys && !Array.isArray(keys) && Object.hasOwn(keys, "preferredQuality")) {
            values.preferredQuality = keys.preferredQuality;
          }
          return values;
        },
        async set(value) {
          if (Object.hasOwn(value, "managedTabs")) {
            if (managedTabSaveFailures > 0) {
              managedTabSaveFailures -= 1;
              throw new Error("Managed tab storage failed");
            }
          if (managedTabSavePause) {
            const pause = managedTabSavePause;
            managedTabSavePause = null;
            pause.signalStarted();
            await pause.wait;
            if (pause.failOnRelease) throw new Error("Managed tab storage failed");
          }
          storedTabs = JSON.parse(JSON.stringify(value.managedTabs));
          }
          if (Object.hasOwn(value, "closeOnRaidRedirect")) {
            storedRaidSetting = value.closeOnRaidRedirect;
          }
          if (Object.hasOwn(value, "raidRedirectedChannelIds")) {
          const pause = raidRedirectedSavePause;
          if (pause) {
            raidRedirectedSavePause = null;
            pause.signalStarted();
            await pause.wait;
          }
            storedRaidRedirectedIds = JSON.parse(JSON.stringify(value.raidRedirectedChannelIds));
          }
          if (Object.hasOwn(value, "managedTabOwners")) {
            storedTabOwners = JSON.parse(JSON.stringify(value.managedTabOwners));
          }
        if (Object.hasOwn(value, "adoptExistingTwitchTabs")) {
          storedAdoptionEnabled = value.adoptExistingTwitchTabs;
        }
          if (Object.hasOwn(value, "completedNativeCommandIds")) {
            if (completedCommandSaveFailures > 0) {
              completedCommandSaveFailures -= 1;
              throw new Error("Completed native command ID storage failed");
            }
            storedCompletedCommandIds = JSON.parse(JSON.stringify(value.completedNativeCommandIds));
          }
          if (Object.hasOwn(value, "preferredQuality")) {
            storedPreferredQuality = value.preferredQuality;
          }
          if (Object.hasOwn(value, "theme")) {
            storedTheme = value.theme;
          }
        },
      },
    },
    tabs: {
      onUpdated,
      onAttached,
      onCreated,
      onRemoved,
      async query(query = {}) {
        return [...tabsById.values()]
          .filter((tab) => query.windowId === undefined || tab.windowId === query.windowId)
          .map((tab) => ({ ...tab }));
      },
      async reload(tabId) {
        if (!tabsById.has(tabId)) throw new Error(`No such tab: ${tabId}`);
        calls.tabReloads.push(tabId);
        if (reloadPause) {
          const pause = reloadPause;
          reloadPause = null;
          pause.signalStarted();
          await pause.wait;
        }
      },
      async get(tabId) {
        const tab = tabsById.get(tabId);
        if (!tab) throw new Error("No such tab");
        return { ...tab };
      },
      async update(tabId, changes) {
        const tab = tabsById.get(tabId);
        if (!tab) throw new Error("No such tab");
        calls.tabUpdates.push({ tabId, changes: { ...changes } });
        if (changes.active === true) {
          for (const sibling of tabsById.values()) {
            if (sibling.windowId === tab.windowId) sibling.active = sibling.id === tabId;
          }
        }
        const restoreDiscarded = tab.discarded && changes.active === true;
        Object.assign(tab, changes);
        if (restoreDiscarded) tab.discarded = false;
        if (changes.url !== undefined || restoreDiscarded) {
          tab.status = "loading";
          if (pauseFirstNavigation && navigationCount++ === 0) {
            notifyFirstNavigation();
            await firstNavigationGate;
          }
          setTimeout(() => {
            const current = tabsById.get(tabId);
            if (!current) return;
            current.status = "complete";
            current.discarded = false;
            onUpdated.dispatch(
              tabId,
              { status: "complete", url: current.url },
              { ...current },
            );
          }, 0);
        }
        return { ...tab };
      },
      async remove(tabId) {
        if (failedRemovals.has(tabId)) throw new Error("Blocked tab removal");
        const tab = tabsById.get(tabId);
        if (!tab) throw new Error("No such tab");
        signalBrowserMutation({ type: "tab_removed", tabId });
        tabsById.delete(tabId);
        await Promise.all(onRemoved.dispatch(tabId));
      },
      async sendMessage(tabId, message) {
        const tab = tabsById.get(tabId);
        const contentMessage = {
          tabId,
          type: message.type,
          status: tab?.status,
          discarded: tab?.discarded,
        };
        if (message.type === "set_player_audio_preference") {
          contentMessage.unmuted = message.unmuted;
          contentMessage.login = message.login;
        }
        if (message.type === "start_monitoring") {
          contentMessage.viewer_login = message.viewer_login;
        }
        calls.contentMessages.push(contentMessage);
        if (message.type === "get_managed_tab_identity") {
          const pause = managedTabIdentityReadPause;
          if (pause) {
            managedTabIdentityReadPause = null;
            pause.signalStarted();
            await pause.wait;
          }
          if (!tab || tab.contentScriptReady === false) {
            throw new Error("No content script is available");
          }
          return {
            identity: typeof tab.managedTabIdentity === "string"
              ? tab.managedTabIdentity
              : null,
          };
        }
        if (message.type === "set_managed_tab_identity") {
          if (!tab) throw new Error("No such tab");
          if (failManagedIdentityWrites) return { identity: null };
          tab.managedTabIdentity = message.identity;
          return { identity: tab.managedTabIdentity };
        }
      },
    },
    windows: {
      async get(windowId, options = {}) {
        const window = windowsById.get(windowId);
        if (!window) throw new Error("No such window");
        return {
          ...window,
          ...(options.populate
            ? { tabs: [...tabsById.values()].filter((tab) => tab.windowId === windowId) }
            : {}),
        };
      },
      async create(options) {
        if (pauseFirstWindow && windowCreateCount++ === 0) {
          notifyFirstWindow();
          await firstWindowGate;
        }
        calls.windowCreates.push({ ...options });
        const window = {
          id: nextWindowId++,
          focused: options.focused !== false,
          state: options.state || "normal",
          type: options.type || "normal",
        };
        if (window.focused) {
          for (const existing of windowsById.values()) existing.focused = false;
        }
        windowsById.set(window.id, window);
        if (Number.isInteger(options.tabId)) {
          const tab = tabsById.get(options.tabId);
          if (!tab) throw new Error("No such tab");
          for (const sibling of tabsById.values()) {
            if (sibling.windowId === tab.windowId) sibling.active = false;
          }
          tab.windowId = window.id;
          tab.active = true;
        } else {
          const tab = {
            id: nextTabId++,
            windowId: window.id,
            status: "complete",
            active: true,
            discarded: false,
            autoDiscardable: true,
            url: options.url || "chrome://newtab/",
          };
          tabsById.set(tab.id, tab);
          signalBrowserMutation({
            type: "window_created",
            tabId: tab.id,
            windowId: window.id,
          });
          onCreated.dispatch({ ...tab });
        }
        return { ...window };
      },
      async update(windowId, options) {
        calls.windowUpdates.push({ windowId, options: { ...options } });
        const window = windowsById.get(windowId);
        if (!window) throw new Error("No such window");
        Object.assign(window, options);
        if (options.focused === true) {
          for (const existing of windowsById.values()) {
            existing.focused = existing.id === windowId;
          }
        }
      },
    },
    runtime: {
      id: runtimeId,
      onStartup: event(),
      onInstalled: event(),
      onMessage: event(),
      get lastError() {
        runtimeLastErrorReads += 1;
        return runtimeLastError;
      },
      getManifest() {
        return { version: extensionVersion };
      },
      connectNative() {
        calls.nativeConnections += 1;
        port = createPort();
        return connectNativeImpl ? connectNativeImpl(port) : port;
      },
      sendMessage(message) {
        return new Promise((resolve) => {
          const results = chrome.runtime.onMessage.dispatch(message, {}, resolve);
          if (!results.includes(true)) resolve(undefined);
        });
      },
    },
    alarms: {
      onAlarm: event(),
      create(name, options) {
        calls.alarms.push({ name, options: { ...options } });
      },
    },
  };
  let startupGraceTimerUsed = false;
  const context = vm.createContext({
    chrome,
    setTimeout: (callback, delay) => {
      if (
        !startupGraceTimerUsed
        && delay === BROWSER_STARTUP_EVENT_GRACE_PERIOD_MS
      ) {
        startupGraceTimerUsed = true;
        return setTimeout(callback, delay);
      }
      return setTimeoutImpl(callback, delay);
    },
    clearTimeout: clearTimeoutImpl,
    URL,
    AbortController,
    crypto,
    fetch(url, options) {
      calls.registrationRequests.push({
        url: String(url),
        options: {
          method: options.method,
          cache: options.cache,
          credentials: options.credentials,
          hasAbortSignal: options.signal !== undefined,
        },
      });
      return fetchImpl(url, options);
    },
    console: {
      debug(...args) {
        calls.logs.push({ level: "debug", message: args.map(String).join(" ") });
      },
      warn(...args) {
        calls.logs.push({ level: "warn", message: args.map(String).join(" ") });
      },
    },
  });
  vm.runInContext(qualitySource, context, { filename: "quality.js" });
  context.importScripts = (...files) => {
    for (const file of files) {
      const source = fs.readFileSync(path.join(__dirname, file), "utf8");
      vm.runInContext(source, context, { filename: file });
    }
  };
  const source = fs.readFileSync(path.join(__dirname, "background.js"), "utf8");
  vm.runInContext(source, context, { filename: "background.js" });
  if (browserStartup) chrome.runtime.onStartup.dispatch();
  await vm.runInContext("restorePromise", context);
  await vm.runInContext("initialNativeHostSync", context);
  return {
    calls,
    tabsById,
    windowsById,
    preferredQuality() {
      return storedPreferredQuality;
    },
    theme() {
      return storedTheme;
    },
    open(message) {
      return vm.runInContext(`handleNativeCommand(${JSON.stringify(message)})`, context);
    },
    openFromPort(message, sourcePort) {
      context.commandSourcePort = sourcePort;
      return vm.runInContext(
        `handleNativeCommand(${JSON.stringify(message)}, commandSourcePort)`,
        context,
      );
    },
    currentNativePort() {
      return vm.runInContext("nativePort", context);
    },
    send(message) {
      port.onMessage.dispatch(message);
      return vm.runInContext("nativeCommandQueue", context);
    },
    blockNativeCommandQueue() {
      let release;
      context.commandQueueGate = new Promise((resolve) => {
        release = resolve;
      });
      vm.runInContext("nativeCommandQueue = commandQueueGate", context);
      return release;
    },
    reportExternalStreams() {
      return vm.runInContext("reportExternalStreams()", context);
    },
    disconnectHost(error) {
      runtimeLastError = error;
      try {
        port.onDisconnect.dispatch();
      } finally {
        runtimeLastError = null;
      }
    },
    lastErrorReads() {
      return runtimeLastErrorReads;
    },
    isNativeConnected() {
      return vm.runInContext("nativePort !== null", context);
    },
    status() {
      return vm.runInContext("nativeHostStatus()", context);
    },
    synchronize() {
      return vm.runInContext("synchronizeNativeHost()", context);
    },
    openPopup() {
      const statusWaiters = [];
      const managedStreamWaiters = [];
      const actionStatusWaiters = [];
      const themeWaiters = [];
      const actionStatus = {
        value: "",
        get textContent() {
          return this.value;
        },
        set textContent(value) {
          this.value = value;
          actionStatusWaiters.shift()?.(value);
        },
      };
      const managedStreams = {
        value: "",
        get textContent() {
          return this.value;
        },
        set textContent(value) {
          const changed = value !== this.value;
          this.value = value;
          if (changed) managedStreamWaiters.shift()?.(value);
        },
      };
      const status = {
        value: "Waking the extension worker…",
        get textContent() {
          return this.value;
        },
        set textContent(value) {
          const changed = value !== this.value;
          this.value = value;
          if (changed && !value.startsWith("Waking")) statusWaiters.shift()?.(value);
        },
      };
      const makeButton = () => ({
        disabled: false,
        onClick: null,
        addEventListener(_name, listener) {
          this.onClick = listener;
        },
        click() {
          if (!this.disabled) this.onClick?.();
        },
      });
      const wakeButton = makeButton();
      const pauseButton = makeButton();
      const openSelectedButton = makeButton();
      const themeRoot = {
        value: null,
        setAttribute(name, value) {
          if (name !== "data-theme") return;
          this.value = value;
          for (const resolve of themeWaiters.splice(0)) resolve(value);
        },
      };
      context.document = {
        documentElement: themeRoot,
        querySelector(selector) {
          if (selector === "#status") return status;
          if (selector === "#wake") return wakeButton;
          if (selector === "#managed-streams") return managedStreams;
          if (selector === "#action-status") return actionStatus;
          if (selector === "#pause") return pauseButton;
          if (selector === "#open-selected") return openSelectedButton;
          return null;
        },
      };
      const popupSource = fs.readFileSync(path.join(__dirname, "popup.js"), "utf8");
      const firstStatus = new Promise((resolve) => statusWaiters.push(resolve));
      vm.runInContext(popupSource, context, { filename: "popup.js" });
      return {
        status,
        managedStreams,
        actionStatus,
        pauseButton,
        openSelectedButton,
        wakeButton,
        theme() {
          return themeRoot.value;
        },
        themeApplied() {
          return themeRoot.value === null
            ? new Promise((resolve) => themeWaiters.push(resolve))
            : Promise.resolve(themeRoot.value);
        },
        firstStatus,
        nextStatus() {
          return new Promise((resolve) => statusWaiters.push(resolve));
        },
        nextActionStatus() {
          return new Promise((resolve) => actionStatusWaiters.push(resolve));
        },
        nextManagedStreams() {
          return new Promise((resolve) => managedStreamWaiters.push(resolve));
        },
      };
    },
    async syncOnAlarm() {
      return vm.runInContext(
        "handleNativeHostSyncAlarm({ name: NATIVE_HOST_SYNC_ALARM })",
        context,
      );
    },
    cancelReconnect() {
      vm.runInContext(
        "if (reconnectTimer !== null) clearTimeout(reconnectTimer); reconnectTimer = null;",
        context,
      );
    },
    reconnectHost() {
      vm.runInContext(
        "if (reconnectTimer !== null) clearTimeout(reconnectTimer); reconnectTimer = null;",
        context,
      );
      port = createPort();
      vm.runInContext("connectNativeHost()", context);
    },
    snapshotManagedTabs() {
      return JSON.parse(JSON.stringify(storedTabs));
    },
    managedRecordEntries() {
      return vm.runInContext("TwitchManagedTabs.recordEntries()", context);
    },
    flushManagedTabs() {
      return vm.runInContext("TwitchManagedTabs.saveManagedTabs()", context);
    },
    snapshotRaidSetting() {
      return storedRaidSetting;
    },
    snapshotManagedTabOwners() {
      return JSON.parse(JSON.stringify(storedTabOwners));
    },
    snapshotExistingTabAdoption() {
      return storedAdoptionEnabled;
    },
    snapshotRaidRedirectedChannelIds() {
      return JSON.parse(JSON.stringify(storedRaidRedirectedIds));
    },
    snapshotCompletedNativeCommandIds() {
      return JSON.parse(JSON.stringify(storedCompletedCommandIds));
    },
    managedRecordRouteMatches(record, url, surface) {
      return vm.runInContext(
        `TwitchManagedTabs.matchesManagedRecordRoute(${JSON.stringify(record)}, ${JSON.stringify(url)}, ${JSON.stringify(surface)})`,
        context,
      );
    },
    pendingRecordRouteMatches(record, url, surface) {
      return vm.runInContext(
        `TwitchManagedTabs.matchesPendingRoute(${JSON.stringify(record)}, ${JSON.stringify(url)}, ${JSON.stringify(surface)})`,
        context,
      );
    },
    async updateTabUrl(tabId, url) {
      const tab = tabsById.get(tabId);
      if (!tab) throw new Error("No such tab");
      tab.url = url;
      await Promise.all(onUpdated.dispatch(tabId, { url }, { ...tab }));
    },
    completeTab(tabId) {
      const tab = tabsById.get(tabId);
      if (!tab) throw new Error("No such tab");
      tab.status = "complete";
      return Promise.all(onUpdated.dispatch(tabId, {
        status: "complete",
        url: tab.url,
      }, { ...tab }));
    },
    reportManagedTabIdentity(tabId, identity) {
      const tab = tabsById.get(tabId);
      if (tab) tab.managedTabIdentity = identity;
      return new Promise((resolve) => {
        const results = chrome.runtime.onMessage.dispatch(
          { type: "managed_tab_identity_ready", identity },
          { tab: tab ? { id: tabId } : undefined },
          resolve,
        );
        if (!results.includes(true)) resolve(undefined);
      });
    },
    attachTabToWindow(tabId, windowId) {
      const tab = tabsById.get(tabId);
      if (!tab) throw new Error("No such tab");
      tab.windowId = windowId;
      return Promise.all(onAttached.dispatch(tabId, {
        newWindowId: windowId,
        newPosition: 0,
      }));
    },
    reportState(tabId, message) {
      return vm.runInContext(
        `forwardChannelState(${JSON.stringify(message)}, { tab: { id: ${tabId} }, frameId: 0 })`,
        context,
      );
    },
    discard(tabId) {
      const tab = tabsById.get(tabId);
      tab.discarded = true;
      tab.status = "complete";
    },
    waitForFirstWindow: () => firstWindowStarted,
    releaseFirstWindow,
    waitForFirstNavigation: () => firstNavigationStarted,
    releaseFirstNavigation,
    failNextManagedTabSave() {
      managedTabSaveFailures += 1;
    },
    failNextCompletedCommandSave() {
      completedCommandSaveFailures += 1;
    },
    pauseNextManagedTabSave({ failOnRelease = false } = {}) {
      let signalStarted;
      let release;
      const started = new Promise((resolve) => {
        signalStarted = resolve;
      });
      const wait = new Promise((resolve) => {
        release = resolve;
      });
      managedTabSavePause = { signalStarted, wait, failOnRelease };
      return { started, release };
    },
    pauseNextManagedTabIdentityRead() {
      let signalStarted;
      let release;
      const started = new Promise((resolve) => {
        signalStarted = resolve;
      });
      const wait = new Promise((resolve) => {
        release = resolve;
      });
      managedTabIdentityReadPause = { signalStarted, wait };
      return { started, release };
    },
    pauseNextRaidRedirectedSave() {
      let signalStarted;
      let release;
      const started = new Promise((resolve) => {
        signalStarted = resolve;
      });
      const wait = new Promise((resolve) => {
        release = resolve;
      });
      raidRedirectedSavePause = { signalStarted, wait };
      return { started, release };
    },
    pauseNextTabReload() {
      let signalStarted;
      let release;
      const started = new Promise((resolve) => {
        signalStarted = resolve;
      });
      const wait = new Promise((resolve) => {
        release = resolve;
      });
      reloadPause = { signalStarted, wait };
      return { started, release };
    },
    waitForNextBrowserMutation() {
      return new Promise((resolve) => browserMutationWaiters.push(resolve));
    },
    failRemoval(tabId) {
      failedRemovals.add(tabId);
    },
    allowRemoval(tabId) {
      failedRemovals.delete(tabId);
    },
  };
}

test("content script identity survives document reloads", () => {
  const source = fs.readFileSync(path.join(__dirname, "content.js"), "utf8");
  const values = new Map();
  const messages = [];
  const loadPage = () => {
    const runtimeMessages = event();
    const context = vm.createContext({
      chrome: {
        runtime: {
          onMessage: runtimeMessages,
          sendMessage(message) {
            messages.push(JSON.parse(JSON.stringify(message)));
            return Promise.resolve();
          },
        },
        storage: {
          local: { get: () => Promise.resolve({}) },
          onChanged: event(),
        },
      },
      document: {
        querySelectorAll: () => [],
        querySelector: () => null,
      },
      location: { href: "https://www.twitch.tv/streamer" },
      TwitchTabUtils: { channelRoute: () => ({ login: "streamer", surface: "player" }) },
      window: {
        sessionStorage: {
          getItem(key) {
            return values.get(key) ?? null;
          },
          setItem(key, value) {
            values.set(key, value);
          },
        },
        setInterval: () => 1,
        clearInterval() {},
        setTimeout: () => 1,
      },
    });
    vm.runInContext(qualitySource, context, { filename: "quality.js" });
    vm.runInContext(source, context, { filename: "content.js" });
    return runtimeMessages;
  };

  const firstPage = loadPage();
  assert.deepEqual(messages.at(-1), {
    type: "managed_tab_identity_ready",
    identity: null,
  });
  let setResponse;
  firstPage.dispatch(
    { type: "set_managed_tab_identity", identity: "managed-tab-token" },
    {},
    (response) => { setResponse = response; },
  );
  assert.deepEqual(JSON.parse(JSON.stringify(setResponse)), { identity: "managed-tab-token" });

  const restoredPage = loadPage();
  assert.deepEqual(messages.at(-1), {
    type: "managed_tab_identity_ready",
    identity: "managed-tab-token",
  });
  let getResponse;
  restoredPage.dispatch(
    { type: "get_managed_tab_identity" },
    {},
    (response) => { getResponse = response; },
  );
  assert.deepEqual(JSON.parse(JSON.stringify(getResponse)), { identity: "managed-tab-token" });
});
test("content script removes an identity copied to an opener tab", () => {
  const source = fs.readFileSync(path.join(__dirname, "content.js"), "utf8");
  const key = "__twitch_streak_saver_managed_tab_identity__";
  const values = new Map([[key, "copied-token"]]);
  const messages = [];
  const context = vm.createContext({
    chrome: {
      runtime: {
        onMessage: event(),
        sendMessage(message) {
          messages.push(JSON.parse(JSON.stringify(message)));
          return Promise.resolve();
        },
      },
      storage: {
        local: { get: () => Promise.resolve({}) },
        onChanged: event(),
      },
    },
    window: {
      opener: {},
      sessionStorage: {
        getItem(name) {
          return values.get(name) ?? null;
        },
        removeItem(name) {
          values.delete(name);
        },
      },
        setInterval: () => 1,
        clearInterval() {},
        setTimeout: () => 1,
    },
    document: {
      querySelectorAll: () => [],
      querySelector: () => null,
    },
    location: { href: "https://www.twitch.tv/streamer" },
    TwitchTabUtils: { channelRoute: () => ({ login: "streamer", surface: "player" }) },
  });
  vm.runInContext(qualitySource, context, { filename: "quality.js" });
  vm.runInContext(source, context, { filename: "content.js" });

  assert.equal(values.has(key), false);
  assert.deepEqual(messages.at(-1), {
    type: "managed_tab_identity_ready",
    identity: null,
  });
});
test("browser startup does not trust an opener-copied tab identity", async () => {
  const extension = await createExtension({
    storedManagedTabs: [
      ["channel-opener", {
        login: "streamer",
        layout: "combined_window",
        playerTabId: 53,
        chatTabId: 53,
        playerWindowId: 17,
        identityToken: "copied-tab-session",
        identityPersisted: true,
        identityPolicyVersion: 1,
      }],
    ],
    storedManagedTabOwners: [[53, "channel-opener"]],
    storedCloseOnRaidRedirect: true,
    browserStartup: true,
    initialTabs: [{
      id: 53,
      url: "https://www.twitch.tv/another?referrer=raid",
      windowId: 17,
      openerTabId: 54,
      managedTabIdentity: "copied-tab-session",
    }],
  });

  assert.equal(extension.tabsById.has(53), true);
  assert.equal(extension.snapshotManagedTabs()[0][1].pendingRaidOwnershipVerification, true);
  await extension.send({
    type: "sync_channels",
    close_on_raid_redirect: true,
    channels: [{ id: "channel-opener", login: "streamer" }],
  });
  await extension.reportManagedTabIdentity(53, "copied-tab-session");

  assert.equal(extension.tabsById.has(53), true);
  assert.deepEqual(Array.from(extension.snapshotRaidRedirectedChannelIds()), []);
});
test("startup quarantines an old token when opener metadata is gone", async () => {
  const extension = await createExtension({
    storedManagedTabs: [
      ["channel-old-policy", {
        login: "streamer",
        layout: "combined_window",
        playerTabId: 55,
        chatTabId: 55,
        playerWindowId: 17,
        identityToken: "old-tab-session",
        identityPersisted: true,
        contentVersion: "0.3.12",
      }],
    ],
    storedManagedTabOwners: [[55, "channel-old-policy"]],
    storedCloseOnRaidRedirect: true,
    browserStartup: true,
    initialTabs: [{
      id: 55,
      url: "https://www.twitch.tv/another?referrer=raid",
      windowId: 17,
      managedTabIdentity: "old-tab-session",
    }],
  });

  assert.equal(extension.tabsById.has(55), true);
  assert.equal(extension.snapshotManagedTabs()[0][1].pendingRaidOwnershipVerification, true);
  await extension.send({
    type: "sync_channels",
    close_on_raid_redirect: true,
    channels: [{ id: "channel-old-policy", login: "streamer" }],
  });
  await extension.reportManagedTabIdentity(55, "old-tab-session");

  assert.equal(extension.tabsById.has(55), true);
  assert.equal(extension.snapshotManagedTabs()[0][1].pendingRaidOwnershipVerification, true);
  assert.deepEqual(Array.from(extension.snapshotRaidRedirectedChannelIds()), []);
});
test("session storage failure does not block a managed stream opening", async () => {
  const extension = await createExtension({ failManagedIdentityWrites: true });
  await extension.open({
    type: "open_selected",
    channels: [{ id: "channel-storage", login: "streamer" }],
  });

  const streamTab = [...extension.tabsById.values()].find(
    (tab) => tab.url === "https://www.twitch.tv/streamer",
  );
  assert.ok(streamTab);
  assert.equal(extension.snapshotManagedTabs()[0][1].identityPersisted, false);
  assert.equal(extension.calls.nativeMessages.some(
    (message) => message.type === "tab_opened" && message.channel_id === "channel-storage",
  ), true);
  assert.equal(extension.calls.nativeMessages.some(
    (message) => message.type === "error" && message.channel_id === "channel-storage",
  ), false);
});
test("new managed tabs stay muted when their first persistence attempt fails", async () => {
  const extension = await createExtension();
  extension.failNextManagedTabSave();
  const command = {
    type: "open_selected",
    channels: [{ id: "channel-storage-retry", login: "streamer" }],
  };
  await extension.open(command);
  const [tab] = [...extension.tabsById.values()];
  assert.ok(tab);
  assert.equal(tab.muted, true);

  await extension.open(command);

  assert.equal(tab.url, "https://www.twitch.tv/streamer");
  assert.equal(tab.muted, true);
  const playerAudioMessage = extension.calls.contentMessages.find(
    (message) => message.type === "set_player_audio_preference",
  );
  assert.equal(playerAudioMessage?.tabId, tab.id);
  assert.equal(playerAudioMessage?.unmuted, false);
});
test("new managed tabs honor the per-channel unmuted setting", async () => {
  const extension = await createExtension();

  await extension.open({
    type: "open_selected",
    wake_browser: false,
    channels: [{ id: "channel-unmuted-preference", login: "streamer", unmuted: true }],
  });

  const [tab] = [...extension.tabsById.values()];
  assert.ok(tab);
  assert.equal(tab.muted, false);
  const playerAudioMessage = extension.calls.contentMessages.find(
    (message) => message.type === "set_player_audio_preference",
  );
  assert.equal(playerAudioMessage?.tabId, tab.id);
  assert.equal(playerAudioMessage?.unmuted, true);
});
test("managed player reload reapplies its saved audio preference", async () => {
  const extension = await createExtension();
  const command = {
    type: "open_selected",
    wake_browser: false,
    channels: [{ id: "channel-audio-reload", login: "streamer", unmuted: true }],
  };
  await extension.open(command);
  const [tab] = [...extension.tabsById.values()];
  const audioMessages = () => extension.calls.contentMessages.filter(
    (message) =>
      message.tabId === tab.id
      && message.type === "set_player_audio_preference",
  );
  assert.equal(audioMessages().length, 1);

  await extension.completeTab(tab.id);
  await extension.reportManagedTabIdentity(tab.id, tab.managedTabIdentity);
  await new Promise((resolve) => setImmediate(resolve));

  assert.equal(audioMessages().length, 2);
  assert.equal(audioMessages().at(-1)?.login, "streamer");
  assert.equal(audioMessages().at(-1)?.unmuted, true);
});
test("unpersisted identity preserves startup raid quarantine", async () => {
  const opened = await createExtension({ failManagedIdentityWrites: true });
  const sync = {
    type: "sync_channels",
    close_on_raid_redirect: true,
    channels: [{ id: "channel-storage", login: "streamer" }],
  };
  await opened.send(sync);
  await opened.open({
    type: "open_selected",
    channels: [{ id: "channel-storage", login: "streamer" }],
  });
  const streamTab = [...opened.tabsById.values()].find(
    (tab) => tab.url === "https://www.twitch.tv/streamer",
  );
  assert.ok(streamTab);

  const restarted = await createExtension({
    storedManagedTabs: opened.snapshotManagedTabs(),
    storedManagedTabOwners: opened.snapshotManagedTabOwners(),
    storedCloseOnRaidRedirect: true,
    browserStartup: true,
    initialTabs: [{
      id: streamTab.id,
      url: "https://www.twitch.tv/another?referrer=raid",
      windowId: streamTab.windowId,
    }],
  });
  await restarted.send(sync);
  await restarted.reportManagedTabIdentity(streamTab.id, null);

  assert.equal(restarted.tabsById.has(streamTab.id), true);
  assert.equal(restarted.snapshotManagedTabs()[0][1].pendingRaidOwnershipVerification, true);
  assert.equal(
    restarted.tabsById.get(streamTab.id).url,
    "https://www.twitch.tv/another?referrer=raid",
  );
});
test("manifest key derives the trusted Chrome extension ID", () => {
  const manifest = JSON.parse(
    fs.readFileSync(path.join(__dirname, "manifest.json"), "utf8"),
  );
  const publicKey = Buffer.from(manifest.key, "base64");
  const digest = crypto.createHash("sha256").update(publicKey).digest().subarray(0, 16);
  const extensionId = Array.from(
    digest,
    (byte) => String.fromCharCode(97 + (byte >> 4)) + String.fromCharCode(97 + (byte & 15)),
  ).join("");

  assert.equal(extensionId, trustedExtensionId);
});

test("pairs through native messaging without the localhost registration request", async () => {
  let registrationRequests = 0;
  const extension = await createExtension({
    fetchImpl: async () => {
      registrationRequests += 1;
      return { status: 403 };
    },
  });

  assert.equal(extension.calls.nativeConnections, 1);
  assert.equal(extension.isNativeConnected(), true);
  assert.equal(extension.calls.registrationRequests.length, 0);
  assert.equal(registrationRequests, 0);
  await extension.send({ type: "bridge_ready" });
  assert.equal(extension.status().bridgeReady, true);
});
test("coalesces concurrent native-host pairing checks", async () => {
  const extension = await createExtension({
    setTimeoutImpl: (callback, delay) => ({ callback, delay }),
    clearTimeoutImpl: () => {},
  });
  extension.disconnectHost();
  const connectionCount = extension.calls.nativeConnections;

  const results = await Promise.all([
    extension.synchronize(),
    extension.synchronize(),
  ]);

  assert.deepEqual(results, [true, true]);
  assert.equal(extension.calls.registrationRequests.length, 0);
  assert.equal(extension.calls.nativeConnections, connectionCount + 1);
  assert.equal(extension.isNativeConnected(), true);
});

test("native host quality settings reach Twitch content preferences", async () => {
  const extension = await createExtension();

  await extension.open({ type: "set_preferred_quality", preferred_quality: "720p" });
  assert.equal(extension.preferredQuality(), "720p");

  await extension.open({ type: "set_preferred_quality", preferred_quality: "invalid" });
  assert.equal(extension.preferredQuality(), "160p");
});
test("app theme sync updates the extension popup and defaults to dark", async () => {
  const freshExtension = await createExtension();
  const freshPopup = freshExtension.openPopup();
  assert.equal(await freshPopup.themeApplied(), "dark");

  const extension = await createExtension();
  for (const theme of ["system", "light", "dark", "amoled"]) {
    await extension.open({ type: "set_theme", theme });
    assert.equal(extension.theme(), theme);
  }
  const popup = extension.openPopup();
  assert.equal(await popup.themeApplied(), "amoled");

  await extension.open({ type: "set_theme", theme: "invalid" });
  assert.equal(extension.theme(), "dark");
});

test("toolbar popup wakes, restarts a stalled host, and follows the app handshake", async () => {
  const extension = await createExtension({
    fetchImpl: async () => ({ status: 403 }),
    setTimeoutImpl: setTimeout,
    clearTimeoutImpl: clearTimeout,
  });
  assert.equal(extension.calls.nativeConnections, 1);
  assert.equal(extension.calls.registrationRequests.length, 0);

  const popup = extension.openPopup();
  assert.equal(
    await popup.firstStatus,
    "Native host started; waiting for the app handshake.",
  );

  const restarting = popup.nextStatus();
  popup.wakeButton.click();
  assert.equal(await restarting, "Restarting the pending native-host connection…");
  assert.equal(
    await popup.nextStatus(),
    "Native host started; waiting for the app handshake.",
  );
  assert.equal(extension.calls.nativeConnections, 2);

  const connected = popup.nextStatus();
  await extension.send({ type: "bridge_ready" });
  assert.equal(await connected, "Connected to Twitch Streak Saver.");
  assert.equal(extension.calls.registrationRequests.length, 0);
  assert.equal(extension.calls.nativeConnections, 2);
});

test("opens each channel in its own active, unfocused combined Twitch window", async () => {
  const extension = await createExtension();
  await extension.open({
    type: "open_selected",
    wake_browser: false,
    channels: [
      { id: "channel-1", login: "streamer" },
      { id: "channel-2", login: "another_streamer" },
    ],
  });

  const tabs = [...extension.tabsById.values()];
  assert.equal(tabs.length, 2);
  for (const [login, tab] of [
    ["streamer", tabs.find((candidate) => candidate.url.endsWith("/streamer"))],
    ["another_streamer", tabs.find((candidate) => candidate.url.endsWith("/another_streamer"))],
  ]) {
    assert.equal(tab.url, `https://www.twitch.tv/${login}`);
    assert.equal(tab.status, "complete");
    assert.equal(tab.active, true);
    assert.equal(tab.autoDiscardable, false);
    assert.equal(tab.muted, true);
    assert.equal(
      [...extension.tabsById.values()].filter((candidate) => candidate.windowId === tab.windowId)
        .length,
      1,
    );
    assert.equal(extension.windowsById.get(tab.windowId).focused, false);
  }
  assert.deepEqual(extension.calls.windowCreates, [
    { url: "about:blank", focused: false, type: "normal" },
    { url: "about:blank", focused: false, type: "normal" },
  ]);
  assert.deepEqual(extension.calls.windowUpdates, []);
  assert.deepEqual(
    extension.calls.nativeMessages
      .filter((message) => message.type === "tab_opened")
      .map((message) => message.surface),
    ["player", "chat", "player", "chat"],
  );
});
test("keeps a user's unmuted state when reopening a managed channel", async () => {
  const extension = await createExtension();
  const command = {
    type: "open_selected",
    wake_browser: false,
    channels: [{ id: "channel-unmuted", login: "streamer" }],
  };
  await extension.open(command);
  const tab = [...extension.tabsById.values()][0];
  assert.equal(tab.muted, true);

  const audioPreferenceMessages = extension.calls.contentMessages
    .filter((message) => message.type === "set_player_audio_preference").length;
  tab.muted = false;
  await extension.open(command);

  assert.equal(tab.muted, false);
  assert.equal(
    extension.calls.contentMessages
      .filter((message) => message.type === "set_player_audio_preference").length,
    audioPreferenceMessages,
  );
});
test("updates managed player audio when the channel preference changes", async () => {
  const extension = await createExtension();
  await extension.open({
    type: "open_selected",
    wake_browser: false,
    channels: [{ id: "channel-audio-change", login: "streamer", unmuted: false }],
  });
  const [tab] = [...extension.tabsById.values()];
  assert.ok(tab);
  assert.equal(tab.muted, true);

  await extension.open({
    type: "open_selected",
    wake_browser: false,
    channels: [{ id: "channel-audio-change", login: "streamer", unmuted: true }],
  });

  assert.equal(tab.muted, false);
  const audioMessages = extension.calls.contentMessages.filter(
    (message) => message.type === "set_player_audio_preference",
  );
  assert.equal(audioMessages.length, 2);
  assert.equal(audioMessages.at(-1)?.unmuted, true);
});
test("opens subsequent stream windows while an earlier Twitch page is still loading", async () => {
  const extension = await createExtension({ pauseFirstNavigation: true });
  const opening = extension.open({
    type: "open_selected",
    wake_browser: false,
    channels: [
      { id: "channel-slow", login: "slow_streamer" },
      { id: "channel-fast", login: "fast_streamer" },
    ],
  });

  await extension.waitForFirstNavigation();
  const windowsOpenedWhileLoading = extension.calls.windowCreates.length;
  extension.releaseFirstNavigation();
  await opening;

  assert.equal(windowsOpenedWhileLoading, 2);
  assert.equal(extension.tabsById.size, 2);
  assert.deepEqual(
    [...extension.tabsById.values()].map((tab) => tab.url).sort(),
    [
      "https://www.twitch.tv/fast_streamer",
      "https://www.twitch.tv/slow_streamer",
    ],
  );
});

test("combined Twitch page reports both player and chat readiness from one active tab", async () => {
  const extension = await createExtension();
  await extension.open({
    type: "open_selected",
    wake_browser: false,
    channels: [{ id: "channel-1", login: "streamer" }],
  });
  await extension.send({
    type: "sync_channels",
    channels: [{ id: "channel-1", login: "streamer" }],
    viewer_login: "viewer",
  });
  const [tab] = [...extension.tabsById.values()];
  assert.equal(
    extension.calls.contentMessages
      .filter((message) => message.tabId === tab.id && message.type === "start_monitoring")
      .at(-1)?.viewer_login,
    "viewer",
  );

  extension.reportState(tab.id, {
    surface: "player",
    login: "streamer",
    player_state: "playing",
    chat_state: "ready",
  });

  extension.reportState(tab.id, {
    surface: "player",
    login: "streamer",
    player_state: "playing",
    chat_state: "ready",
    chat_message_author_login: "viewer",
  });
  assert.deepEqual(
    JSON.parse(JSON.stringify(extension.calls.nativeMessages.filter(
      (message) => message.type === "channel_status",
    ))),
    [
      {
        type: "channel_status",
        channel_id: "channel-1",
        tab_id: tab.id,
        surface: "player",
        player_state: "playing",
        chat_state: "ready",
      },
      {
        type: "channel_status",
        channel_id: "channel-1",
        tab_id: tab.id,
        surface: "player",
        player_state: "playing",
        chat_state: "ready",
        chat_message_author_login: "viewer",
      },
    ],
  );
});

test("wake mode focuses the dedicated window and waits for the combined page to load", async () => {
  const extension = await createExtension();
  await extension.open({
    type: "open_selected",
    wake_browser: true,
    channels: [{ id: "channel-2", login: "streamer" }],
  });

  const [tab] = [...extension.tabsById.values()];
  assert.equal(tab.url, "https://www.twitch.tv/streamer");
  assert.equal(tab.status, "complete");
  assert.equal(tab.active, true);
  assert.equal(extension.windowsById.get(tab.windowId).focused, true);
  assert.deepEqual(JSON.parse(JSON.stringify(extension.calls.windowUpdates)), [
    { windowId: tab.windowId, options: { state: "normal", focused: true } },
  ]);
});
test("wake mode leaves the last requested stream window focused", async () => {
  const extension = await createExtension({ pauseFirstWindow: true });
  const opening = extension.open({
    type: "open_selected",
    wake_browser: true,
    channels: [
      { id: "channel-first", login: "first_streamer" },
      { id: "channel-last", login: "last_streamer" },
    ],
  });
  await extension.waitForFirstWindow();
  extension.releaseFirstWindow();
  await opening;

  const lastTab = [...extension.tabsById.values()]
    .find((tab) => tab.url.endsWith("/last_streamer"));
  assert.equal(extension.windowsById.get(lastTab.windowId).focused, true);
  assert.equal(
    [...extension.tabsById.values()]
      .some((tab) => extension.windowsById.get(tab.windowId).focused && tab.id !== lastTab.id),
    false,
  );
});

test("reopening a discarded managed tab reloads it before monitoring resumes", async () => {
  const extension = await createExtension();
  const command = {
    type: "open_selected",
    wake_browser: false,
    channels: [{ id: "channel-3", login: "streamer" }],
  };
  await extension.open(command);
  const [tab] = [...extension.tabsById.values()];
  const priorMonitoringCalls = extension.calls.contentMessages.length;
  extension.discard(tab.id);

  await extension.open(command);

  const restored = extension.tabsById.get(tab.id);
  const resumedMonitoring = extension.calls.contentMessages
    .slice(priorMonitoringCalls)
    .filter((message) => message.type === "start_monitoring");
  assert.equal(extension.tabsById.size, 1);
  assert.equal(restored.status, "complete");
  assert.equal(restored.discarded, false);
  assert.equal(restored.active, true);
  assert.equal(restored.autoDiscardable, false);
  assert.ok(resumedMonitoring.some(
    (message) => message.status === "complete" && message.discarded === false,
  ));
  assert.deepEqual(extension.calls.windowUpdates, []);
});

test("legacy regular pages are moved into separate windows without moving user tabs", async () => {
  const extension = await createExtension({
    storedManagedTabs: [
      ["channel-1", {
        login: "streamer",
        layout: "regular_website",
        playerTabId: 11,
        chatTabId: 11,
      }],
      ["channel-2", {
        login: "another_streamer",
        layout: "regular_website",
        playerTabId: 12,
        chatTabId: 12,
      }],
    ],
    initialTabs: [
      { id: 11, url: "https://www.twitch.tv/streamer", windowId: 17, active: true },
      { id: 12, url: "https://www.twitch.tv/another_streamer", windowId: 17 },
      { id: 13, url: "https://example.com/", windowId: 17 },
    ],
  });

  await extension.send({
    type: "sync_channels",
    channels: [
      { id: "channel-1", login: "streamer" },
      { id: "channel-2", login: "another_streamer" },
    ],
  });

  const first = extension.tabsById.get(11);
  const second = extension.tabsById.get(12);
  assert.notEqual(first.windowId, second.windowId);
  assert.equal(first.active, true);
  assert.equal(second.active, true);
  assert.equal(extension.tabsById.get(13).windowId, 17);
  assert.equal(extension.tabsById.has(13), true);
  assert.equal(extension.tabsById.size, 3);
  assert.deepEqual(extension.calls.windowUpdates, []);
});
test("enabled raid redirects close managed streams and survive worker restarts", async () => {
  const extension = await createExtension({
    storedManagedTabs: [
      ["channel-raid", {
        login: "streamer",
        layout: "combined_window",
        playerTabId: 21,
        chatTabId: 21,
        contentVersion: "0.3.13",
      }],
    ],
    initialTabs: [
      { id: 21, url: "https://www.twitch.tv/streamer", active: true },
      { id: 22, url: "https://www.twitch.tv/external" },
    ],
  });
  await extension.send({
    type: "sync_channels",
    channels: [{ id: "channel-raid", login: "streamer" }],
    close_on_raid_redirect: true,
  });

  await extension.updateTabUrl(22, "https://www.twitch.tv/external?referrer=raid");
  await extension.updateTabUrl(21, "https://www.twitch.tv/streamer?referrer=not-raid");
  assert.equal(extension.tabsById.has(21), true);
  assert.equal(extension.tabsById.has(22), true);

  const restarted = await createExtension({
    storedManagedTabs: extension.snapshotManagedTabs(),
    storedCloseOnRaidRedirect: extension.snapshotRaidSetting(),
    storedManagedTabOwners: extension.snapshotManagedTabOwners(),
    initialTabs: [
      { id: 21, url: "https://www.twitch.tv/streamer?referrer=not-raid", active: true },
      { id: 22, url: "https://www.twitch.tv/external?referrer=raid" },
    ],
  });
  await restarted.updateTabUrl(
    21,
    "https://www.twitch.tv/streamer?source=raid&referrer=raid",
  );

  assert.equal(restarted.tabsById.has(21), false);
  assert.equal(restarted.tabsById.has(22), true);
  assert.ok(restarted.calls.nativeMessages.some(
    (message) => message.type === "tab_closed"
      && message.channel_id === "channel-raid"
      && message.surface === "player"
      && message.close_reason === "raid_redirect",
  ));
});

test("worker restoration closes a managed tab already redirected by a raid", async () => {
  const extension = await createExtension({
    storedManagedTabs: [
      ["channel-raid", {
        login: "streamer",
        layout: "combined_window",
        playerTabId: 41,
        chatTabId: 41,
        contentVersion: "0.3.13",
      }],
    ],
    storedCloseOnRaidRedirect: true,
    storedManagedTabOwners: [[41, "channel-raid"]],
    initialTabs: [
      { id: 41, url: "https://www.twitch.tv/another?source=raid&referrer=raid" },
    ],
  });

  assert.equal(extension.tabsById.has(41), false);
  assert.deepEqual(extension.snapshotRaidRedirectedChannelIds(), ["channel-raid"]);
  await extension.reportExternalStreams();
  assert.ok(extension.calls.nativeMessages.some(
    (message) => message.type === "external_streams"
      && message.raid_redirected_channel_ids.includes("channel-raid"),
  ));
});

test("managed redirect closes after a Twitch route change", async () => {
  const extension = await createExtension({
    storedManagedTabs: [
      ["channel-raid", {
        login: "streamer",
        layout: "combined_window",
        playerTabId: 71,
        chatTabId: 71,
        contentVersion: "0.3.13",
      }],
    ],
    storedCloseOnRaidRedirect: true,
    initialTabs: [{ id: 71, url: "https://www.twitch.tv/streamer" }],
  });

  await extension.updateTabUrl(71, "https://www.twitch.tv/another?referrer=raid");

  assert.equal(extension.tabsById.has(71), false);
  assert.deepEqual(extension.snapshotRaidRedirectedChannelIds(), ["channel-raid"]);
});

test("extension reload closes a raid redirect in the recorded managed window", async () => {
  const extension = await createExtension({
    storedManagedTabs: [
      ["channel-raid", {
        login: "streamer",
        layout: "combined_window",
        playerTabId: 43,
        chatTabId: 43,
        playerWindowId: 17,
        contentVersion: "0.3.13",
      }],
    ],
    storedCloseOnRaidRedirect: true,
    initialTabs: [{ id: 43, url: "https://www.twitch.tv/another?referrer=raid" }],
  });

  assert.equal(extension.tabsById.has(43), false);
  assert.deepEqual(extension.snapshotRaidRedirectedChannelIds(), ["channel-raid"]);
});

test("moved managed tabs retain verifiable ownership after extension reload", async () => {
  const extension = await createExtension({
    storedManagedTabs: [
      ["channel-raid", {
        login: "streamer",
        layout: "combined_window",
        playerTabId: 44,
        chatTabId: 44,
        playerWindowId: 17,
        contentVersion: "0.3.13",
      }],
    ],
    storedCloseOnRaidRedirect: true,
    initialTabs: [{ id: 44, url: "https://www.twitch.tv/streamer", windowId: 17 }],
  });

  await extension.attachTabToWindow(44, 18);
  assert.equal(extension.snapshotManagedTabs()[0][1].playerWindowId, 18);

  const restarted = await createExtension({
    storedManagedTabs: extension.snapshotManagedTabs(),
    storedCloseOnRaidRedirect: true,
    initialTabs: [{ id: 44, url: "https://www.twitch.tv/another?referrer=raid", windowId: 18 }],
  });

  assert.equal(restarted.tabsById.has(44), false);
});

test("raid redirect markers survive native disconnects and extension restarts", async () => {
  const extension = await createExtension({
    storedManagedTabs: [
      ["channel-raid", {
        login: "streamer",
        layout: "combined_window",
        playerTabId: 51,
        chatTabId: 51,
        contentVersion: "0.3.13",
      }],
    ],
    initialTabs: [{ id: 51, url: "https://www.twitch.tv/streamer" }],
  });
  await extension.open({
    type: "sync_channels",
    channels: [{ id: "channel-raid", login: "streamer" }],
    close_on_raid_redirect: true,
  });
  extension.disconnectHost();
  await extension.updateTabUrl(51, "https://www.twitch.tv/streamer?referrer=raid");

  assert.equal(extension.tabsById.has(51), false);
  assert.deepEqual(extension.snapshotRaidRedirectedChannelIds(), ["channel-raid"]);
  extension.reconnectHost();
  await extension.reportExternalStreams();
  assert.ok(extension.calls.nativeMessages.some(
    (message) => message.type === "external_streams"
      && message.raid_redirected_channel_ids.includes("channel-raid"),
  ));

  const restarted = await createExtension({
    storedManagedTabs: extension.snapshotManagedTabs(),
    storedCloseOnRaidRedirect: extension.snapshotRaidSetting(),
    storedRaidRedirectedChannelIds: extension.snapshotRaidRedirectedChannelIds(),
  });
  await restarted.reportExternalStreams();
  assert.ok(restarted.calls.nativeMessages.some(
    (message) => message.type === "external_streams"
      && message.raid_redirected_channel_ids.includes("channel-raid"),
  ));
});

test("failed raid tab removals retain suppression and identify the close failure", async () => {
  const extension = await createExtension({
    storedManagedTabs: [
      ["channel-raid", {
        login: "streamer",
        layout: "combined_window",
        playerTabId: 61,
        chatTabId: 61,
        contentVersion: "0.3.13",
      }],
    ],
    storedCloseOnRaidRedirect: true,
    initialTabs: [{ id: 61, url: "https://www.twitch.tv/streamer" }],
  });
  extension.failRemoval(61);
  await extension.updateTabUrl(61, "https://www.twitch.tv/streamer?referrer=raid");

  assert.equal(extension.tabsById.has(61), true);
  assert.deepEqual(extension.snapshotRaidRedirectedChannelIds(), ["channel-raid"]);
  assert.ok(extension.calls.nativeMessages.some(
    (message) => message.type === "error"
      && message.channel_id === "channel-raid"
      && message.close_reason === "raid_redirect",
  ));
});

test("offline confirmation clears persisted raid suppression", async () => {
  const extension = await createExtension({
    storedRaidRedirectedChannelIds: ["channel-raid"],
  });
  await extension.open({
    type: "clear_raid_redirected_channels",
    channel_ids: ["channel-raid"],
  });
  assert.deepEqual(Array.from(extension.snapshotRaidRedirectedChannelIds()), []);

  await extension.reportExternalStreams();
  const inventory = extension.calls.nativeMessages.findLast(
    (message) => message.type === "external_streams",
  );
  assert.deepEqual(Array.from(inventory.raid_redirected_channel_ids), []);
});
test("raid redirects leave managed stream tabs open when the setting is disabled", async () => {
  const extension = await createExtension({
    storedManagedTabs: [
      ["channel-raid", {
        login: "streamer",
        layout: "combined_window",
        playerTabId: 31,
        chatTabId: 31,
        contentVersion: "0.3.13",
      }],
    ],
    initialTabs: [
      { id: 31, url: "https://www.twitch.tv/streamer", active: true },
    ],
  });
  await extension.send({
    type: "sync_channels",
    channels: [{ id: "channel-raid", login: "streamer" }],
    close_on_raid_redirect: false,
  });

  await extension.updateTabUrl(31, "https://www.twitch.tv/streamer?referrer=raid");

  assert.equal(extension.tabsById.has(31), true);
  assert.equal(extension.snapshotManagedTabs().length, 1);
  assert.equal(extension.snapshotRaidSetting(), false);
});


test("legacy split tabs migrate as one combined tab and preserve failed removals for retry", async () => {
  const extension = await createExtension({
    storedManagedTabs: [
      ["channel-failure", {
        login: "streamer",
        layout: "separate_player_chat",
        playerTabId: 21,
        chatTabId: 22,
      }],
    ],
    initialTabs: [
      {
        id: 21,
        url: "https://player.twitch.tv/?channel=streamer&parent=twitch.tv",
        windowId: 17,
      },
      { id: 22, url: "https://www.twitch.tv/popout/streamer/chat", windowId: 17 },
      { id: 23, url: "https://example.com/", windowId: 17 },
    ],
  });
  extension.failRemoval(21);
  const command = {
    type: "open_selected",
    wake_browser: false,
    channels: [{ id: "channel-failure", login: "streamer" }],
  };

  await extension.open(command);

  assert.equal(extension.tabsById.has(21), true);
  assert.equal(extension.tabsById.has(22), false);
  assert.equal([...extension.tabsById.values()].some(
    (tab) => tab.url === "https://www.twitch.tv/streamer",
  ), false);

  extension.allowRemoval(21);
  await extension.open(command);

  const managedTabs = [...extension.tabsById.values()]
    .filter((tab) => tab.url === "https://www.twitch.tv/streamer");
  assert.equal(managedTabs.length, 1);
  assert.equal(managedTabs[0].active, true);
  assert.equal(managedTabs[0].autoDiscardable, false);
  assert.equal(extension.tabsById.has(23), true);
  assert.equal(extension.tabsById.size, 2);
});

test("queued repeated opens reuse the same dedicated tab", async () => {
  const extension = await createExtension({ pauseFirstWindow: true });
  const command = {
    type: "open_selected",
    wake_browser: false,
    channels: [{ id: "channel-race", login: "streamer" }],
  };
  const opening = extension.send(command);
  await extension.waitForFirstWindow();
  const repeated = extension.send(command);
  extension.releaseFirstWindow();
  await opening;
  await repeated;

  assert.equal(extension.tabsById.size, 1);
  assert.equal([...extension.tabsById.values()][0].active, true);
  assert.equal(extension.windowsById.size, 2);
});
test("native host disconnect retries its connection after five seconds", async () => {
  const scheduledTimers = [];
  const clearedTimers = new Set();
  const extension = await createExtension({
    setTimeoutImpl: (callback, delay) => {
      const timer = { callback, delay };
      scheduledTimers.push(timer);
      return timer;
    },
    clearTimeoutImpl: (timer) => clearedTimers.add(timer),
  });
  extension.disconnectHost({ message: "Native host has exited" });

  assert.equal(extension.lastErrorReads(), 1);
  assert.equal(extension.isNativeConnected(), false);
  assert.ok(extension.calls.logs.some(
    (entry) => entry.level === "debug" && entry.message.includes("Native host has exited"),
  ));
  const retryTimer = scheduledTimers.find(
    (timer) => !clearedTimers.has(timer) && timer.delay === 5000,
  );
  assert.ok(retryTimer);
  assert.equal(await retryTimer.callback(), true);
  assert.equal(extension.calls.registrationRequests.length, 0);
  assert.equal(extension.calls.nativeConnections, 2);
  assert.equal(extension.isNativeConnected(), true);
});
test("sync after extension reload restores persisted managed tabs", async () => {
  const original = await createExtension();
  await original.open({
    type: "open_selected",
    wake_browser: false,
    channels: [{ id: "channel-reload", login: "streamer" }],
  });
  const [originalTab] = [...original.tabsById.values()];
  const restarted = await createExtension({
    storedManagedTabs: original.snapshotManagedTabs(),
    initialTabs: [originalTab],
  });

  await restarted.send({
    type: "sync_channels",
    channels: [{ id: "channel-reload", login: "streamer" }],
  });

  const [restoredTab] = [...restarted.tabsById.values()];
  assert.equal(restoredTab.id, originalTab.id);
  assert.equal(restoredTab.url, "https://www.twitch.tv/streamer");
  assert.equal(restoredTab.active, true);
  assert.equal(restoredTab.autoDiscardable, false);
  assert.deepEqual(restarted.calls.windowCreates, []);
  assert.deepEqual(restarted.calls.windowUpdates, []);
  await restarted.send({ type: "close_all" });
  assert.equal(restarted.tabsById.size, 0);
  assert.deepEqual(restarted.snapshotManagedTabs(), []);
});
test("external inventory reports restored app-managed player channels separately", async () => {
  const extension = await createExtension({
    storedManagedTabs: [
      ["deselected-channel", {
        login: "deselected",
        layout: "combined_window",
        playerTabId: 11,
        chatTabId: 11,
        contentVersion: "0.3.4",
      }],
    ],
    initialTabs: [
      { id: 11, url: "https://www.twitch.tv/deselected" },
      { id: 12, url: "https://www.twitch.tv/external" },
    ],
  });

  await extension.reportExternalStreams();

  const [inventory] = extension.calls.nativeMessages
    .filter((message) => message.type === "external_streams");
  assert.deepEqual(JSON.parse(JSON.stringify(inventory)), {
    type: "external_streams",
    channel_counts: {},
    open_streamer_logins: ["deselected", "external"],
    unmatched_count: 1,
    managed_channel_ids: ["deselected-channel"],
    raid_redirected_channel_ids: [],
  });
});
test("sync reloads managed pages when the extension version changes", async () => {
  const original = await createExtension({ extensionVersion: "0.3.2" });
  await original.open({
    type: "open_selected",
    wake_browser: false,
    channels: [{ id: "channel-update", login: "streamer", unmuted: true }],
  });
  const [originalTab] = [...original.tabsById.values()];
  const restarted = await createExtension({
    extensionVersion: "0.3.4",
    storedManagedTabs: original.snapshotManagedTabs(),
    initialTabs: [originalTab],
  });

  await restarted.send({
    type: "sync_channels",
    channels: [{ id: "channel-update", login: "streamer", unmuted: true }],
  });

  assert.equal(restarted.tabsById.get(originalTab.id).status, "complete");
  assert.equal(
    restarted.calls.tabUpdates.filter(
      ({ changes }) => changes.url === "https://www.twitch.tv/streamer",
    ).length,
    1,
  );
  assert.equal(restarted.snapshotManagedTabs()[0][1].contentVersion, "0.3.4");
  const playerAudioMessage = restarted.calls.contentMessages.find(
    (message) => message.type === "set_player_audio_preference",
  );
  assert.equal(playerAudioMessage?.tabId, originalTab.id);
  assert.equal(playerAudioMessage?.unmuted, true);
  assert.deepEqual(restarted.calls.windowUpdates, []);
});

test("restoration does not close a reused external tab ID on a raid URL", async () => {
  const extension = await createExtension({
    storedManagedTabs: [
      ["channel-reused", {
        login: "streamer",
        layout: "combined_window",
        playerTabId: 41,
        chatTabId: 41,
      }],
    ],
    storedCloseOnRaidRedirect: true,
    initialTabs: [{ id: 41, url: "https://example.com/?referrer=raid" }],
  });

  await extension.send({
    type: "sync_channels",
    channels: [{ id: "channel-reused", login: "streamer" }],
    close_on_raid_redirect: true,
  });

  assert.equal(extension.tabsById.size, 1);
  assert.equal(extension.tabsById.get(41).url, "https://example.com/?referrer=raid");
  assert.deepEqual(extension.calls.windowCreates, []);
  assert.deepEqual(extension.calls.windowUpdates, []);
});
test("restoration does not close a reused Twitch tab ID on a different raid route", async () => {
  const extension = await createExtension({
    storedManagedTabs: [
      ["channel-reused", {
        login: "streamer",
        layout: "combined_window",
        playerTabId: 42,
        chatTabId: 42,
        playerWindowId: 17,
      }],
    ],
    storedCloseOnRaidRedirect: true,
    initialTabs: [{ id: 42, url: "https://www.twitch.tv/another?referrer=raid", windowId: 18 }],
  });

  assert.equal(extension.tabsById.has(42), true);
  assert.deepEqual(Array.from(extension.snapshotRaidRedirectedChannelIds()), []);
});
test("browser startup rejects a reused tab and window without its managed identity", async () => {
  const extension = await createExtension({
    storedManagedTabs: [
      ["channel-reused", {
        login: "streamer",
        layout: "combined_window",
        playerTabId: 42,
        chatTabId: 42,
        playerWindowId: 17,
        identityToken: "prior-tab-session",
        identityPolicyVersion: 1,
      }],
    ],
    storedManagedTabOwners: [[42, "channel-reused"]],
    storedCloseOnRaidRedirect: true,
    browserStartup: true,
    initialTabs: [{
      id: 42,
      url: "https://www.twitch.tv/another?referrer=raid",
      windowId: 17,
      managedTabIdentity: "different-tab-session",
    }],
  });

  assert.equal(extension.tabsById.has(42), true);
  assert.deepEqual(Array.from(extension.snapshotRaidRedirectedChannelIds()), []);
  assert.deepEqual(Array.from(extension.snapshotManagedTabOwners()), []);
});
test("browser startup closes a restored managed raid tab by its session identity", async () => {
  const extension = await createExtension({
    storedManagedTabs: [
      ["channel-restored", {
        login: "streamer",
        layout: "combined_window",
        playerTabId: 52,
        chatTabId: 52,
        identityToken: "restored-tab-session",
        identityPolicyVersion: 1,
      }],
    ],
    storedCloseOnRaidRedirect: true,
    browserStartup: true,
    initialTabs: [{
      id: 52,
      url: "https://www.twitch.tv/another?referrer=raid",
      windowId: 17,
      managedTabIdentity: "restored-tab-session",
    }],
  });

  assert.equal(extension.tabsById.has(52), false);
  assert.deepEqual(Array.from(extension.snapshotRaidRedirectedChannelIds()), ["channel-restored"]);
});
test("legacy raid records quarantine unverified IDs across startup and worker restore", async () => {
  const extension = await createExtension({
    storedManagedTabs: [
      ["channel-legacy", {
        login: "streamer",
        layout: "combined_window",
        playerTabId: 92,
        chatTabId: 92,
        playerWindowId: 17,
      }],
    ],
    storedManagedTabOwners: [[92, "channel-legacy"]],
    storedCloseOnRaidRedirect: true,
    browserStartup: true,
    initialTabs: [{
      id: 92,
      url: "https://www.twitch.tv/another?referrer=raid",
      windowId: 17,
      contentScriptReady: false,
    }],
  });
  const sync = {
    type: "sync_channels",
    close_on_raid_redirect: true,
    channels: [{ id: "channel-legacy", login: "streamer" }],
  };
  assert.equal(extension.tabsById.has(92), true);
  assert.equal(extension.snapshotManagedTabs()[0][1].pendingRaidOwnershipVerification, true);
  await extension.send(sync);
  assert.equal(
    extension.tabsById.get(92).url,
    "https://www.twitch.tv/another?referrer=raid",
  );

  const restarted = await createExtension({
    storedManagedTabs: extension.snapshotManagedTabs(),
    storedManagedTabOwners: extension.snapshotManagedTabOwners(),
    storedCloseOnRaidRedirect: true,
    initialTabs: [{
      id: 92,
      url: "https://www.twitch.tv/another?referrer=raid",
      windowId: 17,
      contentScriptReady: false,
    }],
  });
  await restarted.send(sync);
  await restarted.reportManagedTabIdentity(92, null);

  assert.equal(restarted.tabsById.has(92), true);
  assert.equal(restarted.snapshotManagedTabs()[0][1].pendingRaidOwnershipVerification, true);
  await restarted.updateTabUrl(92, "https://example.com/");
  assert.equal(restarted.tabsById.has(92), true);
  assert.deepEqual(restarted.snapshotManagedTabs(), []);
});
test("startup identity checks release tabs that leave Twitch", async () => {
  const extension = await createExtension({
    storedManagedTabs: [
      ["channel-left", {
        login: "streamer",
        layout: "combined_window",
        playerTabId: 72,
        chatTabId: 72,
        identityToken: "left-tab-session",
        identityPolicyVersion: 1,
      }],
    ],
    storedCloseOnRaidRedirect: true,
    browserStartup: true,
    initialTabs: [{
      id: 72,
      url: "https://www.twitch.tv/another?referrer=raid",
      windowId: 17,
      contentScriptReady: false,
    }],
  });

  await extension.updateTabUrl(72, "https://example.com/");

  assert.equal(extension.tabsById.get(72).url, "https://example.com/");
  assert.deepEqual(extension.snapshotManagedTabs(), []);
  assert.equal(extension.calls.nativeMessages.some((message) => message.type === "tab_closed"), false);
  await extension.open({
    type: "open_selected",
    channels: [{ id: "channel-left", login: "streamer" }],
  });
  assert.equal(
    [...extension.tabsById.values()].some((tab) => tab.url === "https://www.twitch.tv/streamer"),
    true,
  );
});
test("browser startup waits for a restored tab's identity before deciding ownership", async () => {
  const extension = await createExtension({
    storedManagedTabs: [
      ["channel-delayed", {
        login: "streamer",
        layout: "combined_window",
        playerTabId: 62,
        chatTabId: 62,
        identityToken: "delayed-tab-session",
        identityPolicyVersion: 1,
      }],
    ],
    storedCloseOnRaidRedirect: true,
    browserStartup: true,
    initialTabs: [{
      id: 62,
      url: "https://www.twitch.tv/another?referrer=raid",
      windowId: 17,
      contentScriptReady: false,
    }],
  });

  await extension.send({
    type: "sync_channels",
    close_on_raid_redirect: true,
    channels: [{ id: "channel-delayed", login: "streamer" }],
  });
  await extension.updateTabUrl(62, "https://www.twitch.tv/another?referrer=raid");
  assert.equal(extension.snapshotManagedTabs()[0][1].playerTabId, 62);
  assert.equal(extension.calls.nativeMessages.some((message) => message.type === "tab_closed"), false);
  assert.equal(
    extension.tabsById.get(62).url,
    "https://www.twitch.tv/another?referrer=raid",
  );
  assert.equal(extension.tabsById.has(62), true);
  assert.equal(extension.snapshotManagedTabs()[0][1].playerTabId, 62);
  await extension.reportManagedTabIdentity(62, "delayed-tab-session");

  assert.equal(extension.tabsById.has(62), false);
  assert.deepEqual(Array.from(extension.snapshotRaidRedirectedChannelIds()), ["channel-delayed"]);
});
test("startup identity rejection refreshes external inventory", async () => {
  const scheduled = [];
  const extension = await createExtension({
    storedManagedTabs: [
      ["channel-stale", {
        login: "streamer",
        layout: "combined_window",
        playerTabId: 82,
        chatTabId: 82,
        identityToken: "expected-tab-session",
        identityPolicyVersion: 1,
      }],
    ],
    storedCloseOnRaidRedirect: true,
    browserStartup: true,
    initialTabs: [{
      id: 82,
      url: "https://www.twitch.tv/another?referrer=raid",
      windowId: 17,
      contentScriptReady: false,
    }],
    setTimeoutImpl(callback, delay) {
      const timer = { callback, delay, cancelled: false };
      scheduled.push(timer);
      return timer;
    },
    clearTimeoutImpl(timer) {
      if (timer) timer.cancelled = true;
    },
  });
  await extension.send({
    type: "sync_channels",
    close_on_raid_redirect: true,
    channels: [{ id: "channel-stale", login: "streamer" }],
  });

  async function flushInventory() {
    const timer = scheduled.find((candidate) => !candidate.cancelled && candidate.delay === 100);
    assert.ok(timer);
    timer.cancelled = true;
    timer.callback();
    await new Promise((resolve) => setImmediate(resolve));
  }
  await flushInventory();
  const priorReportCount = extension.calls.nativeMessages
    .filter((message) => message.type === "external_streams").length;
  assert.equal(priorReportCount, 1);
  await extension.reportManagedTabIdentity(82, "different-tab-session");
  await flushInventory();

  const reports = extension.calls.nativeMessages
    .filter((message) => message.type === "external_streams");
  assert.equal(reports.length, priorReportCount + 1);
  assert.deepEqual(JSON.parse(JSON.stringify(reports.at(-1).managed_channel_ids)), []);
});
test("sync resumes a saved blank tab after an interrupted navigation", async () => {
  const extension = await createExtension({
    storedManagedTabs: [
      ["channel-pending", {
        login: "streamer",
        layout: "combined_window",
        playerTabId: 44,
        chatTabId: 44,
        pendingUrl: "https://www.twitch.tv/streamer",
        contentVersion: "0.3.4",
      }],
    ],
    initialTabs: [{ id: 44, url: "about:blank" }],
  });

  await extension.send({
    type: "sync_channels",
    channels: [{ id: "channel-pending", login: "streamer" }],
  });

  const tab = extension.tabsById.get(44);
  assert.equal(tab.url, "https://www.twitch.tv/streamer");
  assert.equal(tab.status, "complete");
  assert.equal(tab.active, true);
  assert.equal(extension.snapshotManagedTabs()[0][1].pendingUrl, undefined);
});
test("interrupted login rename survives reconnect without restarting stale monitoring", async () => {
  const extension = await createExtension({
    pauseFirstNavigation: true,
    storedManagedTabs: [
      ["channel-rename", {
        login: "oldname",
        layout: "combined_window",
        playerTabId: 51,
        chatTabId: 51,
        contentVersion: "0.3.4",
      }],
    ],
    initialTabs: [{ id: 51, url: "https://www.twitch.tv/oldname", active: true }],
  });
  const channels = [{ id: "channel-rename", login: "newname" }];
  const syncing = extension.send({ type: "sync_channels", channels });
  await extension.waitForFirstNavigation();

  const pendingRecord = extension.snapshotManagedTabs()[0][1];
  assert.equal(pendingRecord.login, "newname");
  assert.deepEqual(pendingRecord.previousLogins, ["oldname"]);
  assert.equal(pendingRecord.pendingUrl, "https://www.twitch.tv/newname");
  const pendingManagedTabs = extension.snapshotManagedTabs();
  const priorMonitoringStarts = extension.calls.contentMessages
    .filter((message) => message.type === "start_monitoring").length;
  extension.disconnectHost({ message: "Native host has exited" });
  extension.releaseFirstNavigation();
  await syncing;
  extension.cancelReconnect();

  assert.equal(extension.calls.contentMessages
    .filter((message) => message.type === "start_monitoring").length, priorMonitoringStarts);

  const restarted = await createExtension({
    storedManagedTabs: pendingManagedTabs,
    initialTabs: [{ id: 51, url: "https://www.twitch.tv/oldname", active: true }],
  });
  await restarted.send({ type: "sync_channels", channels });

  assert.equal(restarted.tabsById.get(51).url, "https://www.twitch.tv/newname");
  assert.equal(restarted.tabsById.get(51).status, "complete");
  assert.equal(restarted.snapshotManagedTabs()[0][1].previousLogins, undefined);
  assert.equal(restarted.snapshotManagedTabs()[0][1].pendingUrl, undefined);
});
test("status forwarding waits for the replacement native port to sync", async () => {
  const extension = await createExtension();
  const channels = [{ id: "channel-port", login: "streamer" }];
  await extension.send({ type: "sync_channels", channels });
  await extension.open({
    type: "open_selected",
    wake_browser: false,
    channels,
  });
  const [tab] = [...extension.tabsById.values()];
  const state = {
    surface: "player",
    login: "streamer",
    player_state: "playing",
    chat_state: "ready",
  };
  const report = () => extension.reportState(tab.id, state);
  const statusCount = () => extension.calls.nativeMessages
    .filter((message) => message.type === "channel_status").length;

  report();
  assert.equal(statusCount(), 1);

  extension.disconnectHost({ message: "Native host has exited" });
  extension.cancelReconnect();
  extension.reconnectHost();
  report();
  assert.equal(statusCount(), 1);

  await extension.send({ type: "sync_channels", channels });
  report();
  assert.equal(statusCount(), 2);
});
test("managed-record routes include previous logins but pending routes use only the current login", async () => {
  const extension = await createExtension();
  const record = {
    login: "newlogin",
    previousLogins: ["oldlogin"],
    layout: "combined_window",
  };
  const previousRoute = "https://www.twitch.tv/oldlogin";
  const currentRoute = "https://www.twitch.tv/newlogin";

  assert.equal(extension.managedRecordRouteMatches(record, previousRoute, "player"), true);
  assert.equal(extension.managedRecordRouteMatches(record, currentRoute, "player"), true);
  assert.equal(
    extension.managedRecordRouteMatches(record, "https://www.twitch.tv/unrelated", "player"),
    false,
  );
  assert.equal(extension.pendingRecordRouteMatches(record, previousRoute, "player"), false);
  assert.equal(extension.pendingRecordRouteMatches(record, currentRoute, "player"), true);
});
test("managed record enumeration exposes immutable snapshots", async () => {
  const extension = await createExtension();
  await extension.open({
    type: "open_selected",
    wake_browser: false,
    channels: [{ id: "channel-snapshot", login: "streamer" }],
  });

  const [[channelId, record]] = extension.managedRecordEntries();
  assert.equal(channelId, "channel-snapshot");
  assert.equal(Object.isFrozen(record), true);
  record.login = "mutated";
  assert.equal(record.login, "streamer");
  assert.equal(extension.managedRecordEntries()[0][1].login, "streamer");
});
test("detaching one managed surface updates manager-owned records", async () => {
  const extension = await createExtension({
    storedManagedTabs: [
      ["channel-split", {
        login: "streamer",
        layout: "separate_player_chat",
        playerTabId: 21,
        chatTabId: 22,
      }],
    ],
    initialTabs: [
      {
        id: 21,
        url: "https://player.twitch.tv/?channel=streamer&parent=twitch.tv",
        windowId: 17,
      },
      {
        id: 22,
        url: "https://www.twitch.tv/popout/streamer/chat",
        windowId: 17,
      },
    ],
  });

  await extension.updateTabUrl(21, "https://example.com/");
  const record = extension.managedRecordEntries()[0][1];
  assert.equal(record.playerTabId, null);
  assert.equal(record.chatTabId, 22);
  assert.equal(extension.tabsById.has(22), true);
  await extension.flushManagedTabs();
  const persistedRecord = extension.snapshotManagedTabs()[0][1];
  assert.equal(persistedRecord.playerTabId, null);
  assert.equal(persistedRecord.chatTabId, 22);
  assert.equal(
    extension.calls.nativeMessages.some((message) => (
      message.type === "tab_closed"
      && message.channel_id === "channel-split"
      && message.surface === "player"
    )),
    true,
  );
});
test("detach reports tab closure after a managed-state storage failure", async () => {
  const extension = await createExtension({
    storedManagedTabs: [
      ["channel-split", {
        login: "streamer",
        layout: "separate_player_chat",
        playerTabId: 21,
        chatTabId: 22,
      }],
    ],
    initialTabs: [
      {
        id: 21,
        url: "https://player.twitch.tv/?channel=streamer&parent=twitch.tv",
        windowId: 17,
      },
      {
        id: 22,
        url: "https://www.twitch.tv/popout/streamer/chat",
        windowId: 17,
      },
    ],
  });
  extension.failNextManagedTabSave();

  await extension.updateTabUrl(21, "https://example.com/");

  const record = extension.managedRecordEntries()[0][1];
  assert.equal(record.playerTabId, null);
  assert.equal(record.chatTabId, 22);
  assert.equal(extension.calls.nativeMessages.some((message) => (
    message.type === "tab_closed"
    && message.channel_id === "channel-split"
    && message.surface === "player"
  )), true);
  assert.equal(
    extension.calls.logs.some((entry) => (
      entry.level === "warn"
      && entry.message.includes("Could not persist detached managed tab state.")
    )),
    true,
  );

  await extension.flushManagedTabs();
  const persistedRecord = extension.snapshotManagedTabs()[0][1];
  assert.equal(persistedRecord.playerTabId, null);
  assert.equal(persistedRecord.chatTabId, 22);
});
test("detaching shared surfaces clears manager state before awaiting persistence", async () => {
  const extension = await createExtension({
    storedManagedTabs: [
      ["channel-shared", {
        login: "streamer",
        layout: "combined_window",
        playerTabId: 51,
        chatTabId: 51,
      }],
    ],
    initialTabs: [{
      id: 51,
      url: "https://www.twitch.tv/streamer",
      windowId: 17,
    }],
  });
  const pause = extension.pauseNextManagedTabSave();
  const update = extension.updateTabUrl(51, "https://example.com/");
  await pause.started;
  const nextMutation = extension.waitForNextBrowserMutation();
  const opening = extension.send({
    type: "open_selected",
    wake_browser: false,
    channels: [{ id: "channel-shared", login: "streamer" }],
  });
  const mutation = await nextMutation;
  pause.release();
  await Promise.all([update, opening]);
  assert.equal(mutation.type, "window_created");
  assert.equal(extension.tabsById.has(51), true);
  assert.equal(extension.tabsById.get(51).url, "https://example.com/");
  const managedRecords = extension.managedRecordEntries();
  assert.equal(managedRecords.length, 1);
  assert.equal(managedRecords[0][0], "channel-shared");
  assert.notEqual(managedRecords[0][1].playerTabId, 51);
  assert.equal(
    extension.snapshotManagedTabOwners().some(([tabId]) => tabId === 51),
    false,
  );
  assert.equal(extension.snapshotManagedTabs().length, 1);
  assert.equal(extension.calls.nativeMessages.filter((message) => (
    message.type === "tab_closed" && message.channel_id === "channel-shared"
  )).length, 2);
});

test("network recovery reloads only app-managed player tabs", async () => {
  const extension = await createExtension();
  await extension.open({
    type: "open_selected",
    channels: [{ id: "channel-recovery", login: "recovery", unmuted: false }],
    wake_browser: false,
    command_id: "session-open",
  });
  assert.ok(extension.calls.nativeMessages.some((message) =>
    message.type === "command_acknowledged" && message.command_id === "session-open"
  ));

  const managedRecord = extension.snapshotManagedTabs()[0][1];
  const playerTabId = managedRecord.playerTabId;
  const externalTabId = 999;
  extension.tabsById.set(externalTabId, {
    id: externalTabId,
    url: "https://www.twitch.tv/external",
    windowId: 17,
    status: "complete",
  });

  const reloadCommand = {
    type: "reload_managed_player_tabs",
    command_id: "session-reload",
  };
  await extension.open(reloadCommand);

  assert.deepEqual(extension.calls.tabReloads, [playerTabId]);
  assert.equal(
    extension.calls.nativeMessages.filter(
      (message) => message.type === "command_acknowledged"
        && message.command_id === reloadCommand.command_id,
    ).length,
    1,
  );
  await extension.open(reloadCommand);
  assert.deepEqual(extension.calls.tabReloads, [playerTabId]);
  assert.equal(
    extension.calls.nativeMessages.filter(
      (message) => message.type === "command_acknowledged"
        && message.command_id === reloadCommand.command_id,
    ).length,
    2,
  );
  const restarted = await createExtension({
    initialTabs: Array.from(extension.tabsById.values()),
    storedManagedTabs: extension.snapshotManagedTabs(),
    storedManagedTabOwners: extension.snapshotManagedTabOwners(),
    storedCompletedNativeCommandIds: extension.snapshotCompletedNativeCommandIds(),
  });
  await restarted.open(reloadCommand);
  assert.deepEqual(restarted.calls.tabReloads, []);
  assert.equal(
    restarted.calls.nativeMessages.filter(
      (message) => message.type === "command_acknowledged"
        && message.command_id === reloadCommand.command_id,
    ).length,
    1,
  );
});

test("completed command acknowledgments wait for durable storage", async () => {
  const extension = await createExtension();
  await extension.open({
    type: "open_selected",
    channels: [{ id: "channel-durable", login: "durable", unmuted: false }],
    wake_browser: false,
    command_id: "session-durable-open",
  });
  const playerTabId = extension.snapshotManagedTabs()[0][1].playerTabId;
  const command = {
    type: "reload_managed_player_tabs",
    command_id: "session-storage-failure",
  };

  extension.failNextCompletedCommandSave();
  await assert.rejects(
    extension.open(command),
    (error) => error.message === "Completed native command ID storage failed",
  );

  assert.deepEqual(extension.calls.tabReloads, [playerTabId]);
  assert.ok(!extension.snapshotCompletedNativeCommandIds().includes(command.command_id));
  assert.equal(
    extension.calls.nativeMessages.some(
      (message) => message.type === "command_acknowledged"
        && message.command_id === command.command_id,
    ),
    false,
  );

  await extension.open(command);
  assert.deepEqual(extension.calls.tabReloads, [playerTabId, playerTabId]);
  assert.ok(extension.snapshotCompletedNativeCommandIds().includes(command.command_id));
  assert.equal(
    extension.calls.nativeMessages.filter(
      (message) => message.type === "command_acknowledged"
        && message.command_id === command.command_id,
    ).length,
    1,
  );
});

test("completed command IDs remain bounded under concurrent updates", async () => {
  const extension = await createExtension();
  const commandIds = Array.from({ length: 130 }, (_, index) => `completed-${index}`);

  await Promise.all(commandIds.map((command_id) => extension.open({
    type: "open_selected",
    channels: [],
    wake_browser: false,
    command_id,
  })));

  assert.deepEqual(
    extension.snapshotCompletedNativeCommandIds(),
    commandIds.slice(-128),
  );
});

test("stale native-port recovery commands are not acknowledged", async () => {
  const extension = await createExtension();
  await extension.open({
    type: "open_selected",
    command_id: "session-stale-open",
    channels: [{ id: "channel-stale-port", login: "stale_port", unmuted: false }],
    wake_browser: false,
  });
  const playerTabId = extension.snapshotManagedTabs()[0][1].playerTabId;
  const command = {
    type: "reload_managed_player_tabs",
    command_id: "session-stale-reload",
  };

  await extension.openFromPort(command, {});
  assert.deepEqual(extension.calls.tabReloads, []);
  assert.equal(
    extension.calls.nativeMessages.some((message) =>
      message.type === "command_acknowledged"
        && message.command_id === command.command_id,
    ),
    false,
  );

  await extension.openFromPort(command, extension.currentNativePort());
  assert.deepEqual(extension.calls.tabReloads, [playerTabId]);
  assert.ok(extension.calls.nativeMessages.some((message) =>
    message.type === "command_acknowledged"
      && message.command_id === command.command_id,
  ));
});
test("in-flight stale-port commands are deduplicated without a stale acknowledgment", async () => {
  const extension = await createExtension();
  await extension.open({
    type: "open_selected",
    command_id: "session-inflight-open",
    channels: [{ id: "channel-inflight-port", login: "inflight_port", unmuted: false }],
    wake_browser: false,
  });
  const playerTabId = extension.snapshotManagedTabs()[0][1].playerTabId;
  const stalePort = extension.currentNativePort();
  const command = {
    type: "reload_managed_player_tabs",
    command_id: "session-inflight-reload",
  };
  const pause = extension.pauseNextTabReload();
  const processing = extension.openFromPort(command, stalePort);
  await pause.started;

  extension.disconnectHost();
  extension.reconnectHost();
  const currentPort = extension.currentNativePort();
  assert.notEqual(currentPort, stalePort);
  pause.release();
  await processing;

  assert.deepEqual(extension.calls.tabReloads, [playerTabId]);
  assert.equal(
    extension.calls.nativeMessages.some((message) =>
      message.type === "command_acknowledged"
        && message.command_id === command.command_id,
    ),
    false,
  );
  await extension.openFromPort(command, currentPort);
  assert.deepEqual(extension.calls.tabReloads, [playerTabId]);
  assert.ok(extension.calls.nativeMessages.some((message) =>
    message.type === "command_acknowledged"
      && message.command_id === command.command_id,
  ));
});
test("external Twitch tabs stay unmanaged until existing-tab adoption is enabled", async () => {
  const extension = await createExtension({
    initialTabs: [{
      id: 201,
      windowId: 17,
      status: "complete",
      url: "https://www.twitch.tv/streamer",
    }],
  });
  await extension.send({
    type: "sync_channels",
    channels: [{ id: "channel-streamer", login: "streamer", unmuted: false }],
  });
  await extension.reportExternalStreams();
  await extension.send({ type: "close_all" });

  assert.equal(extension.tabsById.has(201), true);
  assert.deepEqual(extension.snapshotManagedTabs(), []);
  const inventory = extension.calls.nativeMessages.findLast(
    (message) => message.type === "external_streams",
  );
  assert.deepEqual(JSON.parse(JSON.stringify(inventory.channel_counts)), {
    "channel-streamer": 1,
  });
});

test("opt-in adopts only one eligible followed player tab and closes only that tab", async () => {
  const extension = await createExtension({
    initialTabs: [
      {
        id: 211,
        windowId: 17,
        status: "complete",
        url: "https://www.twitch.tv/streamer",
      },
      {
        id: 212,
        windowId: 17,
        status: "complete",
        active: true,
        url: "https://www.twitch.tv/streamer",
      },
      {
        id: 213,
        windowId: 17,
        status: "complete",
        openerTabId: 88,
        url: "https://www.twitch.tv/streamer",
      },
      {
        id: 214,
        windowId: 17,
        status: "complete",
        url: "https://www.twitch.tv/popout/streamer/chat",
      },
      {
        id: 215,
        windowId: 17,
        status: "complete",
        url: "https://www.twitch.tv/unfollowed",
      },
    ],
  });
  await extension.send({
    type: "sync_channels",
    adopt_existing_twitch_tabs: true,
    channels: [{ id: "channel-streamer", login: "streamer", unmuted: false }],
  });
  await extension.reportExternalStreams();

  const records = extension.snapshotManagedTabs();
  assert.equal(records.length, 1);
  assert.equal(records[0][1].adoptedExternal, true);
  assert.equal(records[0][1].playerTabId, 212);
  assert.equal(extension.tabsById.get(212).url, "https://www.twitch.tv/streamer");
  assert.equal(extension.tabsById.has(211), true);
  assert.equal(extension.tabsById.has(213), true);
  assert.equal(extension.tabsById.has(214), true);
  assert.equal(extension.tabsById.has(215), true);
  assert.ok(extension.calls.nativeMessages.some(
    (message) => message.type === "tab_adopted"
      && message.channel_id === "channel-streamer"
      && message.tab_id === 212,
  ));
  const inventory = extension.calls.nativeMessages.findLast(
    (message) => message.type === "external_streams",
  );
  assert.deepEqual(JSON.parse(JSON.stringify(inventory.channel_counts)), {
    "channel-streamer": 2,
  });
  assert.equal(inventory.unmatched_count, 1);

  await extension.send({ type: "close_all" });
  assert.equal(extension.tabsById.has(212), false);
  assert.equal(extension.tabsById.has(211), true);
  assert.equal(extension.tabsById.has(213), true);
  assert.equal(extension.tabsById.has(214), true);
  assert.equal(extension.tabsById.has(215), true);
});
test("popup refreshes managed streams after an existing player tab is adopted", async () => {
  const extension = await createExtension({
    initialTabs: [
      {
        id: 216,
        windowId: 17,
        status: "complete",
        active: true,
        url: "https://www.twitch.tv/popup_streamer",
      },
    ],
  });
  const popup = extension.openPopup();
  await popup.firstStatus;
  const updatedStreams = popup.nextManagedStreams();

  await extension.send({
    type: "sync_channels",
    adopt_existing_twitch_tabs: true,
    channels: [{ id: "popup-channel", login: "popup_streamer", unmuted: false }],
  });
  await extension.reportExternalStreams();

  assert.equal(await updatedStreams, "1: popup_streamer");
});

test("disabling adoption releases existing tabs without closing them", async () => {
  const extension = await createExtension({
    initialTabs: [{
      id: 221,
      windowId: 17,
      status: "complete",
      url: "https://www.twitch.tv/streamer",
    }],
  });
  const channels = [{ id: "channel-streamer", login: "streamer", unmuted: false }];
  await extension.send({
    type: "sync_channels",
    adopt_existing_twitch_tabs: true,
    channels,
  });
  await extension.reportExternalStreams();
  assert.equal(extension.snapshotManagedTabs().length, 1);
  await extension.send({
    type: "sync_channels",
    adopt_existing_twitch_tabs: false,
    channels,
  });
  await extension.reportExternalStreams();

  assert.equal(extension.snapshotManagedTabs().length, 0);
  assert.equal(extension.tabsById.has(221), true);
  assert.equal(extension.snapshotExistingTabAdoption(), false);
  assert.ok(extension.calls.nativeMessages.some(
    (message) => message.type === "tab_released"
      && message.channel_id === "channel-streamer"
      && message.tab_id === 221,
  ));
});
test("unverifiable restored adopted tabs are released without being closed", async () => {
  const extension = await createExtension({
    storedAdoptExistingTwitchTabs: true,
    storedManagedTabs: [["channel-streamer", {
      login: "streamer",
      playerTabId: 231,
      chatTabId: null,
      layout: "adopted_external",
      identityToken: "old-session",
      identityPersisted: true,
      identityPolicyVersion: 1,
      adoptedExternal: true,
    }]],
    storedManagedTabOwners: [[231, "channel-streamer"]],
    initialTabs: [{
      id: 231,
      windowId: 17,
      status: "complete",
      url: "https://www.twitch.tv/streamer",
    }],
  });

  assert.deepEqual(extension.snapshotManagedTabs(), []);
  assert.equal(extension.tabsById.has(231), true);
});
test("failed adoption persistence leaves the tab external and retries safely", async () => {
  const extension = await createExtension({
    initialTabs: [{
      id: 241,
      windowId: 17,
      status: "complete",
      url: "https://www.twitch.tv/streamer",
    }],
    setTimeoutImpl(callback, delay) {
      return { callback, delay };
    },
    clearTimeoutImpl() {},
  });
  await extension.send({
    type: "sync_channels",
    adopt_existing_twitch_tabs: true,
    channels: [{ id: "channel-streamer", login: "streamer", unmuted: false }],
  });

  extension.failNextManagedTabSave();
  await extension.reportExternalStreams();
  assert.deepEqual(extension.snapshotManagedTabs(), []);
  assert.equal(extension.tabsById.has(241), true);
  assert.ok(extension.calls.nativeMessages.some(
    (message) => message.type === "error"
      && message.channel_id === "channel-streamer"
      && message.message.includes("save adopted Twitch tab ownership"),
  ));
  const inventory = extension.calls.nativeMessages.findLast(
    (message) => message.type === "external_streams",
  );
  assert.deepEqual(JSON.parse(JSON.stringify(inventory.channel_counts)), {
    "channel-streamer": 1,
  });

  await extension.reportExternalStreams();
  assert.equal(extension.snapshotManagedTabs().length, 1);
  assert.equal(extension.tabsById.has(241), true);
});
test("verified adopted website tabs survive browser-start restoration", async () => {
  const extension = await createExtension({
    storedAdoptExistingTwitchTabs: true,
    storedManagedTabs: [["channel-streamer", {
      login: "streamer",
      layout: "combined_window",
      playerTabId: 251,
      chatTabId: null,
      identityToken: "restored-session",
      identityPersisted: true,
      identityPolicyVersion: 1,
      adoptedExternal: true,
    }]],
    storedManagedTabOwners: [[251, "channel-streamer"]],
    browserStartup: true,
    initialTabs: [{
      id: 251,
      windowId: 17,
      status: "complete",
      url: "https://www.twitch.tv/streamer",
      managedTabIdentity: "restored-session",
    }],
  });

  assert.equal(extension.snapshotManagedTabs().length, 1);
  assert.equal(extension.snapshotManagedTabs()[0][1].playerTabId, 251);
  assert.equal(extension.tabsById.get(251).url, "https://www.twitch.tv/streamer");
});

test("closing adopted tabs refuses a tab that navigated to another channel", async () => {
  const extension = await createExtension({
    initialTabs: [{
      id: 261,
      windowId: 17,
      status: "complete",
      url: "https://www.twitch.tv/streamer",
    }],
  });
  await extension.send({
    type: "sync_channels",
    adopt_existing_twitch_tabs: true,
    channels: [{ id: "channel-streamer", login: "streamer", unmuted: false }],
  });
  await extension.reportExternalStreams();

  extension.tabsById.get(261).url = "https://www.twitch.tv/another";
  await extension.send({ type: "close_all" });

  assert.equal(extension.tabsById.has(261), true);
  assert.equal(extension.snapshotManagedTabs().length, 1);
  assert.ok(extension.calls.nativeMessages.some(
    (message) => message.type === "error"
      && message.channel_id === "channel-streamer"
      && message.message.includes("ownership could not be verified"),
  ));
});

test("failed adoption opt-out persistence keeps ownership and retries", async () => {
  const extension = await createExtension({
    initialTabs: [{
      id: 271,
      windowId: 17,
      status: "complete",
      url: "https://www.twitch.tv/streamer",
    }],
    setTimeoutImpl(callback, delay) {
      return { callback, delay };
    },
    clearTimeoutImpl() {},
  });
  const channels = [{ id: "channel-streamer", login: "streamer", unmuted: false }];
  await extension.send({
    type: "sync_channels",
    adopt_existing_twitch_tabs: true,
    channels,
  });
  await extension.reportExternalStreams();
  assert.equal(extension.snapshotManagedTabs().length, 1);

  extension.failNextManagedTabSave();
  await extension.send({
    type: "sync_channels",
    adopt_existing_twitch_tabs: false,
    channels,
  });
  assert.equal(extension.snapshotManagedTabs().length, 1);
  assert.equal(extension.snapshotExistingTabAdoption(), true);
  assert.equal(extension.tabsById.has(271), true);
  assert.equal(extension.calls.nativeMessages.some(
    (message) => message.type === "tab_released" && message.tab_id === 271,
  ), false);

  await extension.send({
    type: "sync_channels",
    adopt_existing_twitch_tabs: false,
    channels,
  });
  assert.equal(extension.snapshotManagedTabs().length, 0);
  assert.equal(extension.snapshotExistingTabAdoption(), false);
  assert.equal(extension.tabsById.has(271), true);
  assert.ok(extension.calls.nativeMessages.some(
    (message) => message.type === "tab_released"
      && message.channel_id === "channel-streamer"
      && message.tab_id === 271,
  ));
});
test("an explicit channel open wins a race with existing-tab adoption", async () => {
  const extension = await createExtension({
    initialTabs: [{
      id: 281,
      windowId: 17,
      status: "complete",
      url: "https://www.twitch.tv/streamer",
    }],
  });
  await extension.send({
    type: "sync_channels",
    channels: [{ id: "channel-streamer", login: "streamer", unmuted: true }],
    adopt_existing_twitch_tabs: true,
  });
  const identityRead = extension.pauseNextManagedTabIdentityRead();
  const adoption = extension.reportExternalStreams();
  await identityRead.started;
  await extension.open({
    type: "open_selected",
    channels: [{ id: "channel-streamer", login: "streamer" }],
    wake_browser: false,
  });
  identityRead.release();
  await adoption;
  const [channelId, record] = extension.snapshotManagedTabs()[0];
  assert.equal(channelId, "channel-streamer");
  assert.notEqual(record.playerTabId, 281);
  assert.notEqual(record.adoptedExternal, true);
  assert.equal(extension.tabsById.has(281), true);
  assert.equal(extension.tabsById.has(record.playerTabId), true);
  assert.equal(extension.snapshotManagedTabs().length, 1);
});

test("adoption releases a tab that navigates while its state is being persisted", async () => {
  const extension = await createExtension({
    initialTabs: [{
      id: 282,
      windowId: 17,
      status: "complete",
      url: "https://www.twitch.tv/streamer",
    }],
  });
  await extension.send({
    type: "sync_channels",
    channels: [{ id: "channel-streamer", login: "streamer", unmuted: true }],
    adopt_existing_twitch_tabs: true,
  });
  const save = extension.pauseNextManagedTabSave();
  const adoption = extension.reportExternalStreams();
  await save.started;
  const navigation = extension.updateTabUrl(282, "https://www.twitch.tv/another");
  save.release();
  await Promise.all([adoption, navigation]);
  assert.equal(extension.tabsById.has(282), true);
  assert.equal(extension.tabsById.get(282).url, "https://www.twitch.tv/another");
  assert.equal(extension.snapshotManagedTabs().length, 0);
  assert.equal(extension.calls.nativeMessages.some(
    (message) => message.type === "tab_adopted" && message.channel_id === "channel-streamer",
  ), false);
});

test("an adopted tab navigating away during raid-close persistence is not closed", async () => {
  const extension = await createExtension({
    initialTabs: [{
      id: 283,
      windowId: 17,
      status: "complete",
      url: "https://www.twitch.tv/streamer",
    }],
  });
  await extension.send({
    type: "sync_channels",
    channels: [{ id: "channel-streamer", login: "streamer", unmuted: true }],
    adopt_existing_twitch_tabs: true,
    close_on_raid_redirect: true,
  });
  await extension.reportExternalStreams();
  const save = extension.pauseNextRaidRedirectedSave();
  const redirect = extension.updateTabUrl(283, "https://www.twitch.tv/streamer?referrer=raid");
  await save.started;
  const away = extension.updateTabUrl(283, "https://example.com/");
  save.release();
  await Promise.all([redirect, away]);
  assert.equal(extension.tabsById.has(283), true);
  assert.equal(extension.tabsById.get(283).url, "https://example.com/");
  assert.equal(extension.snapshotManagedTabs().length, 0);
});
test("failed opt-out does not restore an adopted tab that has navigated away", async () => {
  const extension = await createExtension({
    initialTabs: [{
      id: 291,
      windowId: 17,
      status: "complete",
      url: "https://www.twitch.tv/streamer",
    }],
  });
  await extension.send({
    type: "sync_channels",
    channels: [{ id: "channel-streamer", login: "streamer", unmuted: true }],
    adopt_existing_twitch_tabs: true,
  });
  await extension.reportExternalStreams();
  assert.equal(extension.snapshotManagedTabs()[0][1].adoptedExternal, true);

  const save = extension.pauseNextManagedTabSave({ failOnRelease: true });
  const sync = extension.send({
    type: "sync_channels",
    channels: [],
    adopt_existing_twitch_tabs: false,
  });
  await save.started;
  extension.updateTabUrl(291, "https://example.com/");
  save.release();
  await sync;

  assert.equal(extension.tabsById.has(291), true);
  assert.equal(extension.tabsById.get(291).url, "https://example.com/");
  assert.equal(extension.snapshotManagedTabs().length, 0);
  assert.equal(extension.snapshotExistingTabAdoption(), true);
  assert.ok(extension.calls.nativeMessages.some(
    (message) => message.type === "tab_released"
      && message.channel_id === "channel-streamer"
      && message.tab_id === 291,
  ));
});
test("popup shows managed streams and reports actions that cannot be applied", async () => {
  const extension = await createExtension();
  const popup = extension.openPopup();
  await popup.firstStatus;
  assert.equal(popup.managedStreams.textContent, "No managed streams are open.");

  const waiting = popup.nextActionStatus();
  const result = popup.nextActionStatus();
  popup.pauseButton.click();
  assert.equal(await waiting, "Waiting for the app…");
  assert.match(await result, /^Could not apply action: The app is not connected/);
});
test("popup displays the app-reported reason an action was not applied", async () => {
  const extension = await createExtension();
  const popup = extension.openPopup();
  await popup.firstStatus;
  await extension.send({ type: "bridge_ready" });

  const waiting = popup.nextActionStatus();
  const result = popup.nextActionStatus();
  popup.openSelectedButton.click();
  const request = extension.calls.nativeMessages
    .filter((message) => message.type === "popup_action")
    .at(-1);
  assert.ok(request);
  extension.send({
    type: "popup_action_result",
    request_id: request.request_id,
    success: false,
    message: "Select followed Twitch channels to open.",
  });

  assert.equal(await waiting, "Waiting for the app…");
  assert.equal(
    await result,
    "Could not apply action: Select followed Twitch channels to open.",
  );
});
test("popup confirms open-selected and refreshes its managed-stream summary", async () => {
  const extension = await createExtension();
  const popup = extension.openPopup();
  await popup.firstStatus;
  const connected = popup.nextStatus();
  await extension.send({ type: "bridge_ready" });
  assert.equal(await connected, "Connected to Twitch Streak Saver.");

  const waiting = popup.nextActionStatus();
  const opened = popup.nextActionStatus();
  const managed = popup.nextManagedStreams();
  popup.openSelectedButton.click();
  const request = extension.calls.nativeMessages
    .filter((message) => message.type === "popup_action")
    .at(-1);
  assert.ok(request);

  const openCommand = extension.send({
    type: "open_selected",
    wake_browser: false,
    channels: [{ id: "popup-channel", login: "popup_streamer", unmuted: true }],
    command_id: "popup-open",
  });
  extension.send({
    type: "popup_action_result",
    request_id: request.request_id,
    success: true,
    message: "Open-selected request sent to the browser.",
  });

  assert.equal(await waiting, "Waiting for the app…");
  assert.equal(await opened, "Open-selected request sent to the browser.");
  await openCommand;
  assert.equal(await managed, "1: popup_streamer");
});

test("popup reports a pending action when the native bridge disconnects", async () => {
  const extension = await createExtension();
  const popup = extension.openPopup();
  await popup.firstStatus;
  const connected = popup.nextStatus();
  await extension.send({ type: "bridge_ready" });
  assert.equal(await connected, "Connected to Twitch Streak Saver.");

  const waiting = popup.nextActionStatus();
  const failed = popup.nextActionStatus();
  popup.pauseButton.click();
  assert.equal(await waiting, "Waiting for the app…");
  extension.disconnectHost(new Error("native bridge disconnected"));
  extension.cancelReconnect();
  assert.equal(
    await failed,
    "Could not apply action: The app disconnected before it could apply the action.",
  );
});
test("popup action results arrive while open-selected is still queued", async () => {
  const extension = await createExtension();
  const popup = extension.openPopup();
  await popup.firstStatus;
  const connected = popup.nextStatus();
  await extension.send({ type: "bridge_ready" });
  assert.equal(await connected, "Connected to Twitch Streak Saver.");

  const waiting = popup.nextActionStatus();
  const applied = popup.nextActionStatus();
  popup.openSelectedButton.click();
  const request = extension.calls.nativeMessages
    .filter((message) => message.type === "popup_action")
    .at(-1);
  assert.ok(request);

  const releaseQueue = extension.blockNativeCommandQueue();
  let openCommand;
  try {
    openCommand = extension.send({
      type: "open_selected",
      wake_browser: false,
      channels: [{ id: "slow-channel", login: "slow_streamer", unmuted: true }],
      command_id: "slow-open",
    });
    extension.send({
      type: "popup_action_result",
      request_id: request.request_id,
      success: true,
      message: "Open-selected request sent to the browser.",
    });
    assert.equal(await waiting, "Waiting for the app…");
    assert.equal(await applied, "Open-selected request sent to the browser.");
  } finally {
    releaseQueue();
  }
  await openCommand;
});

test("unmanaged Twitch player navigation refreshes open streamer inventory", async () => {
  const scheduled = [];
  const extension = await createExtension({
    storedManagedTabs: [
      [
        "managed-channel",
        {
          login: "managed",
          layout: "combined_window",
          playerTabId: 402,
          chatTabId: 402,
          contentVersion: "0.3.4",
        },
      ],
    ],
    initialTabs: [
      { id: 401, url: "https://www.twitch.tv/first" },
      { id: 402, url: "https://www.twitch.tv/managed" },
    ],
    setTimeoutImpl(callback, delay) {
      const timer = { callback, delay, cancelled: false };
      scheduled.push(timer);
      return timer;
    },
    clearTimeoutImpl(timer) {
      if (timer) timer.cancelled = true;
    },
  });

  async function flushInventory() {
    const timer = scheduled.find(
      (candidate) => !candidate.cancelled && candidate.delay === 100,
    );
    assert.ok(timer);
    timer.cancelled = true;
    await timer.callback();
  }

  await extension.send({
    type: "sync_channels",
    channels: [{ id: "managed-channel", login: "managed", unmuted: false }],
  });
  while (scheduled.some((candidate) => !candidate.cancelled && candidate.delay === 100)) {
    await flushInventory();
  }
  await extension.reportExternalStreams();
  const priorReportCount = extension.calls.nativeMessages.filter(
    (message) => message.type === "external_streams",
  ).length;
  await extension.updateTabUrl(401, "https://www.twitch.tv/second");
  await flushInventory();

  const inventories = extension.calls.nativeMessages.filter(
    (message) => message.type === "external_streams",
  );
  assert.equal(inventories.length, priorReportCount + 1);
  assert.deepEqual(
    JSON.parse(JSON.stringify(inventories.at(-1).open_streamer_logins)),
    ["managed", "second"],
  );
});