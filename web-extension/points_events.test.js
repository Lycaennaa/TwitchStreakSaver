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

function createPointsPage({ trustedGetter = true } = {}) {
  const messages = [];
  const sockets = [];
  const dispatchedEvents = [];
  const listeners = new Map();
  const transferredPorts = [];
  let nextPageId = 0;
  class FakeEvent {
    constructor(type, { isTrusted = false } = {}) {
      this.type = type;
      this.trusted = isTrusted;
      if (!trustedGetter) Object.defineProperty(this, "isTrusted", { value: isTrusted, configurable: true });
    }
  }
  if (trustedGetter) {
    Object.defineProperty(FakeEvent.prototype, "isTrusted", {
      configurable: true,
      get() { return this.trusted; },
    });
  }
  class FakeMessageEvent extends FakeEvent {
    constructor(type, { data, ports = [], isTrusted = false, origin = "", source = null } = {}) {
      super(type, { isTrusted });
      this.messageData = data;
      this.messagePorts = ports;
      this.messageOrigin = origin;
      this.messageSource = source;
    }
  }
  Object.defineProperties(FakeMessageEvent.prototype, {
    data: { get() { return this.messageData; } },
    ports: { get() { return this.messagePorts; } },
    origin: { get() { return this.messageOrigin; } },
    source: { get() { return this.messageSource; } },
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
        const cloned = JSON.parse(JSON.stringify(message));
        messages.push(cloned);
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
        isTrusted: true,
      }));
    },
  });
  context.window = context;
  context.addEventListener("message", (event) => {
    if (typeof event.data !== "string") return;
    const message = JSON.parse(event.data);
    if (message.channel === BRIDGE_CHANNEL_EVENT && message.type === "connect") {
      transferredPorts.push(...event.ports);
    }
  });
  vm.runInContext(pointsEventSource, context, { filename: "points_events.js" });
  context.postMessage(JSON.stringify({
    channel: BRIDGE_CHANNEL_EVENT,
    type: "challenge",
    token: "bridge-token",
  }), context.location.origin);
  return { context, messages, sockets, transferredPorts, dispatchedEvents };
}


test("main-world hook announces readiness after registering its message listener", () => {
  const page = createPointsPage();
  const ready = page.dispatchedEvents.find((event) => {
    if (event.type !== "message" || typeof event.data !== "string") return false;
    const message = JSON.parse(event.data);
    return message.channel === BRIDGE_CHANNEL_EVENT && message.type === "hook-ready";
  });

  assert.ok(ready);
});

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

test("supports event prototypes without an isTrusted getter", () => {
  const page = createPointsPage({ trustedGetter: false });
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
  socket.dispatch("message", {
    data: JSON.stringify({
      type: "response",
      parentId: "request-1",
      subscribeResponse: { subscription: { id: "subscription-1" }, result: "ok" },
    }),
    isTrusted: true,
  });

  assert.deepEqual(page.messages[0], {
    kind: "source",
    source_id: "page-1:1",
    viewer_id: "12345",
    connected: true,
  });
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

