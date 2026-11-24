var browserApi = globalThis.browser ?? globalThis.chrome;
(() => {
  const BRIDGE_CHANNEL_EVENT = "tss-points-event-bridge";
  const MAX_SOURCES = 64;
  const activeSources = new Map();
  const isRecord = (value) => value !== null && typeof value === "object" && !Array.isArray(value);
  const isViewerId = (value) => typeof value === "string" && /^\d{1,32}$/.test(value);
  const isSourceId = (value) => typeof value === "string" && value.length > 0 && value.length <= 128;
  const isChannelId = (value) => typeof value === "string" && /^\d{1,32}$/.test(value);
  const isPointsValue = (value) => Number.isSafeInteger(value) && value >= 0;
  const bridgeToken = crypto.randomUUID();
  let pagePort = null;
  let documentToken = null;


  function sendSources() {
    if (!documentToken) return;
    try {
      browserApi.runtime.sendMessage({
        type: "points_event_sources",
        document_token: documentToken,
        sources: Array.from(activeSources, ([sourceId, viewerId]) => ({ source_id: sourceId, viewer_id: viewerId })),
      }).catch(() => {});
    } catch {}
  }

  function isPointsEvent(event) {
    if (!isRecord(event) || !isChannelId(event.channel_id)) return false;
    if (event.type === "points_earned") {
      return isPointsValue(event.points_earned)
        && isPointsValue(event.balance)
        && typeof event.reason_code === "string"
        && /^[A-Z0-9_]{1,64}$/.test(event.reason_code);
    }
    if (event.type === "points_spent") return isPointsValue(event.balance);
    return event.type === "claim_available";
  }

  function onPageMessage(event) {
    const message = event.data;
    if (!isRecord(message)) return;
    if (!isSourceId(message.source_id) || !isViewerId(message.viewer_id)) return;

    if (message.kind === "source" && typeof message.connected === "boolean") {
      if (message.connected) {
        if (activeSources.size < MAX_SOURCES || activeSources.has(message.source_id)) {
          activeSources.set(message.source_id, message.viewer_id);
        }
      } else if (activeSources.get(message.source_id) === message.viewer_id) {
        activeSources.delete(message.source_id);
      }
      sendSources();
      return;
    }

    if (
      message.kind === "event"
      && documentToken
      && activeSources.get(message.source_id) === message.viewer_id
      && isPointsEvent(message.event)
    ) {
      try {
        browserApi.runtime.sendMessage({
          type: "points_event",
          document_token: documentToken,
          source_id: message.source_id,
          viewer_id: message.viewer_id,
          event: message.event,
        }).catch(() => {});
      } catch {}
    }
  }

  function postToPage(message, transfer = []) {
    try {
      window.postMessage(
        JSON.stringify({ channel: BRIDGE_CHANNEL_EVENT, ...message }),
        window.location.origin,
        transfer,
      );
    } catch {}
  }

  function dispatchChallenge() {
    postToPage({ type: "challenge", token: bridgeToken });
  }

  function onPageHandshake(event) {
    if (
      pagePort
      || event.origin !== window.location.origin
      || typeof event.data !== "string"
    ) return;
    let message;
    try { message = JSON.parse(event.data); } catch { return; }
    if (message?.channel !== BRIDGE_CHANNEL_EVENT) return;
    if (message.type === "hook-ready") {
      if (event.ports?.length === 0) dispatchChallenge();
      return;
    }
    if (
      message.type !== "connect"
      || message.token !== bridgeToken
      || event.ports?.length !== 1
    ) return;

    const [port] = event.ports;
    if (!port || typeof port.postMessage !== "function") return;
    pagePort = port;
    pagePort.onmessage = onPageMessage;
    pagePort.start();
    window.removeEventListener("message", onPageHandshake);
  }

  window.addEventListener("message", onPageHandshake);
  // Retry after the main-world hook announces readiness; static content-script ordering can vary.
  dispatchChallenge();

  browserApi.runtime.onMessage.addListener((message, _sender, sendResponse) => {
    if (message?.type !== "get_points_event_sources") return false;
    if (
      typeof message.document_token !== "string"
      || message.document_token.length === 0
      || message.document_token.length > 128
    ) return false;
    documentToken = message.document_token;
    sendResponse({
      document_token: documentToken,
      sources: Array.from(activeSources, ([sourceId, viewerId]) => ({ source_id: sourceId, viewer_id: viewerId })),
    });
    return false;
  });
})();
