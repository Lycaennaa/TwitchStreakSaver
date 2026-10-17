# Twitch Streak Saver

Cross-platform Rust desktop app built with egui.

On startup, the app immediately begins connecting to Twitch instead of waiting for **Connect Twitch**.

The app signs in through Twitch’s browser OAuth flow, stores the token in the macOS Keychain or Windows Credential Manager, loads followed channels, and refreshes live status every minute. A read-only live PubSub listener detects `WATCH_STREAK`, points earned/spent, and bonus-claim availability; events missed while disconnected cannot be reconstructed reliably. Point balances show the latest received event, not a startup snapshot. Bonus claims remain manual. Selected live Streak channels take priority. If no `WATCH_STREAK` arrives, each turn yields after 16 eligible minutes of playing video, ready chat UI, and an available app slot. While Points is scheduled, timed-out Streak channels are deprioritized until a Points turn completes or 16 elapsed minutes pass. Points turns use the same eligibility conditions and advance after 16 eligible minutes. Point, Streak, and automatic stream-switch activity appears in an in-app log with local time, streamer, and event details. Separate player tabs are muted by default. Channel selections and settings remain local. OAuth uses Streamlink Twitch GUI’s public Client ID and fixed localhost callback.

The followed-channel list is sorted alphabetically by display name, ignoring letter case.

The activity log can be filtered by event type and searched with case-insensitive fuzzy matching across event text, streamer names, and timestamps.

Each button selects its option for every followed channel, regardless of the current search. Once all channels have an option selected, the button changes to **Unselect all** and clears that selection.

## Chromium browser extension setup

1. For an x86-64 GNU Windows bundle, run `just package-windows-x86_64` and extract the resulting ZIP while keeping `twitch-streak-saver.exe`, `twitch_streak_saver_host.exe`, and `chrome-extension/` together. The ZIP is written to `dist/twitch-streak-saver-windows-x86_64-gnu.zip`. For other builds, use `distill --cwd . cargo build --bins`; place `chrome-extension/` beside the desktop executable on Windows/portable installs or in `Contents/Resources/` for a macOS app. Keep the native-host sidecar beside the executable (`Contents/MacOS/` in a macOS app bundle).
2. Start the desktop app and click **Set up browser extension**. Choose a detected Chrome, Brave, Edge, or Helium browser from the list; the app opens its extensions page and reveals the shipped folder.
3. In the selected browser, enable Developer mode, choose **Load unpacked**, and select the revealed folder. If upgrading from an earlier build, remove the old unpacked extension first; native-host pairing is automatic.
4. Click the extension toolbar icon: its popup wakes the service worker, pairs directly through Chrome native messaging, and updates through the app handshake. The extension no longer depends on a separate localhost HTTP registration request. Use **Wake / reconnect** to restart a pending native-host connection. Chrome can suspend idle Manifest V3 workers; an open native-messaging port keeps it alive, while the 30-second alarm retries after disconnects or stalled handshakes. The desktop app's **Check app bridge** button checks/restarts only the app side. The app reports connected only after the native-messaging handshake succeeds. Then use **Open selected channels** to load the selected streams.

**Preferred stream quality** is chosen in the Rust desktop app under **Stream quality**. It defaults to 160p to save bandwidth and applies automatically on Twitch player pages. Choose 160p, 360p, 480p, 720p, 1080p, Source, or Twitch Auto. If a selected rendition is unavailable, the extension chooses the lowest available video quality. The app saves this preference and sends it to the extension when the browser bridge connects; changes apply to open Twitch pages. Refresh already-open Twitch pages after upgrading to extension version 0.3.15.

5. With Twitch and the browser bridge connected, selected live Streak channels open first, followed by eligible Points channels. Each channel opens as one combined Twitch page in its own window; the tab is selected without focusing the window, and the extension asks the browser not to automatically discard it. **Restore and focus browser when opening** restores and focuses each stream window and waits for the page to load; enabling it may interrupt other work. Background tabs can still be frozen or throttled, and playback/autoplay remain browser-controlled. Manually opened tabs occupy capacity and remain under user control. The app schedules up to two streams and reserves capacity for already-open external and app-managed tabs; it leaves existing tabs open. **Start**/**Stop All** control scheduling; **Auto-start on app launch** is enabled by default. Twitch and browser connections are still required.

**Close app-opened stream tabs on Twitch raid redirects** is off by default. When enabled, the extension closes a managed player tab whose URL changes to include the query parameter `referrer=raid`; external tabs are unaffected. The channel stays suppressed from automatic reopening until Twitch confirms it is offline. At browser startup, a managed tab already on a raid URL without a verifiable saved identity (including tabs created before version 0.3.13 or tabs where session storage was blocked) stays open and its channel is held from reopening until the route changes, avoiding false closure of a reused tab ID.

On Windows, **Launch at Windows sign-in** adds or removes this app's per-user startup entry.

The extension inventories external Twitch channel/player tabs, excluding app-managed tabs and chat-only tabs. Live followed channels are mapped by ID; unknown Twitch channel tabs reserve capacity conservatively. VOD and non-channel pages are ignored. Player status and combined-page chat UI readiness are reported; chat UI readiness does not prove a live Twitch chat connection. External tabs count toward capacity but are not rotated by the app. The extension reports app-managed player channels separately, so tabs restored after reconnect also reserve capacity; unknown managed channels reserve capacity conservatively. Reload extension version 0.3.15 after updating. Older versions cannot report restored app-managed streams, so the app pauses stream opening until the extension is reloaded. Versions through 0.3.1 stored managed-tab ownership in session storage, which the browser clears on extension upgrade; those tabs stay open rather than risk closing tabs the app did not create.

Build incrementally with:

```sh
distill --cwd . cargo build
```

Run Rust tests with:

```sh
buildproj cargo test
```

## macOS app bundle

On Apple Silicon, run `just package-macos` to build `dist/macos/Twitch Streak Saver.app` and install/overwrite `/Applications/Twitch Streak Saver.app`. It stages the replacement before removing the existing app. The bundle includes the Chromium extension in `Contents/Resources/chrome-extension` and the native-host sidecar in `Contents/MacOS`. In Chromium, use Developer mode and **Load unpacked** on the extension folder. The bundle is unsigned; direct distribution requires Developer ID signing and notarization.