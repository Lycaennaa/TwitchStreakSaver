const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

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
  assert.equal(sent.length, 1);
  assert.deepEqual(JSON.parse(JSON.stringify(sent[0])), {
    type: "channel_status",
    surface: "player",
    login: "streamer",
    player_state: "playing",
    chat_state: "ready",
  });
  assert.equal(intervalCount, 1);

  shouldThrow = true;
  video.paused = true;
  assert.doesNotThrow(() => intervalCallback());
  assert.equal(sent.length, 2);
  assert.deepEqual(clearedIntervals, [1]);
  intervalCallback();
  assert.equal(sent.length, 2);
});
