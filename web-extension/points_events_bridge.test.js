const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");

const pointsEventBridgeSource = fs.readFileSync(path.join(__dirname, "points_events_bridge.js"), "utf8");
const BRIDGE_CHANNEL_EVENT = "tss-points-event-bridge";

function createPointsBridge() {
  const sent = [];
  const listeners = new Map();
  const dispatchedEvents = [];
  let runtimeListener;
  const window = {
    location: { origin: "https://www.twitch.tv" },
    addEventListener(type, listener) {
      if (!listeners.has(type)) listeners.set(type, []);
      listeners.get(type).push(listener);
    },
    removeEventListener(type, listener) {
      listeners.set(type, (listeners.get(type) ?? []).filter((candidate) => candidate !== listener));
    },
    dispatchEvent(event) {
      dispatchedEvents.push(event);
      for (const listener of [...(listeners.get(event.type) ?? [])]) listener(event);
      return true;
    },
    postMessage(data, targetOrigin, transfer = []) {
      if (targetOrigin !== this.location.origin) return;
      this.dispatchEvent(new FakeMessageEvent("message", {
        data,
        ports: transfer,
        origin: this.location.origin,
        source: this,
      }));
    },
  };
  class FakeMessageEvent {
    constructor(type, { data, ports = [], origin = "", source = null } = {}) {
      this.type = type;
      this.data = data;
      this.ports = ports;
      this.origin = origin;
      this.source = source;
    }
  }
  const context = vm.createContext({
    browser: {
      runtime: {
        onMessage: {
          addListener(listener) {
            runtimeListener = listener;
          },
        },
        sendMessage(message) {
          sent.push(JSON.parse(JSON.stringify(message)));
          return Promise.resolve();
        },
      },
    },
    crypto: { randomUUID: () => "bridge-token" },
    MessageEvent: FakeMessageEvent,
    window,
  });
  vm.runInContext(pointsEventBridgeSource, context, { filename: "points_events_bridge.js" });
  const challengeEvent = dispatchedEvents.find((event) => {
    if (event.type !== "message" || typeof event.data !== "string") return false;
    const message = JSON.parse(event.data);
    return message.channel === BRIDGE_CHANNEL_EVENT && message.type === "challenge";
  });
  const challengeToken = JSON.parse(challengeEvent.data).token;
  const pagePort = { onmessage: null, start() {}, postMessage() {} };
  const requestSources = (documentToken) => {
    let response;
    runtimeListener({
      type: "get_points_event_sources",
      document_token: documentToken,
    }, {}, (value) => { response = value; });
    return response === undefined ? response : JSON.parse(JSON.stringify(response));
  };
  return {
    sent,
    challengeToken,
    challengeCount() {
      return dispatchedEvents.filter((event) => {
        if (event.type !== "message" || typeof event.data !== "string") return false;
        const message = JSON.parse(event.data);
        return message.channel === BRIDGE_CHANNEL_EVENT && message.type === "challenge";
      }).length;
    },
    signalMainWorldReady() {
      window.postMessage(JSON.stringify({
        channel: BRIDGE_CHANNEL_EVENT,
        type: "hook-ready",
      }), window.location.origin);
    },
    connectMainWorld(token = challengeToken, port = pagePort) {
      window.postMessage(JSON.stringify({
        channel: BRIDGE_CHANNEL_EVENT,
        type: "connect",
        token,
      }), window.location.origin, [port]);
    },
    sendPageMessage(data, origin = window.location.origin) {
      window.dispatchEvent({ type: "message", source: window, origin, data, ports: [] });
    },
    sendPortMessage(data, port = pagePort) {
      port.onmessage?.({ data });
    },
    setDocumentToken(documentToken = "document-token") {
      return requestSources(documentToken);
    },
    reportSources() {
      return requestSources("document-token");
    },
  };
}

test("bridge retries its challenge when the main-world hook starts later", () => {
  const bridge = createPointsBridge();
  assert.equal(bridge.challengeCount(), 1);

  bridge.signalMainWorldReady();
  assert.equal(bridge.challengeCount(), 2);

  bridge.connectMainWorld();
  assert.deepEqual(bridge.setDocumentToken(), {
    document_token: "document-token",
    sources: [],
  });
});

test("relays only validated events over the one-time private page bridge", () => {
  const bridge = createPointsBridge();
  const source = {
    kind: "source",
    source_id: "page-1:1",
    viewer_id: "12345",
    connected: true,
  };
  const event = {
    kind: "event",
    source_id: "page-1:1",
    viewer_id: "12345",
    event: { type: "claim_available", channel_id: "98765" },
  };

  bridge.sendPageMessage(source);
  assert.equal(bridge.sent.length, 0);
  bridge.connectMainWorld("wrong-token");
  bridge.sendPortMessage(source);
  assert.equal(bridge.sent.length, 0);

  bridge.connectMainWorld();
  bridge.sendPageMessage(source);
  bridge.sendPortMessage(source);
  assert.equal(bridge.sent.length, 0);
  assert.deepEqual(bridge.setDocumentToken(), {
    document_token: "document-token",
    sources: [{ source_id: "page-1:1", viewer_id: "12345" }],
  });
  assert.deepEqual(bridge.reportSources(), {
    document_token: "document-token",
    sources: [{ source_id: "page-1:1", viewer_id: "12345" }],
  });
  bridge.sendPortMessage(source);
  assert.deepEqual(bridge.sent[0], {
    type: "points_event_sources",
    document_token: "document-token",
    sources: [{ source_id: "page-1:1", viewer_id: "12345" }],
  });

  bridge.sendPortMessage({
    ...event,
    event: {
      type: "points_earned",
      channel_id: "98765",
      points_earned: -1,
      balance: 100,
      reason_code: "CLAIM",
    },
  });
  assert.equal(bridge.sent.length, 1);

  const replacementPort = { onmessage: null, start() {}, postMessage() {} };
  bridge.connectMainWorld(bridge.challengeToken, replacementPort);
  bridge.sendPortMessage(event, replacementPort);
  assert.equal(bridge.sent.length, 1);

  bridge.sendPortMessage(event);
  assert.deepEqual(bridge.sent[1], {
    type: "points_event",
    document_token: "document-token",
    source_id: "page-1:1",
    viewer_id: "12345",
    event: { type: "claim_available", channel_id: "98765" },
  });
  bridge.sendPageMessage(event, "https://attacker.invalid");
  assert.equal(bridge.sent.length, 2);

  bridge.sendPortMessage({ ...source, connected: false });
  assert.deepEqual(bridge.sent[2], {
    type: "points_event_sources",
    document_token: "document-token",
    sources: [],
  });
  assert.equal(bridge.sent.length, 3);
});
