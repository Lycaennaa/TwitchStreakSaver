(() => {
  let previousState = "";
  let lastSentAt = 0;
  let monitoring = false;
  let timer = null;

  function channelRoute() {
    return globalThis.TwitchTabUtils.channelRoute(location.href);
  }

  function playerState() {
    const video = Array.from(document.querySelectorAll("video"))
      .filter((item) => item.getClientRects().length > 0)
      .sort((left, right) => {
        const leftRect = left.getBoundingClientRect();
        const rightRect = right.getBoundingClientRect();
        return rightRect.width * rightRect.height - leftRect.width * leftRect.height;
      })[0];
    return !video
      ? "missing"
      : video.ended
        ? "ended"
        : video.paused
          ? "paused"
          : video.readyState < HTMLMediaElement.HAVE_FUTURE_DATA
            ? "buffering"
            : "playing";
  }

  function chatState() {
    const ready = document.querySelector(
      '[data-a-target="chat-input"], [data-test-selector="chat-scrollable-area__message-container"], [data-a-target="chat-message-list"]',
    );
    if (ready) return "ready";
    const loading = document.querySelector(
      '[data-test-selector="chat-room-component-layout"], .chat-room__content, iframe[src*="chat"]',
    );
    return loading ? "loading" : "missing";
  }

  function stopMonitoring() {
    monitoring = false;
    if (timer !== null) window.clearInterval(timer);
    timer = null;
    previousState = "";
  }

  function report() {
    if (!monitoring) return;
    const route = channelRoute();
    if (!route.login || !route.surface) return;
    const state = {
      type: "channel_status",
      surface: route.surface,
      login: route.login,
      player_state: playerState(),
      chat_state: chatState(),
    };
    const encoded = JSON.stringify(state);
    const now = Date.now();
    if (encoded === previousState && now - lastSentAt < 5000) return;
    previousState = encoded;
    lastSentAt = now;
    try {
      chrome.runtime.sendMessage(state).catch(() => {});
    } catch (error) {
      if (!String(error).toLowerCase().includes("extension context invalidated")) throw error;
      stopMonitoring();
    }
  }

  chrome.runtime.onMessage.addListener((message) => {
    if (message?.type === "start_monitoring" && !monitoring) {
      monitoring = true;
      report();
      if (monitoring) timer = window.setInterval(report, 2000);
    } else if (message?.type === "stop_monitoring" && monitoring) {
      stopMonitoring();
    }
  });
})();