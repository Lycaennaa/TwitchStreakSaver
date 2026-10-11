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

async function createExtension({ pauseFirstCreate = false } = {}) {
  const tabsById = new Map();
  const calls = { windowUpdates: [], nativeMessages: [] };
  let nextTabId = 1;
  const failedRemovals = new Set();
  let notifyFirstCreate;
  const firstCreateStarted = new Promise((resolve) => {
    notifyFirstCreate = resolve;
  });
  let releaseFirstCreate;
  const firstCreateGate = new Promise((resolve) => {
    releaseFirstCreate = resolve;
  });
  let createCount = 0;
  const onUpdated = event();
  const onCreated = event();
  const onRemoved = event();
  const port = {
    onMessage: event(),
    onDisconnect: event(),
    postMessage(message) {
      calls.nativeMessages.push(message);
    },
  };
  const chrome = {
    storage: {
      session: {
        async get() {
          return { managedTabs: [] };
        },
        async set() {},
      },
    },
    tabs: {
      onUpdated,
      onCreated,
      onRemoved,
      async query() {
        return [...tabsById.values()].map((tab) => ({ ...tab }));
      },
      async create(options) {
        if (pauseFirstCreate && createCount++ === 0) {
          notifyFirstCreate();
          await firstCreateGate;
        }
        const tab = {
          id: nextTabId++,
          windowId: 17,
          status: "complete",
          active: options.active,
          url: options.url,
        };
        tabsById.set(tab.id, tab);
        return { ...tab };
      },
      async get(tabId) {
        const tab = tabsById.get(tabId);
        if (!tab) throw new Error("No such tab");
        return { ...tab };
      },
      async update(tabId, changes) {
        const tab = tabsById.get(tabId);
        if (!tab) throw new Error("No such tab");
        Object.assign(tab, changes);
        if (changes.url) {
          tab.status = "loading";
          setTimeout(() => {
            const current = tabsById.get(tabId);
            if (!current) return;
            current.status = "complete";
            onUpdated.dispatch(tabId, { status: "complete", url: current.url }, { ...current });
          }, 0);
        }
        return { ...tab };
      },
      async remove(tabId) {
        if (failedRemovals.has(tabId)) throw new Error("Blocked tab removal");
        tabsById.delete(tabId);
        onRemoved.dispatch(tabId);
      },
      async sendMessage() {},
    },
    windows: {
      async update(windowId, options) {
        calls.windowUpdates.push({ windowId, options });
      },
    },
    runtime: {
      onStartup: event(),
      onInstalled: event(),
      onMessage: event(),
      connectNative() {
        return port;
      },
    },
  };
  const context = vm.createContext({ chrome, setTimeout, clearTimeout, console, URL });
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
    open(message) {
      return vm.runInContext(`handleNativeCommand(${JSON.stringify(message)})`, context);
    },
    send(message) {
      port.onMessage.dispatch(message);
      return vm.runInContext("nativeCommandQueue", context);
    },
    waitForFirstCreate: () => firstCreateStarted,
    releaseFirstCreate,
    failRemoval(tabId) {
      failedRemovals.add(tabId);
    },
    allowRemoval(tabId) {
      failedRemovals.delete(tabId);
    },
  };
}

test("wake mode restores browser, focuses player, and waits for both tabs to load", async () => {
  const extension = await createExtension();
  await extension.open({
    type: "open_selected",
    layout: "separate_player_chat",
    wake_browser: true,
    channels: [{ id: "channel-1", login: "streamer" }],
  });

  const tabs = [...extension.tabsById.values()];
  const player = tabs.find((tab) => tab.url.includes("player.twitch.tv"));
  const chat = tabs.find((tab) => tab.url.includes("/popout/"));
  assert.equal(player.status, "complete");
  assert.equal(player.active, true);
  assert.equal(player.muted, true);
  assert.equal(chat.status, "complete");
  assert.equal(chat.active, false);
  assert.deepEqual(JSON.parse(JSON.stringify(extension.calls.windowUpdates)), [
    { windowId: 17, options: { state: "normal", focused: true } },
  ]);
  assert.deepEqual(
    extension.calls.nativeMessages
      .filter((message) => message.type === "tab_opened")
      .map((message) => message.surface),
    ["player", "chat"],
  );
});

