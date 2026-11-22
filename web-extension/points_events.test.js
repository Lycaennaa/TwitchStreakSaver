const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");
const { parsePointsNotification, parseTopicViewerId } = require("./points_events.js");

const pointsEventSource = fs.readFileSync(path.join(__dirname, "points_events.js"), "utf8");
const pointsEventBridgeSource = fs.readFileSync(path.join(__dirname, "points_events_bridge.js"), "utf8");
const TOPIC_PREFIX = "community-points-user-v1.";
const BRIDGE_CHANNEL_EVENT = "tss-points-event-bridge";

function createPointsPage() {
  const messages = [];
  const sockets = [];
  const listeners = new Map();
  const transferredPorts = [];
  let nextPageId = 0;
  class FakeEvent {
    constructor(type, { isTrusted = false } = {}) {
      this.type = type;
      this.trusted = isTrusted;
    }
  }
  Object.defineProperty(FakeEvent.prototype, "isTrusted", {
    configurable: true,
    get() { return this.trusted; },
  });
  class FakeMessageEvent extends FakeEvent {
    constructor(type, { data, ports = [], isTrusted = false } = {}) {
      super(type, { isTrusted });
      this.messageData = data;
      this.messagePorts = ports;
    }
  }
  Object.defineProperties(FakeMessageEvent.prototype, {
    data: { get() { return this.messageData; } },
    ports: { get() { return this.messagePorts; } },
  });
  class FakeEventTarget {
    constructor() {
      this.listeners = new Map();
    }

    addEventListener(type, listener) {
      if (!this.listeners.has(type)) this.listeners.set(type, []);
      this.listeners.get(type).push(listener);
    }

    dispatchEvent(event) {
      for (const listener of this.listeners.get(event.type) ?? []) listener(event);
      return true;
    }
  }
  class FakeWebSocket extends FakeEventTarget {
    constructor(url) {
      super();
      this.url = url;
      this.sent = [];
      sockets.push(this);
    }

    send(data) {
      this.sent.push(data);
    }

    dispatch(type, event) {
      this.dispatchEvent(new FakeMessageEvent(type, {
        ...event,
        isTrusted: event.isTrusted ?? true,
      }));
    }
  }
  class FakeURL {
    constructor(value, base) {
      const parsed = new URL(value, base);
      this.hostnameValue = parsed.hostname;
      this.href = parsed.href;
    }
  }
  Object.defineProperty(FakeURL.prototype, "hostname", {
    configurable: true,
    get() { return this.hostnameValue; },
  });
  class FakeMessagePort {
    constructor(onPost) {
      this.onPost = onPost;
    }

    start() {}

    postMessage(message) {
      this.onPost(message);
    }
  }
  class FakeMessageChannel {
    constructor() {
      this.port1 = new FakeMessagePort((message) => {
        messages.push(JSON.parse(JSON.stringify(message)));
      });
      this.port2 = new FakeMessagePort(() => {});
    }
  }
  const context = vm.createContext({
    WebSocket: FakeWebSocket,
    URL: FakeURL,
    EventTarget: FakeEventTarget,
    Event: FakeEvent,
    MessageEvent: FakeMessageEvent,
    MessagePort: FakeMessagePort,
    MessageChannel: FakeMessageChannel,
    location: { href: "https://www.twitch.tv/example", origin: "https://www.twitch.tv" },
    crypto: { randomUUID: () => `page-${++nextPageId}` },
    addEventListener(type, listener) {
      if (!listeners.has(type)) listeners.set(type, []);
      listeners.get(type).push(listener);
    },
    removeEventListener(type, listener) {
      listeners.set(type, (listeners.get(type) ?? []).filter((candidate) => candidate !== listener));
    },
    dispatchEvent(event) {
      for (const listener of [...(listeners.get(event.type) ?? [])]) listener(event);
      return true;
    },
  });
  context.window = context;
  context.addEventListener(BRIDGE_CHANNEL_EVENT, (event) => {
    if (typeof event.data !== "string") return;
    const message = JSON.parse(event.data);
    if (message.type === "connect") transferredPorts.push(...event.ports);
  });
  vm.runInContext(pointsEventSource, context, { filename: "points_events.js" });
  context.dispatchEvent(new FakeMessageEvent(BRIDGE_CHANNEL_EVENT, {
    data: JSON.stringify({ type: "challenge", token: "bridge-token" }),
  }));
  return { context, messages, sockets, transferredPorts };
}

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
  };
  class FakeMessageEvent {
    constructor(type, { data, ports = [] } = {}) {
      this.type = type;
      this.data = data;
      this.ports = ports;
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
  const challengeEvent = dispatchedEvents.find((event) => event.type === BRIDGE_CHANNEL_EVENT);
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
    connectMainWorld(token = challengeToken, port = pagePort) {
      window.dispatchEvent(new FakeMessageEvent(BRIDGE_CHANNEL_EVENT, {
        data: JSON.stringify({ type: "connect", token }),
        ports: [port],
      }));
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


test("parses earned, spent, and manual claim notices from Twitch Hermes payloads", () => {
  assert.deepEqual(parsePointsNotification({
    type: "points-earned",
    data: {
      point_gain: { reason_code: "WATCH_STREAK", total_points: 400 },
      balance: { channel_id: "98765", balance: 1400 },
    },
  }, "12345"), {
    type: "points_earned",
    channel_id: "98765",
    points_earned: 400,
    balance: 1400,
    reason_code: "WATCH_STREAK",
  });
  assert.deepEqual(parsePointsNotification({
    type: "points-spent",
    data: { balance: { channel_id: "98765", balance: 80 } },
  }, "12345"), {
    type: "points_spent",
    channel_id: "98765",
    balance: 80,
  });
  assert.deepEqual(parsePointsNotification({
    type: "claim-available",
    data: { claim: { channel_id: "98765" } },
  }, "12345"), {
    type: "claim_available",
    channel_id: "98765",
  });
});

test("uses the top-level earned channel id when balance omits it and rejects conflicts", () => {
  assert.deepEqual(parsePointsNotification({
    type: "points-earned",
    data: {
      channel_id: "98765",
      point_gain: { reason_code: "WATCH_STREAK", total_points: 50 },
      balance: { balance: 1050 },
    },
  }, "12345"), {
    type: "points_earned",
    channel_id: "98765",
    points_earned: 50,
    balance: 1050,
    reason_code: "WATCH_STREAK",
  });
  assert.equal(parsePointsNotification({
    type: "points-earned",
    data: {
      channel_id: "98765",
      point_gain: { reason_code: "WATCH_STREAK", total_points: 50 },
      balance: { channel_id: "54321", balance: 1050 },
    },
  }, "12345"), null);
});

test("rejects malformed point payloads instead of inventing account state", () => {
  assert.equal(parseTopicViewerId(`${TOPIC_PREFIX}viewer-123`), null);
  assert.equal(parseTopicViewerId(`${TOPIC_PREFIX}123`), "123");
  assert.equal(parsePointsNotification({
    type: "points-earned",
    data: {
      point_gain: { reason_code: "WATCH", total_points: Number.MAX_SAFE_INTEGER + 1 },
      balance: { channel_id: "98765", balance: 1400 },
    },
  }, "12345"), null);
  assert.equal(parsePointsNotification({
    type: "points-spent",
    data: { balance: { channel_id: "not-a-channel-id", balance: 0 } },
  }, "12345"), null);
  assert.equal(parsePointsNotification({ type: "other", data: {} }, "12345"), null);
});

test("forwards only events for an acknowledged Hermes Points subscription", () => {
  const page = createPointsPage();
  page.context.MessagePort.prototype.postMessage = () => {
    throw new Error("page replaced the port sender");
  };
  const socket = new page.context.WebSocket("wss://hermes.twitch.tv/ws");
  socket.send(JSON.stringify({ type: "authenticate", authenticate: {} }));
  assert.equal(page.messages.length, 0);

  socket.send(JSON.stringify({
    id: "request-1",
    type: "subscribe",
    subscribe: {
      id: "subscription-request-1",
      type: "pubsub",
      pubsub: { topic: `${TOPIC_PREFIX}12345` },
    },
  }));
  socket.dispatch("message", { data: JSON.stringify({
    type: "response",
    parentId: "request-1",
    subscribeResponse: { subscription: { id: "subscription-1" }, result: "ok" },
  }) });
  assert.equal(page.transferredPorts.length, 1);
  assert.deepEqual(page.messages[0], {
    kind: "source",
    source_id: "page-1:1",
    viewer_id: "12345",
    connected: true,
  });

  const pointsNotification = {
    type: "notification",
    notification: {
      type: "pubsub",
      subscription: { id: "subscription-1" },
      pubsub: {
        type: "points-earned",
        data: {
          point_gain: { reason_code: "WATCH_STREAK", total_points: 400 },
          balance: { channel_id: "98765", balance: 1400 },
        },
      },
    },
  };
  Object.defineProperty(page.context.Event.prototype, "isTrusted", {
    configurable: true,
    get() { return true; },
  });
  const nativeJsonParse = vm.runInContext("JSON.parse", page.context);
  page.context.originalJsonParse = nativeJsonParse;
  page.context.forgedPointsNotification = pointsNotification;
  vm.runInContext("JSON.parse = () => forgedPointsNotification", page.context);
  socket.dispatch("message", { data: '{"points-earned":', isTrusted: true });
  assert.equal(page.messages.length, 1);
  vm.runInContext("JSON.parse = originalJsonParse", page.context);
  delete page.context.originalJsonParse;
  delete page.context.forgedPointsNotification;
  socket.dispatch("message", {
    data: JSON.stringify(pointsNotification),
    isTrusted: false,
  });
  assert.equal(page.messages.length, 1);
  socket.dispatch("message", { data: JSON.stringify(pointsNotification) });
  assert.deepEqual(page.messages[1], {
    kind: "event",
    source_id: "page-1:1",
    viewer_id: "12345",
    event: {
      type: "points_earned",
      channel_id: "98765",
      points_earned: 400,
      balance: 1400,
      reason_code: "WATCH_STREAK",
    },
  });

  socket.dispatch("message", { data: JSON.stringify({
    type: "response",
    parentId: "request-2",
    subscribeResponse: { subscription: { id: "rejected" }, result: "error" },
  }) });
  socket.dispatch("message", { data: JSON.stringify({
    type: "notification",
    notification: {
      type: "pubsub",
      subscription: { id: "rejected" },
      pubsub: { type: "points-spent", data: { balance: { channel_id: "98765", balance: 0 } } },
    },
  }) });
  assert.equal(page.messages.length, 2);

  socket.dispatch("close", {});
  assert.deepEqual(page.messages[2], {
    kind: "source",
    source_id: "page-1:1",
    viewer_id: "12345",
    connected: false,
  });
});

test("forwards a JSON-encoded pubsub payload for an acknowledged Points subscription", () => {
  const page = createPointsPage();
  const socket = new page.context.WebSocket("wss://hermes.twitch.tv/ws");
  socket.send(JSON.stringify({
    id: "request-1",
    type: "subscribe",
    subscribe: {
      id: "subscription-request-1",
      type: "pubsub",
      pubsub: { topic: `${TOPIC_PREFIX}12345` },
    },
  }));
  socket.dispatch("message", { data: JSON.stringify({
    type: "response",
    parentId: "request-1",
    subscribeResponse: { subscription: { id: "subscription-1" }, result: "ok" },
  }) });
  socket.dispatch("message", { data: JSON.stringify({
    type: "notification",
    notification: {
      type: "pubsub",
      subscription: { id: "subscription-1" },
      pubsub: JSON.stringify({
        type: "points-earned",
        data: {
          point_gain: { reason_code: "CLAIM", total_points: 50 },
          balance: { channel_id: "98765", balance: 1450 },
        },
      }),
    },
  }) });

  assert.deepEqual(page.messages[1].event, {
    type: "points_earned",
    channel_id: "98765",
    points_earned: 50,
    balance: 1450,
    reason_code: "CLAIM",
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
  bridge.sendPortMessage(event);
  assert.equal(bridge.sent.length, 3);
});
