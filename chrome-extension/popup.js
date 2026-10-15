const status = document.querySelector("#status");
const wakeButton = document.querySelector("#wake");
const STATUS_POLL_INTERVAL_MS = 500;
const STATUS_POLL_TIMEOUT_MS = 10000;

let statusPollTimer = null;
let wakeGeneration = 0;

function showStatus(result) {
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
    statusPollTimer = setTimeout(
      () => void pollStatus(generation, deadline),
      STATUS_POLL_INTERVAL_MS,
    );
  } catch (error) {
    status.textContent = `Could not read connection status: ${error.message}`;
  }
}

async function wakeServiceWorker(forceReconnect = false) {
  if (statusPollTimer !== null) {
    clearTimeout(statusPollTimer);
    statusPollTimer = null;
  }
  const generation = ++wakeGeneration;
  const deadline = Date.now() + STATUS_POLL_TIMEOUT_MS;
  wakeButton.disabled = true;
  status.textContent = forceReconnect
    ? "Restarting the pending native-host connection…"
    : "Waking the extension worker and retrying native messaging…";
  try {
    const result = await chrome.runtime.sendMessage({
      type: "wake_native_host",
      forceReconnect,
    });
    if (generation !== wakeGeneration) return;
    showStatus(result);
    if (!result?.bridgeReady) {
      statusPollTimer = setTimeout(
        () => void pollStatus(generation, deadline),
        STATUS_POLL_INTERVAL_MS,
      );
    }
  } catch (error) {
    status.textContent = `Could not wake the extension worker: ${error.message}`;
  } finally {
    if (generation === wakeGeneration) wakeButton.disabled = false;
  }
}

wakeButton.addEventListener("click", () => void wakeServiceWorker(true));
void wakeServiceWorker();
