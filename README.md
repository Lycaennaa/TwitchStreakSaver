# Twitch Streak Saver

Cross-platform Rust desktop app built with egui.

The app signs in through Twitch’s browser OAuth flow, stores the token in the macOS Keychain or Windows Credential Manager, loads followed channels, and refreshes live status every minute. A read-only live PubSub listener detects `WATCH_STREAK`, points earned/spent, and bonus-claim availability; events missed while disconnected cannot be reconstructed reliably. Point balances show the latest received event, not a startup snapshot. Bonus claims remain manual. Selected live Streak channels take priority over automatically rotating selected live Points channels. Points turns advance after 16 eligible minutes of playing video, ready chat UI, and an available app slot. Point and Streak activity appears in an in-app log with local time, streamer, and event details. Separate player tabs are muted by default. Channel selections and settings remain local. OAuth uses Streamlink Twitch GUI’s public Client ID and fixed localhost callback.

## Chrome / Brave extension setup

1. Build both executables with `distill --cwd . cargo build --bins`.
2. In Chrome (`chrome://extensions`) or Brave (`brave://extensions`), enable Developer mode, choose **Load unpacked**, and select `chrome-extension/`.
3. Copy the extension ID. In the app’s browser integration panel, paste it and choose **Install host**; this registers the native host for Chrome and Brave.
4. Reload the extension and wait for the app to show **Browser connected**.
5. With Twitch and the browser bridge connected, selected live Streak channels open first, followed by eligible Points channels. Each channel opens as one combined Twitch page in its own window; the tab is selected without focusing the window, and the extension asks the browser not to automatically discard it. **Restore and focus browser when opening** restores and focuses each stream window and waits for the page to load; enabling it may interrupt other work. Background tabs can still be frozen or throttled, and playback/autoplay remain browser-controlled. Manually opened tabs occupy capacity and remain under user control. **Start**/**Stop All** control scheduling; **Auto-start on app launch** is enabled by default. Twitch and browser connections are still required.

The extension inventories external Twitch channel/player tabs and excludes app-managed tabs and chat-only tabs. Live followed channels are mapped by ID; unknown Twitch channel tabs reserve capacity conservatively. VOD and non-channel pages are ignored. Player status and combined-page chat UI readiness are reported; chat UI readiness does not prove a live Twitch chat connection. External tabs count toward capacity but are not rotated by the app. Reload extension version 0.3.3 after updating. Versions through 0.3.1 stored managed-tab ownership in session storage, which the browser clears on extension reload/update. Use **Stop All** before updating if possible; if already reloaded, manually close old app-opened Twitch tabs once. Version 0.3.3 persists ownership and reloads managed pages when the extension version changes.

Build incrementally with:

```sh
distill --cwd . cargo build
```

Run Rust tests with:

```sh
buildproj cargo test
```