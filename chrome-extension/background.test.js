const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
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
      for (const listener of [...listeners]) listener(...args);
    },
  };
}

async function createExtension({
  pauseFirstWindow = false,
  pauseFirstNavigation = false,
  storedManagedTabs = [],
  initialTabs = [],
  extensionVersion = "0.3.4",
} = {}) {
  const tabsById = new Map();
  const windowsById = new Map([[17, { id: 17, focused: true, state: "normal", type: "normal" }]]);
  const calls = {
    windowCreates: [],
    windowUpdates: [],
    tabUpdates: [],
    nativeMessages: [],
    contentMessages: [],
    logs: [],
  };
  let nextTabId = 1;
  let nextWindowId = 18;
  let storedTabs = JSON.parse(JSON.stringify(storedManagedTabs));
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
  });
  let port = createPort();
  const chrome = {
    storage: {
      local: {
        async get() {
          return { managedTabs: storedTabs };
        },
        async set(value) {
          storedTabs = JSON.parse(JSON.stringify(value.managedTabs));
        },
      },
    },
    tabs: {
      onUpdated,
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
        return port;
      },
    },
  };
  const context = vm.createContext({
    chrome,
    setTimeout,
    clearTimeout,
    URL,
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
  await vm.runInContext("restorePromise", context);
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
test("native host disconnect consumes runtime.lastError and schedules reconnection", async () => {
  const extension = await createExtension();
  extension.disconnectHost({ message: "Native host has exited" });

  assert.equal(extension.lastErrorReads(), 1);
  assert.equal(extension.isNativeConnected(), false);
  assert.ok(extension.calls.logs.some(
    (entry) => entry.level === "debug" && entry.message.includes("Native host has exited"),
  ));
  extension.cancelReconnect();
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

test("restoration does not claim a reused tab ID with a different URL", async () => {
  const extension = await createExtension({
    storedManagedTabs: [
      ["channel-reused", {
        login: "streamer",
        layout: "combined_window",
        playerTabId: 41,
        chatTabId: 41,
      }],
    ],
    initialTabs: [{ id: 41, url: "https://example.com/" }],
  });

  await extension.send({
    type: "sync_channels",
    channels: [{ id: "channel-reused", login: "streamer" }],
  });

  assert.equal(extension.tabsById.size, 1);
  assert.equal(extension.tabsById.get(41).url, "https://example.com/");
  assert.deepEqual(extension.calls.windowCreates, []);
  assert.deepEqual(extension.calls.windowUpdates, []);
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