test("wake mode opens a regular Twitch page as one loaded active tab", async () => {
  const extension = await createExtension();
  await extension.open({
    type: "open_selected",
    layout: "regular_website",
    wake_browser: true,
    channels: [{ id: "channel-2", login: "streamer" }],
  });

  const [tab] = [...extension.tabsById.values()];
  assert.equal(tab.url, "https://www.twitch.tv/streamer");
  assert.equal(tab.status, "complete");
  assert.equal(tab.active, true);
  assert.deepEqual(JSON.parse(JSON.stringify(extension.calls.windowUpdates)), [
    { windowId: 17, options: { state: "normal", focused: true } },
  ]);
  assert.deepEqual(
    extension.calls.nativeMessages
      .filter((message) => message.type === "tab_opened")
      .map((message) => message.surface),
    ["player", "chat"],
  );
});

test("layout changes queue behind in-progress opens and retain tab ownership", async () => {
  const extension = await createExtension({ pauseFirstCreate: true });
  const opening = extension.send({
    type: "open_selected",
    layout: "separate_player_chat",
    wake_browser: false,
    channels: [{ id: "channel-race", login: "streamer" }],
  });
  await extension.waitForFirstCreate();
  const migration = extension.send({
    type: "apply_layout",
    layout: "regular_website",
    wake_browser: false,
  });
  extension.releaseFirstCreate();
  await migration;
  await opening;

  const [tab] = [...extension.tabsById.values()];
  assert.equal(extension.tabsById.size, 1, JSON.stringify([...extension.tabsById.values()]));
  assert.equal(tab.url, "https://www.twitch.tv/streamer");
  assert.equal(tab.status, "complete");
  await extension.send({ type: "close_all" });
  assert.equal(extension.tabsById.size, 0);
});

test("failed tab removal preserves ownership and retries layout migration", async () => {
  const extension = await createExtension();
  await extension.open({
    type: "open_selected",
    layout: "separate_player_chat",
    wake_browser: false,
    channels: [{ id: "channel-failure", login: "streamer" }],
  });
  const player = [...extension.tabsById.values()].find((tab) => tab.url.includes("player.twitch.tv"));
  extension.failRemoval(player.id);

  await extension.open({ type: "apply_layout", layout: "regular_website", wake_browser: false });

  assert.equal(extension.tabsById.has(player.id), true);
  assert.equal([...extension.tabsById.values()].some((tab) => tab.url === "https://www.twitch.tv/streamer"), false);
  assert.equal(
    extension.calls.nativeMessages.filter(
      (message) => message.type === "tab_closed" && message.tab_id === player.id,
    ).length,
    0,
  );
  assert.equal(
    extension.calls.nativeMessages.filter(
      (message) => message.type === "error" && message.channel_id === "channel-failure",
    ).length,
    1,
  );

  extension.allowRemoval(player.id);
  await extension.open({ type: "apply_layout", layout: "regular_website", wake_browser: false });

  const [regularTab] = [...extension.tabsById.values()];
  assert.equal(extension.tabsById.size, 1);
  assert.equal(regularTab.url, "https://www.twitch.tv/streamer");
  assert.equal(regularTab.status, "complete");
  assert.equal(
    extension.calls.nativeMessages.filter(
      (message) => message.type === "tab_closed" && message.tab_id === player.id,
    ).length,
    1,
  );
});

test("default tab opening leaves browser focus and active tab unchanged", async () => {
  const extension = await createExtension();
  await extension.open({
    type: "open_selected",
    layout: "regular_website",
    wake_browser: false,
    channels: [{ id: "channel-3", login: "streamer" }],
  });

  const [tab] = [...extension.tabsById.values()];
  assert.equal(tab.status, "complete");
  assert.equal(tab.active, false);
  assert.deepEqual(extension.calls.windowUpdates, []);
});