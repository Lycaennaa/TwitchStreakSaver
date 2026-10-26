const status = document.querySelector("#status");
const managedStreams = document.querySelector("#managed-streams");
const actionStatus = document.querySelector("#action-status");
const pauseButton = document.querySelector("#pause");
const openSelectedButton = document.querySelector("#open-selected");
const wakeButton = document.querySelector("#wake");
const STATUS_POLL_INTERVAL_MS = 500;
const STATUS_POLL_TIMEOUT_MS = 10000;
let statusPollTimer = null;
let wakeGeneration = 0;

function showManagedStreams(streams) {
  if (!Array.isArray(streams)) return;
  const logins = Array.from(new Set(
    streams.filter((login) => typeof login === "string" && login.trim().length > 0),
  ));
  managedStreams.textContent = logins.length === 0
    ? "No managed streams are open."
    : `${logins.length}: ${logins.join(", ")}`;
}

function showStatus(result) {
  showManagedStreams(result?.managedStreams);
  if (result?.bridgeReady) {
    status.textContent = "Connected to Twitch Streak Saver.";
  } else if (result?.error) {
    status.textContent = `Pairing failed: ${result.error}. Automatic retries continue.`;
  } else if (result?.nativeConnected) {
    status.textContent = "Native host started; waiting for the app handshake.";
  } else {
    status.textContent = "App is not reachable. Leave it open; the extension will retry automatically.";
  }
}
async function refreshStatus() {
  try {
    showStatus(await chrome.runtime.sendMessage({ type: "native_host_status" }));
  } catch (error) {
    status.textContent = `Could not refresh native-messaging status: ${error.message}`;
  }
}


async function pollStatus(generation, deadline) {
  if (generation !== wakeGeneration) return;
  try {
    const result = await chrome.runtime.sendMessage({ type: "native_host_status" });
    if (generation !== wakeGeneration) return;
    showStatus(result);
    if (result?.bridgeReady) return;
    if (Date.now() >= deadline) {
      status.textContent = "Still waiting for the native-messaging handshake. Use Wake / reconnect to retry.";
      return;
    }
  } catch (error) {
    if (generation !== wakeGeneration) return;
    status.textContent = `Could not read native-messaging status: ${error.message}`;
    if (Date.now() >= deadline) return;
  }
  statusPollTimer = setTimeout(
    () => void pollStatus(generation, deadline),
    STATUS_POLL_INTERVAL_MS,
  );
}

async function wakeServiceWorker(forceReconnect = false) {
  if (statusPollTimer !== null) {
    clearTimeout(statusPollTimer);
    statusPollTimer = null;
  }
  const generation = ++wakeGeneration;
  const deadline = Date.now() + STATUS_POLL_TIMEOUT_MS;
  wakeButton.disabled = true;
  if (forceReconnect) status.textContent = "Restarting the pending native-host connection…";
  try {
    const result = await chrome.runtime.sendMessage({
      type: "wake_native_host",
      forceReconnect,
    });
    if (generation !== wakeGeneration) return;
    showStatus(result);
    if (!result?.bridgeReady) void pollStatus(generation, deadline);
  } catch (error) {
    if (generation === wakeGeneration) {
      status.textContent = `Could not wake the native host: ${error.message}`;
    }
  } finally {
    if (generation === wakeGeneration) wakeButton.disabled = false;
  }
}

async function requestAction(action) {
  pauseButton.disabled = true;
  openSelectedButton.disabled = true;
  actionStatus.textContent = "Waiting for the app…";
  try {
    const result = await chrome.runtime.sendMessage({ type: "popup_action", action });
    actionStatus.textContent = result?.success === true
      ? result.message
      : `Could not apply action: ${result?.message || "The app did not confirm it."}`;
  } catch (error) {
    actionStatus.textContent = `Could not apply action: ${error.message}`;
  } finally {
    await refreshStatus();
    pauseButton.disabled = false;
    openSelectedButton.disabled = false;
  }
}

chrome.runtime.onMessage.addListener((message) => {
  if (message?.type === "popup_status_changed") void refreshStatus();
});

pauseButton.addEventListener("click", () => void requestAction("pause_automation"));
openSelectedButton.addEventListener("click", () => void requestAction("open_selected"));
wakeButton.addEventListener("click", () => void wakeServiceWorker(true));
void wakeServiceWorker();
