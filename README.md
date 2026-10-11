# Twitch Streak Saver

Cross-platform Rust desktop app built with egui.

The app signs in through Twitch’s browser OAuth flow, stores the token in the macOS Keychain or Windows Credential Manager, loads followed channels, and refreshes live status every minute. A read-only live PubSub listener detects `WATCH_STREAK`, points earned/spent, and bonus-claim availability; events missed while disconnected cannot be reconstructed reliably. Point balances show the latest received event, not a startup snapshot. Bonus claims remain manual. Selected live Streak channels take priority over automatically rotating selected live Points channels. Points turns advance after 16 eligible minutes of playing video, ready chat UI, and an available app slot, then show a dismissible completion alert. Separate player tabs are muted by default. Channel selections and settings remain local. OAuth uses Streamlink Twitch GUI’s public Client ID and fixed localhost callback.

## Chrome / Brave extension setup

1. Build both executables with `distill --cwd . cargo build --bins`.
2. In Chrome (`chrome://extensions`) or Brave (`brave://extensions`), enable Developer mode, choose **Load unpacked**, and select `chrome-extension/`.
3. Copy the extension ID. In the app’s browser integration panel, paste it and choose **Install host**; this registers the native host for Chrome and Brave.
4. Reload the extension and wait for the app to show **Browser connected**.
5. With Twitch and the browser bridge connected, selected live Streak channels open first, followed by eligible Points channels. The **Twitch tab opening** selector controls manual and automatic tabs: **Player + chat tabs** opens separate muted player and chat tabs; **Regular Twitch tab** opens a combined channel page. **Restore and focus browser when opening** restores a minimized window, focuses stream tabs, and waits for each page to load; enabling it may interrupt other work. Manually opened tabs occupy capacity and remain under user control. **Start**/**Stop All** control scheduling; **Auto-start on app launch** is enabled by default. Twitch and browser connections are still required.

The extension inventories external Twitch channel/player tabs and excludes app-managed tabs and chat-only tabs. Live followed channels are mapped by ID; unknown Twitch channel tabs reserve capacity conservatively. VOD and non-channel pages are ignored. Player status and popout chat UI readiness are reported; chat UI readiness does not prove a live Twitch chat connection. External tabs count toward capacity but are not rotated by the app. Reload extension version 0.2.0 after updating. Browser commands use a loopback bridge on `127.0.0.1:43127`. Waking ensures Twitch pages finish loading but cannot override browser autoplay, background-throttling, or playback policies.

Build incrementally with:

```sh
distill --cwd . cargo build
```

Run Rust tests with:

```sh
buildproj cargo test
```