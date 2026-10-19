(() => {
  const MANAGED_TAB_IDENTITY_KEY = "__twitch_streak_saver_managed_tab_identity__";
  let previousState = "";
  let lastSentAt = 0;
  let monitoring = false;
  let timer = null;
  const QUALITY_CHECK_INTERVAL_MS = 1000;
  const QUALITY_MENU_SETTLE_MS = 250;
  const QUALITY_RETRY_DELAYS_MS = [1000, 2000, 4000, 8000, 16000];
  const QUALITY_STORAGE_KEY = "preferredQuality";
  let preferredQuality = TwitchQuality.DEFAULT_QUALITY;
  let qualityPreferenceReady = false;
  let qualityPreferenceChangedWhileLoading = false;
  let qualityPreferenceRevision = 0;
  let qualityApplyInProgress = false;
  let playerPreferenceWatcher = null;
  let observedQualityRoute = location.href;
  let observedQualityVideo = null;
  let appliedQualityRoute = "";
  let appliedQualityVideo = null;
  let appliedQualityPreference = "";
  let qualityRetryCount = 0;
  let qualityRetryAfter = 0;
  let playerAudioPreference = null;
  let appliedPlayerAudioVideo = null;
  let appliedPlayerAudioLogin = "";


  function readManagedTabIdentity() {
    try {
      if (window.opener) {
        window.sessionStorage.removeItem(MANAGED_TAB_IDENTITY_KEY);
        return null;
      }
      return window.sessionStorage.getItem(MANAGED_TAB_IDENTITY_KEY);
    } catch {
      return null;
    }
  }
  function channelRoute() {
    return globalThis.TwitchTabUtils.channelRoute(location.href);
  }

  function visiblePlayerVideo() {
    return Array.from(document.querySelectorAll("video"))
      .filter((item) => item.getClientRects().length > 0)
      .sort((left, right) => {
        const leftRect = left.getBoundingClientRect();
        const rightRect = right.getBoundingClientRect();
        return rightRect.width * rightRect.height - leftRect.width * leftRect.height;
      })[0] || null;
  }
  function checkPlayerAudioPreference() {
    const route = channelRoute();
    if (route.surface !== "player" || !route.login) {
      playerAudioPreference = null;
      appliedPlayerAudioVideo = null;
      appliedPlayerAudioLogin = "";
      return;
    }
    const login = route.login.toLowerCase();
    if (playerAudioPreference?.login !== login) {
      playerAudioPreference = null;
      appliedPlayerAudioVideo = null;
      appliedPlayerAudioLogin = "";
      return;
    }
    const video = visiblePlayerVideo();
    if (!video) return;
    if (video === appliedPlayerAudioVideo && login === appliedPlayerAudioLogin) return;
    video.muted = !playerAudioPreference.unmuted;
    appliedPlayerAudioVideo = video;
    appliedPlayerAudioLogin = login;
  }

  function setPlayerAudioPreference(unmuted, login) {
    const route = channelRoute();
    const normalizedLogin = typeof login === "string" ? login.toLowerCase() : "";
    if (
      route.surface !== "player"
      || !route.login
      || route.login.toLowerCase() !== normalizedLogin
    ) return false;
    playerAudioPreference = { login: normalizedLogin, unmuted };
    appliedPlayerAudioVideo = null;
    appliedPlayerAudioLogin = "";
    checkPlayerAudioPreference();
    return true;
  }
  function playerState() {
    const video = visiblePlayerVideo();
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
  function visibleElement(selector) {
    return Array.from(document.querySelectorAll(selector))
      .find((element) => element.getClientRects().length > 0) || null;
  }

  function qualityMenuIsOpen() {
    return visibleElement('[role="menu"]') !== null;
  }

  function closeQualityMenu() {
    const menu = visibleElement('[role="menu"]');
    if (!menu) return;
    const closeButton = Array.from(menu.querySelectorAll("button,[role=\"menuitem\"]"))
      .find((element) => {
        const label = (element.getAttribute("aria-label") || "").trim().toLowerCase();
        const target = (element.getAttribute("data-a-target") || "").toLowerCase();
        const text = (element.innerText || "").trim().toLowerCase();
        return label === "close" || target.includes("close") || text === "close";
      });
    if (closeButton) {
      closeButton.click();
      return;
    }
    const eventOptions = {
      key: "Escape",
      code: "Escape",
      keyCode: 27,
      which: 27,
      bubbles: true,
    };
    menu.dispatchEvent(new KeyboardEvent("keydown", eventOptions));
    menu.dispatchEvent(new KeyboardEvent("keyup", eventOptions));
  }

  function qualityRequestIsCurrent(video, route, quality, revision) {
    return qualityPreferenceReady
      && preferredQuality === quality
      && qualityPreferenceRevision === revision
      && location.href === route
      && visiblePlayerVideo() === video;
  }

  function resetQualityRetry() {
    qualityRetryCount = 0;
    qualityRetryAfter = 0;
  }

  function deferQualityRetry(video, route, quality, revision) {
    if (!qualityRequestIsCurrent(video, route, quality, revision)) return;
    if (qualityRetryCount >= QUALITY_RETRY_DELAYS_MS.length) {
      qualityRetryCount += 1;
      return;
    }
    qualityRetryAfter = Date.now() + QUALITY_RETRY_DELAYS_MS[qualityRetryCount];
    qualityRetryCount += 1;
  }



  async function applyPreferredQuality(video, route, quality, revision) {
    qualityApplyInProgress = true;
    let openedSettings = false;
    try {
      if (qualityMenuIsOpen()) return;
      const settingsButton = visibleElement('button[data-a-target="player-settings-button"]');
      if (!settingsButton) {
        deferQualityRetry(video, route, quality, revision);
        return;
      }
      settingsButton.click();
      openedSettings = true;
      await new Promise((resolve) => window.setTimeout(resolve, QUALITY_MENU_SETTLE_MS));
      if (!qualityRequestIsCurrent(video, route, quality, revision)) return;

      const qualityButton = visibleElement('[data-a-target="player-settings-menu-item-quality"]');
      if (!qualityButton) {
        deferQualityRetry(video, route, quality, revision);
        return;
      }
      qualityButton.click();
      await new Promise((resolve) => window.setTimeout(resolve, QUALITY_MENU_SETTLE_MS));
      if (!qualityRequestIsCurrent(video, route, quality, revision)) return;

      const options = Array.from(document.querySelectorAll('[role="menuitemradio"]'))
        .filter((item) => item.getClientRects().length > 0);
      const option = TwitchQuality.findOption(options, quality);
      if (!option) {
        deferQualityRetry(video, route, quality, revision);
        return;
      }
      option.click();
      await new Promise((resolve) => window.setTimeout(resolve, QUALITY_MENU_SETTLE_MS));
      if (!qualityRequestIsCurrent(video, route, quality, revision)) return;

      appliedQualityVideo = video;
      appliedQualityRoute = route;
      appliedQualityPreference = quality;
      resetQualityRetry();
    } catch (error) {
      if (String(error).toLowerCase().includes("extension context invalidated")) {
        stopPlayerPreferenceAutomation();
      } else {
        if (openedSettings) deferQualityRetry(video, route, quality, revision);
        console.debug("Could not apply the preferred Twitch quality.", error);
      }
    } finally {
      if (openedSettings) closeQualityMenu();
      qualityApplyInProgress = false;
    }
  }

  function checkPreferredQuality() {
    const route = location.href;
    const video = visiblePlayerVideo();
    if (route !== observedQualityRoute || video !== observedQualityVideo) {
      observedQualityRoute = route;
      observedQualityVideo = video;
      qualityPreferenceRevision += 1;
      appliedQualityRoute = "";
      appliedQualityVideo = null;
      appliedQualityPreference = "";
      resetQualityRetry();
    }
    if (!qualityPreferenceReady || !video || qualityApplyInProgress) return;
    if (
      appliedQualityVideo === video
      && appliedQualityRoute === route
      && appliedQualityPreference === preferredQuality
    ) return;
    if (qualityMenuIsOpen()) {
      resetQualityRetry();
      return;
    }
    if (
      qualityRetryCount > QUALITY_RETRY_DELAYS_MS.length
      || Date.now() < qualityRetryAfter
    ) return;
    void applyPreferredQuality(video, route, preferredQuality, qualityPreferenceRevision);
  }
  function checkPlayerPreferences() {
    checkPreferredQuality();
    checkPlayerAudioPreference();
  }

  function stopPlayerPreferenceAutomation() {
    if (playerPreferenceWatcher !== null) window.clearInterval(playerPreferenceWatcher);
    playerPreferenceWatcher = null;
    qualityPreferenceRevision += 1;
  }

  async function loadPreferredQuality() {
    try {
      const stored = await chrome.storage.local.get({
        [QUALITY_STORAGE_KEY]: TwitchQuality.DEFAULT_QUALITY,
      });
      if (!qualityPreferenceChangedWhileLoading) {
        preferredQuality = TwitchQuality.normalizeQuality(stored[QUALITY_STORAGE_KEY]);
      }
    } catch {
      if (!qualityPreferenceChangedWhileLoading) {
        preferredQuality = TwitchQuality.DEFAULT_QUALITY;
      }
    }
    if (qualityPreferenceChangedWhileLoading) return;
    qualityPreferenceReady = true;
    qualityPreferenceRevision += 1;
    checkPreferredQuality();
  }

  function qualityPreferenceChanged(changes, areaName) {
    const change = changes[QUALITY_STORAGE_KEY];
    if (areaName !== "local" || !change) return;
    qualityPreferenceChangedWhileLoading = true;
    preferredQuality = TwitchQuality.normalizeQuality(change.newValue);
    qualityPreferenceReady = true;
    qualityPreferenceRevision += 1;
    appliedQualityRoute = "";
    appliedQualityVideo = null;
    appliedQualityPreference = "";
    resetQualityRetry();
    checkPreferredQuality();
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
      stopPlayerPreferenceAutomation();
    }
  }

  chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
    if (message?.type === "get_managed_tab_identity") {
      sendResponse({ identity: readManagedTabIdentity() });
      return false;
    }
    if (
      message?.type === "set_managed_tab_identity"
      && typeof message.identity === "string"
    ) {
      try {
        window.sessionStorage.setItem(MANAGED_TAB_IDENTITY_KEY, message.identity);
      } catch {}
      sendResponse({ identity: readManagedTabIdentity() });
      return false;
    }
    if (
      message?.type === "set_player_audio_preference"
      && typeof message.unmuted === "boolean"
      && typeof message.login === "string"
    ) {
      setPlayerAudioPreference(message.unmuted, message.login);
      sendResponse({ unmuted: message.unmuted });
      return false;
    }
    if (message?.type === "start_monitoring" && !monitoring) {
      monitoring = true;
      report();
      if (monitoring) timer = window.setInterval(report, 2000);
    } else if (message?.type === "stop_monitoring" && monitoring) {
      stopMonitoring();
    }
  });
  const managedTabIdentity = readManagedTabIdentity();
  try {
    chrome.runtime.sendMessage({
      type: "managed_tab_identity_ready",
      identity: managedTabIdentity,
    }).catch(() => {});
  } catch {}
  chrome.storage.onChanged.addListener(qualityPreferenceChanged);
  playerPreferenceWatcher = window.setInterval(checkPlayerPreferences, QUALITY_CHECK_INTERVAL_MS);
  checkPlayerAudioPreference();
  void loadPreferredQuality();
})();