(() => {
  const TOPIC_PREFIX = "community-points-user-v1.";
  const BRIDGE_CHANNEL_EVENT = "tss-points-event-bridge";
  const MAX_PENDING_SUBSCRIPTIONS = 64;
  const MAX_ACTIVE_SUBSCRIPTIONS = 64;
  const MAX_EVENT_FRAME_CHARS = 131_072;
  const MAX_SUBSCRIBE_FRAME_CHARS = 16_384;
  const nativeCall = Function.prototype.call;
  const nativeApply = Reflect.apply;
  const nativeConstruct = Reflect.construct;
  const nativeArrayIsArray = Array.isArray;
  const nativeIsSafeInteger = Number.isSafeInteger;
  const nativeJsonParse = JSON.parse;
  const nativeJsonStringify = JSON.stringify;
  const nativeSetPrototypeOf = Object.setPrototypeOf;
  const nativeStringIncludes = nativeCall.bind(String.prototype.includes);
  const nativeStringStartsWith = nativeCall.bind(String.prototype.startsWith);
  const nativeStringSlice = nativeCall.bind(String.prototype.slice);
  const nativeRegExpTest = nativeCall.bind(RegExp.prototype.test);
  const NativeMap = Map;
  const NativeSet = Set;
  const nativeMapGet = nativeCall.bind(NativeMap.prototype.get);
  const nativeMapSet = nativeCall.bind(NativeMap.prototype.set);
  const nativeMapDelete = nativeCall.bind(NativeMap.prototype.delete);
  const nativeMapClear = nativeCall.bind(NativeMap.prototype.clear);
  const nativeMapSize = nativeCall.bind(Object.getOwnPropertyDescriptor(NativeMap.prototype, "size").get);
  const nativeMapKeys = nativeCall.bind(NativeMap.prototype.keys);
  const nativeMapIteratorNext = nativeCall.bind(Object.getPrototypeOf(nativeMapKeys(new NativeMap())).next);
  const nativeSetHas = nativeCall.bind(NativeSet.prototype.has);
  const nativeSetAdd = nativeCall.bind(NativeSet.prototype.add);
  const nativeSetClear = nativeCall.bind(NativeSet.prototype.clear);
  const nativeSetForEach = nativeCall.bind(NativeSet.prototype.forEach);
  const isRecord = (value) => value !== null && typeof value === "object" && !nativeArrayIsArray(value);
  const isChannelId = (value) => typeof value === "string" && nativeRegExpTest(/^\d{1,32}$/, value);
  const isPointsValue = (value) => nativeIsSafeInteger(value) && value >= 0;
  const parseTopicViewerId = (topic) => {
    if (typeof topic !== "string" || !nativeStringStartsWith(topic, TOPIC_PREFIX)) return null;
    const viewerId = nativeStringSlice(topic, TOPIC_PREFIX.length);
    return nativeRegExpTest(/^\d{1,32}$/, viewerId) ? viewerId : null;
  };

  function parsePointsNotification(pubsub, viewerId) {
    if (!isRecord(pubsub) || !isRecord(pubsub.data) || !isChannelId(viewerId)) return null;
    const data = pubsub.data;
    if (pubsub.type === "points-earned") {
      const gain = data.point_gain;
      const balance = data.balance;
      if (!isRecord(gain) || !isRecord(balance)) return null;
      const channelId = balance.channel_id ?? data.channel_id;
      if (
        !isChannelId(channelId)
        || (isChannelId(balance.channel_id) && isChannelId(data.channel_id) && balance.channel_id !== data.channel_id)
        || !isPointsValue(gain.total_points)
        || !isPointsValue(balance.balance)
        || !nativeRegExpTest(/^[A-Z0-9_]{1,64}$/, gain.reason_code)
      ) return null;
      return {
        type: "points_earned",
        channel_id: channelId,
        points_earned: gain.total_points,
        balance: balance.balance,
        reason_code: gain.reason_code,
      };
    }
    if (pubsub.type === "points-spent") {
      const balance = data.balance;
      if (!isRecord(balance) || !isChannelId(balance.channel_id) || !isPointsValue(balance.balance)) {
        return null;
      }
      return {
        type: "points_spent",
        channel_id: balance.channel_id,
        balance: balance.balance,
      };
    }
    if (pubsub.type === "claim-available") {
      const claim = data.claim;
      if (!isRecord(claim) || !isChannelId(claim.channel_id)) return null;
      return { type: "claim_available", channel_id: claim.channel_id };
    }
    return null;
  }

  const parser = { parsePointsNotification, parseTopicViewerId };
  if (
    typeof module === "object"
    && module.exports
    && typeof process === "object"
    && process.versions?.node
  ) module.exports = parser;
  if (typeof window !== "object" || typeof window.WebSocket !== "function") return;
  const NativeWebSocket = window.WebSocket;
  const NativeURL = URL;
  const pageHref = window.location.href;
  const pageOrigin = window.location.origin;
  const nativeUrlHostname = nativeCall.bind(Object.getOwnPropertyDescriptor(NativeURL.prototype, "hostname").get);
  const nativeAddEventListener = nativeCall.bind(Object.getPrototypeOf(NativeWebSocket.prototype).addEventListener);
  function eventPropertyReader(prototype, name) {
    for (let current = prototype; current; current = Object.getPrototypeOf(current)) {
      const getter = Object.getOwnPropertyDescriptor(current, name)?.get;
      if (typeof getter === "function") return nativeCall.bind(getter);
    }
    return (event) => event[name];
  }
  const nativeIsTrustedEvent = eventPropertyReader(Event.prototype, "isTrusted");
  const nativeMessageEventData = eventPropertyReader(MessageEvent.prototype, "data");
  const nativeMessageEventPorts = eventPropertyReader(MessageEvent.prototype, "ports");
  const NativeMessageChannel = MessageChannel;
  const nativeWindowPostMessage = window.postMessage;
  const nativePortPostMessage = nativeCall.bind(MessagePort.prototype.postMessage);
  const nativePortStart = nativeCall.bind(MessagePort.prototype.start);
  const patchKey = Symbol.for("twitch-streak-saver.points-events-patched");
  if (window[patchKey]) return;
  window[patchKey] = true;
  function parseJson(text) {
    if (typeof text !== "string") return null;
    try {
      return nativeJsonParse(text, (_key, value) => {
        if (value !== null && typeof value === "object" && !nativeArrayIsArray(value)) {
          return nativeSetPrototypeOf(value, null);
        }
        return value;
      });
    } catch {
      return null;
    }
  }

  let extensionPort = null;


  function postToPage(message, transfer = []) {
    try {
      nativeApply(nativeWindowPostMessage, window, [
        nativeJsonStringify({ channel: BRIDGE_CHANNEL_EVENT, ...message }),
        pageOrigin,
        transfer,
      ]);
    } catch {}
  }

  function onBridgeMessage(event) {
    if (extensionPort || event.origin !== pageOrigin) return;
    let data;
    let ports;
    try {
      data = nativeMessageEventData(event);
      ports = nativeMessageEventPorts(event);
    } catch {
      return;
    }
    if (ports.length !== 0 || typeof data !== "string") return;
    const challenge = parseJson(data);
    if (
      challenge?.channel !== BRIDGE_CHANNEL_EVENT
      || challenge.type !== "challenge"
      || typeof challenge.token !== "string"
      || challenge.token.length > 128
    ) return;
    window.removeEventListener("message", onBridgeMessage);
    const channel = new NativeMessageChannel();
    extensionPort = channel.port1;
    nativePortStart(extensionPort);
    postToPage({ type: "connect", token: challenge.token }, [channel.port2]);
  }

  window.addEventListener("message", onBridgeMessage);
  postToPage({ type: "hook-ready" });

  function postToExtension(message) {
    try {
      if (extensionPort) nativePortPostMessage(extensionPort, message);
    } catch {}
  }

  const nativeWebSocketSend = NativeWebSocket.prototype.send;
  const pageId = crypto.randomUUID();
  let nextSocketId = 0;
  const WebSocketWithPointsEvents = new Proxy(NativeWebSocket, {
    construct(target, args, newTarget) {
      const socket = nativeConstruct(target, args, newTarget);
      let hostname = "";
      try { hostname = nativeUrlHostname(new NativeURL(args[0], pageHref)); } catch {}
      if (hostname !== "hermes.twitch.tv") return socket;

      const sourceId = `${pageId}:${++nextSocketId}`;
      const pendingTopics = new NativeMap();
      const subscriptions = new NativeMap();
      const activeViewerIds = new NativeSet();
      nativeAddEventListener(socket, "message", (event) => {
        let data;
        try {
          if (!nativeIsTrustedEvent(event)) return;
          data = nativeMessageEventData(event);
        } catch {
          return;
        }
        if (
          typeof data !== "string"
          || data.length > MAX_EVENT_FRAME_CHARS
          || !(
            nativeStringIncludes(data, '"subscribeResponse"')
            || nativeStringIncludes(data, "points-earned")
            || nativeStringIncludes(data, "points-spent")
            || nativeStringIncludes(data, "claim-available")
          )
        ) return;
        const frame = parseJson(data);
        if (!isRecord(frame)) return;
        const response = frame.subscribeResponse;
        if (isRecord(response)) {
          const viewerId = nativeMapGet(pendingTopics, frame.parentId);
          nativeMapDelete(pendingTopics, frame.parentId);
          if (!viewerId) return;
          if (response.result !== "ok") {
            return;
          }
          const subscriptionId = response.subscription?.id;
          if (
            typeof subscriptionId === "string"
            && nativeMapSize(subscriptions) < MAX_ACTIVE_SUBSCRIPTIONS
          ) {
            nativeMapSet(subscriptions, subscriptionId, viewerId);
            if (!nativeSetHas(activeViewerIds, viewerId)) {
              nativeSetAdd(activeViewerIds, viewerId);
              postToExtension({
                kind: "source",
                source_id: sourceId,
                viewer_id: viewerId,
                connected: true,
              });
            }
          }
          return;
        }

        const notification = frame.notification;
        if (!isRecord(notification) || notification.type !== "pubsub") return;
        const viewerId = nativeMapGet(subscriptions, notification.subscription?.id);
        if (!viewerId) return;
        const pubsub = typeof notification.pubsub === "string"
          ? parseJson(notification.pubsub)
          : notification.pubsub;
        const pointsEvent = parsePointsNotification(pubsub, viewerId);
        if (pointsEvent) {
          postToExtension({
            kind: "event",
            source_id: sourceId,
            viewer_id: viewerId,
            event: pointsEvent,
          });
        }
      });

      socket.send = function sendWithPointsSubscription(data) {
        if (
          typeof data === "string"
          && data.length <= MAX_SUBSCRIBE_FRAME_CHARS
          && nativeStringIncludes(data, TOPIC_PREFIX)
        ) {
          const message = parseJson(data);
          const topic = message?.subscribe?.pubsub?.topic;
          const viewerId = parseTopicViewerId(topic);
          if (
            message?.type === "subscribe"
            && typeof message.id === "string"
            && viewerId
          ) {
            nativeMapSet(pendingTopics, message.id, viewerId);
            if (nativeMapSize(pendingTopics) > MAX_PENDING_SUBSCRIPTIONS) {
              const oldestId = nativeMapIteratorNext(nativeMapKeys(pendingTopics)).value;
              nativeMapDelete(pendingTopics, oldestId);
            }
          }
        }
        return nativeApply(nativeWebSocketSend, this, arguments);
      };

      nativeAddEventListener(socket, "close", (event) => {
        if (!nativeIsTrustedEvent(event)) return;
        nativeSetForEach(activeViewerIds, (viewerId) => {
          postToExtension({
            kind: "source",
            source_id: sourceId,
            viewer_id: viewerId,
            connected: false,
          });
        });
        nativeSetClear(activeViewerIds);
        nativeMapClear(subscriptions);
        nativeMapClear(pendingTopics);
      });
      return socket;
    },
  });
  window.WebSocket = WebSocketWithPointsEvents;
})();
