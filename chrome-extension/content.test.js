const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");
const TwitchQuality = require("./quality.js");

test("content reporting stops after extension reload invalidates the page context", () => {
  const sent = [];
  let onMessage;
  let intervalCount = 0;
  let shouldThrow = false;
  let intervalCallback;
  const clearedIntervals = [];
  const video = {
    ended: false,
    paused: false,
    readyState: 3,
    getClientRects: () => [1],
    getBoundingClientRect: () => ({ width: 1280, height: 720 }),
  };
  const context = vm.createContext({
    chrome: {
      runtime: {
        sendMessage(message) {
          sent.push(message);
          if (shouldThrow) throw new Error("Extension context invalidated.");
          return Promise.resolve();
        },
        onMessage: {
          addListener(listener) {
            onMessage = listener;
          },
        },
      },
      storage: {
        local: {
          get(defaults) {
            return Promise.resolve(defaults);
          },
        },
        onChanged: { addListener() {} },
      },
    },
    document: {
      querySelectorAll: () => [video],
      querySelector: () => ({}),
    },
    HTMLMediaElement: { HAVE_FUTURE_DATA: 3 },
    location: { href: "https://www.twitch.tv/streamer" },
    TwitchTabUtils: {
      channelRoute: () => ({ login: "streamer", surface: "player" }),
    },
    TwitchQuality,
    window: {
      setInterval(callback) {
        intervalCount += 1;
        intervalCallback = callback;
        return intervalCount;
      },
      clearInterval(id) {
        clearedIntervals.push(id);
      },
    },
  });
  const source = fs.readFileSync(path.join(__dirname, "content.js"), "utf8");
  vm.runInContext(source, context, { filename: "content.js" });

  assert.doesNotThrow(() => onMessage({ type: "start_monitoring" }));
  const reports = () => sent.filter((message) => message.type === "channel_status");
  assert.equal(reports().length, 1);
  assert.deepEqual(JSON.parse(JSON.stringify(reports()[0])), {
    type: "channel_status",
    surface: "player",
    login: "streamer",
    player_state: "playing",
    chat_state: "ready",
  });
  assert.equal(intervalCount, 2);

  shouldThrow = true;
  video.paused = true;
  assert.doesNotThrow(() => intervalCallback());
  assert.equal(reports().length, 2);
  assert.deepEqual(clearedIntervals.sort(), [1, 2]);
  intervalCallback();
  assert.equal(reports().length, 2);
});

test("automatically applies the stored default and reacts to quality preference changes", async () => {
  const timers = [];
  const intervals = [];
  let onQualityChange;
  let storageDefaults;
  let menuState = "";
  let selectedQuality = "";
  let now = 0;
  const video = {
    ended: false,
    paused: false,
    readyState: 3,
    videoWidth: 1920,
    videoHeight: 1080,
    getClientRects: () => [1],
    getBoundingClientRect: () => ({ width: 1920, height: 1080 }),
  };
  const element = (text, click, attributes = {}) => ({
    innerText: text,
    disabled: false,
    getClientRects: () => [1],
    getAttribute: (name) => attributes[name] ?? null,
    click,
  });
  const closeButton = element("Close", () => { menuState = ""; }, { "aria-label": "Close" });
  const menu = {
    getClientRects: () => menuState ? [1] : [],
    querySelectorAll: () => [closeButton],
  };
  const settingsButton = element("", () => { menuState = "settings"; });
  let settingsButtonAvailable = true;
  const qualityButton = element("Quality", () => { menuState = "quality"; });
  const makeQualityOption = (text, width, height) => element(text, () => {
    selectedQuality = text;
    video.videoWidth = width;
    video.videoHeight = height;
  });
  const options = [
    makeQualityOption("Auto", 1920, 1080),
    makeQualityOption("720p60", 1280, 720),
    makeQualityOption("160p", 284, 160),
  ];
  let shownOptions = [];
  const document = {
    querySelectorAll(selector) {
      if (selector === "video") return [video];
      if (selector === 'button[data-a-target="player-settings-button"]') {
        return settingsButtonAvailable ? [settingsButton] : [];
      }
      if (selector === '[role="menu"]') return menuState ? [menu] : [];
      if (selector === '[data-a-target="player-settings-menu-item-quality"]') {
        return menuState === "settings" ? [qualityButton] : [];
      }
      if (selector === '[role="menuitemradio"]') {
        return menuState === "quality" ? shownOptions : [];
      }
      return [];
    },
    querySelector: () => ({}),
  };
  const context = vm.createContext({
    Date: { now: () => now },
    chrome: {
      runtime: {
        sendMessage: () => Promise.resolve(),
        onMessage: { addListener() {} },
      },
      storage: {
        local: {
          get(defaults) {
            storageDefaults = defaults;
            return Promise.resolve({});
          },
        },
        onChanged: {
          addListener(listener) {
            onQualityChange = listener;
          },
        },
      },
    },
    document,
    HTMLMediaElement: { HAVE_FUTURE_DATA: 3 },
    location: { href: "https://www.twitch.tv/streamer" },
    TwitchTabUtils: { channelRoute: () => ({ login: "streamer", surface: "player" }) },
    TwitchQuality,
    window: {
      opener: null,
      sessionStorage: { getItem: () => null, removeItem() {} },
      setInterval(callback, delay) {
        intervals.push({ callback, delay });
        return intervals.length;
      },
      clearInterval() {},
      setTimeout(callback) {
        timers.push(callback);
        return timers.length;
      },
    },
  });
  const source = fs.readFileSync(path.join(__dirname, "content.js"), "utf8");
  vm.runInContext(source, context, { filename: "content.js" });
  const advanceMenu = async () => {
    const callback = timers.shift();
    assert.equal(typeof callback, "function");
    callback();
    await Promise.resolve();
  };

  assert.equal(storageDefaults.preferredQuality, "160p");
  assert.equal(intervals[0].delay, 1000);
  settingsButtonAvailable = false;
  onQualityChange({ preferredQuality: { newValue: undefined } }, "local");
  assert.equal(menuState, "");
  intervals[0].callback();
  assert.equal(menuState, "");

  settingsButtonAvailable = true;
  now = 1000;
  intervals[0].callback();
  assert.equal(menuState, "settings");
  await advanceMenu();
  assert.equal(menuState, "quality");
  await advanceMenu();
  assert.equal(selectedQuality, "");
  assert.equal(menuState, "");
  for (const retryAt of [3000, 7000, 15000]) {
    now = retryAt;
    intervals[0].callback();
    assert.equal(menuState, "settings");
    await advanceMenu();
    assert.equal(menuState, "quality");
    await advanceMenu();
    assert.equal(menuState, "");
  }

  shownOptions = options;
  now = 31000;
  intervals[0].callback();
  assert.equal(menuState, "settings");
  await advanceMenu();
  assert.equal(menuState, "quality");
  await advanceMenu();
  assert.equal(selectedQuality, "160p");
  await advanceMenu();
  assert.equal(menuState, "");
  assert.deepEqual([video.videoWidth, video.videoHeight], [284, 160]);

  onQualityChange({ preferredQuality: { oldValue: "160p", newValue: "720p" } }, "local");
  assert.equal(menuState, "settings");
  await advanceMenu();
  assert.equal(menuState, "quality");
  await advanceMenu();
  assert.equal(selectedQuality, "720p60");
  await advanceMenu();
  assert.equal(menuState, "");
  assert.deepEqual([video.videoWidth, video.videoHeight], [1280, 720]);
});
