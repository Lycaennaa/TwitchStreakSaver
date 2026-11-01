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
test("uses the player mute control without carrying preferences across routes", () => {
  const preferenceKey = "__twitch_streak_saver_player_unmuted__";
  const session = new Map([[preferenceKey, JSON.stringify({ login: "streamer", unmuted: false })]]);
  const makeVideo = (muted) => ({
    muted,
    volume: 0.5,
    ended: false,
    paused: false,
    readyState: 3,
    videoWidth: 1280,
    videoHeight: 720,
    getClientRects: () => [1],
    getBoundingClientRect: () => ({ width: 1280, height: 720 }),
  });
  const createPage = (initialVideos, opener = null, muteButton = null, volumeSlider = null) => {
    let onMessage;
    let routeLogin = "streamer";
    let routeSurface = "player";
    let videos = initialVideos;
    const pageIntervals = [];
    const documentListeners = new Map();
    const playerContainers = new Map();
    const attachPlayer = (video, controls) => {
      const player = {
        querySelector(selector) {
          if (selector === 'button[data-a-target="player-mute-unmute-button"]') {
            return controls.muteButton;
          }
          if (selector === 'input[data-a-target="player-volume-slider"]') {
            return controls.volumeSlider;
          }
          return null;
        },
        querySelectorAll(selector) {
          if (selector === "video") return [video];
          const control = this.querySelector(selector);
          return control ? [control] : [];
        },
      };
      playerContainers.set(video, player);
      video.parentElement = player;
    };
    const controlsForSelector = (selector) => {
      const controls = videos
        .map((video) => playerContainers.get(video)?.querySelector(selector))
        .filter(Boolean);
      if (controls.length > 0) return controls;
      if (selector === 'button[data-a-target="player-mute-unmute-button"]' && muteButton) {
        return [muteButton];
      }
      if (selector === 'input[data-a-target="player-volume-slider"]' && volumeSlider) {
        return [volumeSlider];
      }
      return [];
    };
    const context = vm.createContext({
      chrome: {
        runtime: {
          sendMessage: () => Promise.resolve(),
          onMessage: {
            addListener(listener) {
              onMessage = listener;
            },
          },
        },
        storage: {
          local: { get: (defaults) => Promise.resolve(defaults) },
          onChanged: { addListener() {} },
        },
      },
      document: {
        addEventListener(type, listener) {
          const listeners = documentListeners.get(type) || [];
          listeners.push(listener);
          documentListeners.set(type, listeners);
        },
        querySelector: (selector) => (
          selector === "video"
            ? videos[0] ?? null
            : controlsForSelector(selector)[0] ?? null
        ),
        querySelectorAll: (selector) => (
          selector === "video" ? videos : controlsForSelector(selector)
        ),
      },
      location: {
        get href() { return `https://www.twitch.tv/${routeLogin}`; },
      },
      TwitchTabUtils: {
        channelRoute: () => ({ login: routeLogin, surface: routeSurface }),
      },
      TwitchQuality,
      window: {
        opener,
        sessionStorage: {
          getItem: (key) => session.get(key) ?? null,
          setItem: (key, value) => session.set(key, value),
          removeItem: (key) => session.delete(key),
        },
        setInterval(callback, delay) {
          pageIntervals.push({ callback, delay });
          return pageIntervals.length;
        },
        clearInterval() {},
        setTimeout(callback) {
          callback();
          return 1;
        },
      },
    });
    const source = fs.readFileSync(path.join(__dirname, "content.js"), "utf8");
    vm.runInContext(source, context, { filename: "content.js" });
    return {
      setMuteButton(nextMuteButton) {
        muteButton = nextMuteButton;
      },
      setVolumeSlider(nextVolumeSlider) {
        volumeSlider = nextVolumeSlider;
      },
      setPlayerControls(video, nextMuteButton, nextVolumeSlider) {
        attachPlayer(video, {
          muteButton: nextMuteButton,
          volumeSlider: nextVolumeSlider,
        });
      },
      signalManualAudioChange() {
        const event = {
          isTrusted: true,
          target: {
            closest: (selector) => (
              selector === 'button[data-a-target="player-mute-unmute-button"]'
                ? muteButton
                : null
            ),
            matches: () => false,
          },
        };
        for (const listener of documentListeners.get("click") || []) listener(event);
      },
      onMessage,
      intervals: pageIntervals,
      replaceVideos(nextVideos) {
        videos = nextVideos;
      },
      setLogin(login) {
        routeLogin = login;
      },
      setSurface(surface) {
        routeSurface = surface;
      },
    };
  };

  const forgedVideo = makeVideo(true);
  createPage([forgedVideo]);
  assert.equal(forgedVideo.muted, true);

  const videoA = makeVideo(true);
  let muteButtonLabel = "Unmute";
  let muteButtonClicks = 0;
  let activeVideo = videoA;
  const muteButton = {
    getClientRects: () => [1],
    getAttribute(name) {
      return name === "aria-label" ? muteButtonLabel : null;
    },
    click() {
      muteButtonClicks += 1;
      muteButtonLabel = muteButtonLabel === "Unmute" ? "Mute" : "Unmute";
      activeVideo.muted = muteButtonLabel === "Unmute";
    },
  };
  const page = createPage([videoA]);
  let response;
  page.onMessage(
    { type: "set_player_audio_preference", login: "streamer", unmuted: true },
    {},
    (value) => { response = value; },
  );
  assert.equal(response.unmuted, true);
  assert.equal(videoA.muted, false);
  assert.equal(muteButtonClicks, 0);
  page.setMuteButton(muteButton);
  page.intervals[0].callback();
  assert.equal(muteButtonClicks, 1);
  assert.equal(muteButtonLabel, "Mute");
  assert.equal(
    session.get(preferenceKey),
    JSON.stringify({ login: "streamer", unmuted: false }),
  );

  muteButton.click();
  page.signalManualAudioChange();
  page.intervals[0].callback();
  assert.equal(videoA.muted, true);
  assert.equal(muteButtonClicks, 2);

  const replacementA = makeVideo(true);
  activeVideo = replacementA;
  page.replaceVideos([replacementA]);
  page.intervals[0].callback();
  assert.equal(muteButtonClicks, 3);
  assert.equal(replacementA.muted, false);
  const replacementB = makeVideo(true);
  replacementB.volume = 0;
  const volumeSlider = {
    getClientRects: () => [1],
    valueAsNumber: 0,
  };
  activeVideo = replacementB;
  page.replaceVideos([replacementB]);
  page.setVolumeSlider(volumeSlider);
  page.intervals[0].callback();
  assert.equal(muteButtonClicks, 3);
  assert.equal(replacementB.muted, false);
  assert.equal(replacementB.volume, 0.5);

  page.setLogin("another");
  const videoB = makeVideo(true);
  page.replaceVideos([videoB]);
  activeVideo = videoB;
  page.intervals[0].callback();
  assert.equal(videoB.muted, true);
  page.onMessage(
    { type: "set_player_audio_preference", login: "streamer", unmuted: true },
    {},
    (value) => { response = value; },
  );
  assert.equal(response.unmuted, true);
  assert.equal(videoB.muted, true);
  assert.equal(
    session.get(preferenceKey),
    JSON.stringify({ login: "streamer", unmuted: false }),
  );

  const chatVideo = makeVideo(true);
  const chatPage = createPage([chatVideo]);
  chatPage.onMessage(
    { type: "set_player_audio_preference", login: "streamer", unmuted: true },
    {},
    () => {},
  );
  assert.equal(chatVideo.muted, false);
  chatPage.setSurface("chat");
  chatPage.intervals[0].callback();
  const nonSelectedVideo = makeVideo(true);
  nonSelectedVideo.getBoundingClientRect = () => ({ width: 320, height: 180 });
  const selectedVideo = makeVideo(true);
  selectedVideo.volume = 0;
  let nonSelectedButtonClicks = 0;
  let selectedButtonClicks = 0;
  let nonSelectedButtonLabel = "Unmute";
  const nonSelectedButton = {
    getAttribute: (name) => name === "aria-label" ? nonSelectedButtonLabel : null,
    click() {
      nonSelectedButtonClicks += 1;
      nonSelectedButtonLabel = "Mute";
      nonSelectedVideo.muted = false;
    },
  };
  let selectedButtonLabel = "Unmute";
  const selectedButton = {
    getAttribute: (name) => name === "aria-label" ? selectedButtonLabel : null,
    click() {
      selectedButtonClicks += 1;
      selectedButtonLabel = "Mute";
      selectedVideo.muted = false;
    },
  };
  let nonSelectedSliderReads = 0;
  let selectedSliderReads = 0;
  const nonSelectedSlider = {
    get valueAsNumber() {
      nonSelectedSliderReads += 1;
      return 0.9;
    },
  };
  const selectedSlider = {
    get valueAsNumber() {
      selectedSliderReads += 1;
      return 0.35;
    },
  };
  const multiPlayerPage = createPage(
    [nonSelectedVideo, selectedVideo],
    null,
    nonSelectedButton,
    nonSelectedSlider,
  );
  multiPlayerPage.setPlayerControls(
    nonSelectedVideo,
    nonSelectedButton,
    nonSelectedSlider,
  );
  multiPlayerPage.setPlayerControls(
    selectedVideo,
    selectedButton,
    selectedSlider,
  );
  let multiPlayerResponse;
  multiPlayerPage.onMessage(
    { type: "set_player_audio_preference", login: "streamer", unmuted: true },
    {},
    (value) => { multiPlayerResponse = value; },
  );
  assert.equal(multiPlayerResponse.unmuted, true);
  assert.equal(nonSelectedButtonClicks, 0);
  assert.equal(selectedButtonClicks, 1);
  assert.equal(nonSelectedVideo.muted, true);
  assert.equal(selectedVideo.muted, false);
  assert.equal(nonSelectedSliderReads, 0);
  assert.ok(selectedSliderReads > 0);
  assert.equal(selectedVideo.volume, 0.35);
  nonSelectedButton.click();
  multiPlayerPage.signalManualAudioChange();
  assert.equal(nonSelectedButtonClicks, 1);
  selectedVideo.muted = true;
  selectedButtonLabel = "Unmute";
  multiPlayerPage.intervals[0].callback();
  assert.equal(nonSelectedButtonClicks, 1);
  assert.equal(selectedButtonClicks, 2);
  assert.equal(selectedVideo.muted, false);
  chatPage.setSurface("player");
  const returnedVideo = makeVideo(true);
  chatPage.replaceVideos([returnedVideo]);
  chatPage.intervals[0].callback();
  assert.equal(returnedVideo.muted, true);
});
test("confirms chat messages from transcript echoes without sending message text", () => {
  const sent = [];
  const listeners = new Map();
  const chatInput = {
    value: "",
    closest(selector) {
      return selector === '[data-a-target="chat-input"]' ? this : null;
    },
  };
  const chatMessageList = {};
  let onMessage;
  let chatObserver;
  const document = {
    addEventListener(type, listener) {
      listeners.set(type, listener);
    },
    querySelectorAll: () => [],
    querySelector(selector) {
      return selector === '[data-test-selector="chat-scrollable-area__message-container"], [data-a-target="chat-message-list"]'
        ? chatMessageList
        : chatInput;
    },
  };
  class FakeMutationObserver {
    constructor(callback) {
      this.callback = callback;
      chatObserver = this;
    }
    observe() {}
    disconnect() {}
  }
  const context = vm.createContext({
    chrome: {
      runtime: {
        sendMessage(message) {
          sent.push(message);
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
    document,
    MutationObserver: FakeMutationObserver,
    HTMLMediaElement: { HAVE_FUTURE_DATA: 3 },
    location: { href: "https://www.twitch.tv/streamer" },
    TwitchTabUtils: {
      channelRoute: () => ({ login: "streamer", surface: "player" }),
    },
    TwitchQuality,
    window: {
      opener: null,
      sessionStorage: {
        getItem: () => null,
        removeItem() {},
        setItem() {},
      },
      setInterval: () => 1,
      clearInterval() {},
      setTimeout: () => 1,
    },
  });
  const source = fs.readFileSync(path.join(__dirname, "content.js"), "utf8");
  vm.runInContext(source, context, { filename: "content.js" });
  onMessage({ type: "start_monitoring", viewer_login: "viewer" }, {}, () => {});

  function addChatLine(text, authorLogin = "viewer") {
    const body = { textContent: text };
    const line = {
      nodeType: 1,
      matches(selector) {
        return selector === '[data-a-target="chat-line-message"]';
      },
      closest(selector) {
        return this.matches(selector) ? this : null;
      },
      getAttribute(name) {
        return name === "data-a-user" ? authorLogin : null;
      },
      querySelector(selector) {
        return selector === '[data-a-target="chat-line-message-body"]' ? body : null;
      },
      querySelectorAll: () => [],
    };
    chatObserver.callback([{ addedNodes: [line] }]);
  }
  const submissions = () => sent.filter((message) => message.chat_message_author_login);
  const keydown = listeners.get("keydown");

  chatInput.value = "first message";
  keydown({ key: "Enter", target: chatInput });
  assert.equal(submissions().length, 0);
  addChatLine("unrelated message");
  assert.equal(submissions().length, 0);
  addChatLine("first message", "another_viewer");
  assert.equal(submissions().length, 0);
  addChatLine("first message");
  assert.equal(submissions().length, 1);

  chatInput.value = "second message";
  keydown({ key: "Enter", target: chatInput });
  addChatLine("second message");
  assert.equal(submissions().length, 2);

  chatInput.value = "failed send";
  keydown({ key: "Enter", target: chatInput });
  assert.equal(submissions().length, 2);
  chatInput.value = "multiline";
  keydown({ key: "Enter", shiftKey: true, target: chatInput });
  addChatLine("multiline");
  assert.equal(submissions().length, 2);
  chatInput.value = "";
  keydown({ key: "Enter", target: chatInput });
  assert.equal(submissions().length, 2);

  chatInput.value = "button message";
  const sendButton = {
    disabled: false,
    getAttribute: () => null,
  };
  listeners.get("click")({
    target: {
      closest(selector) {
        return selector === '[data-a-target="chat-send-button"]' ? sendButton : null;
      },
    },
  });
  assert.equal(submissions().length, 2);
  addChatLine("button message");
  assert.equal(submissions().length, 3);

  for (const submission of submissions()) {
    assert.equal(submission.type, "channel_status");
    assert.equal(submission.login, "streamer");
    assert.equal(submission.chat_state, "ready");
    assert.equal(Object.hasOwn(submission, "text"), false);
    assert.equal(Object.hasOwn(submission, "message"), false);
    assert.equal(submission.chat_message_author_login, "viewer");
  }
});
