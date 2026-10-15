const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const crypto = require("node:crypto");
const trustedExtensionId = fs.readFileSync(path.join(__dirname, "trusted-extension-id"), "utf8");
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
  browserStartup = false,
  initialTabs = [],
  extensionVersion = "0.3.12",
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
        async get() {
          return {
            managedTabs: storedTabs,
            closeOnRaidRedirect: storedRaidSetting,
            raidRedirectedChannelIds: storedRaidRedirectedIds,
            managedTabOwners: storedTabOwners,
          };
        },
        async set(value) {
          if (Object.hasOwn(value, "managedTabs")) {
            storedTabs = JSON.parse(JSON.stringify(value.managedTabs));
          }
          if (Object.hasOwn(value, "closeOnRaidRedirect")) {
            storedRaidSetting = value.closeOnRaidRedirect;
          }
          if (Object.hasOwn(value, "raidRedirectedChannelIds")) {
            storedRaidRedirectedIds = JSON.parse(JSON.stringify(value.raidRedirectedChannelIds));
          }
          if (Object.hasOwn(value, "managedTabOwners")) {
            storedTabOwners = JSON.parse(JSON.stringify(value.managedTabOwners));
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
        tabsById.delete(tabId);
        onRemoved.dispatch(tabId);
      },
      async sendMessage(tabId, message) {
        const tab = tabsById.get(tabId);
        calls.contentMessages.push({
          tabId,
          type: message.type,
          status: tab?.status,
          discarded: tab?.discarded,
        });
        if (message.type === "get_managed_tab_identity") {
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
    open(message) {
      return vm.runInContext(`handleNativeCommand(${JSON.stringify(message)})`, context);
    },
    send(message) {
      port.onMessage.dispatch(message);
      return vm.runInContext("nativeCommandQueue", context);
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
      const wakeButton = {
        disabled: false,
        onClick: null,
        addEventListener(_name, listener) {
          this.onClick = listener;
        },
        click() {
          if (!this.disabled) this.onClick?.();
        },
      };
      context.document = {
        querySelector(selector) {
          return selector === "#status" ? status : wakeButton;
        },
      };
      const popupSource = fs.readFileSync(path.join(__dirname, "popup.js"), "utf8");
      const firstStatus = new Promise((resolve) => statusWaiters.push(resolve));
      vm.runInContext(popupSource, context, { filename: "popup.js" });
      return {
        status,
        wakeButton,
        firstStatus,
        nextStatus() {
          return new Promise((resolve) => statusWaiters.push(resolve));
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
    snapshotRaidSetting() {
      return storedRaidSetting;
    },
    snapshotManagedTabOwners() {
      return JSON.parse(JSON.stringify(storedTabOwners));
    },
    snapshotRaidRedirectedChannelIds() {
      return JSON.parse(JSON.stringify(storedRaidRedirectedIds));
    },
    async updateTabUrl(tabId, url) {
      const tab = tabsById.get(tabId);
      if (!tab) throw new Error("No such tab");
      tab.url = url;
      await Promise.all(onUpdated.dispatch(tabId, { url }, { ...tab }));
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
      },
      window: {
        sessionStorage: {
          getItem(key) {
            return values.get(key) ?? null;
          },
          setItem(key, value) {
            values.set(key, value);
          },
        },
      },
    });
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
    },
  });
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
  });
  const [tab] = [...extension.tabsById.values()];

  extension.reportState(tab.id, {
    surface: "player",
    login: "streamer",
    player_state: "playing",
    chat_state: "ready",
  });

  assert.deepEqual(
    JSON.parse(JSON.stringify(extension.calls.nativeMessages.filter(
      (message) => message.type === "channel_status",
    ))),
    [{
      type: "channel_status",
      channel_id: "channel-1",
      tab_id: tab.id,
      surface: "player",
      player_state: "playing",
      chat_state: "ready",
    }],
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
        contentVersion: "0.3.12",
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
        contentVersion: "0.3.12",
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
        contentVersion: "0.3.12",
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
        contentVersion: "0.3.12",
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
        contentVersion: "0.3.12",
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
        contentVersion: "0.3.12",
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
        contentVersion: "0.3.12",
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
        contentVersion: "0.3.12",
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
    channels: [{ id: "channel-update", login: "streamer" }],
  });
  const [originalTab] = [...original.tabsById.values()];
  const restarted = await createExtension({
    extensionVersion: "0.3.4",
    storedManagedTabs: original.snapshotManagedTabs(),
    initialTabs: [originalTab],
  });

  await restarted.send({
    type: "sync_channels",
    channels: [{ id: "channel-update", login: "streamer" }],
  });

  assert.equal(restarted.tabsById.get(originalTab.id).status, "complete");
  assert.equal(
    restarted.calls.tabUpdates.filter(
      ({ changes }) => changes.url === "https://www.twitch.tv/streamer",
    ).length,
    1,
  );
  assert.equal(restarted.snapshotManagedTabs()[0][1].contentVersion, "0.3.4");
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