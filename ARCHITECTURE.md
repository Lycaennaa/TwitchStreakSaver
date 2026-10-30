# Architecture

## Main components

- `src/main.rs` starts the eframe desktop application and coordinates its UI, Twitch state, queue, and browser bridge. UI and platform integrations are split into modules under `src/main/`.
- The library modules exported by `src/lib.rs` contain shared models, queue scheduling, Twitch event parsing, browser protocol types, and the browser bridge.
- `src/twitch.rs` and `src/twitch/` implement Twitch OAuth, Helix requests, connection retries, and the PubSub listener.
- `src/scheduler.rs` plans Streak and Points turns. It receives channel state and turn eligibility from the app and applies the two-stream limit.
- `src/bin/twitch_streak_saver_host.rs` is the browser native-messaging host. It forwards messages between the browser and the app's loopback TCP bridge.
- `chrome-extension/` contains the Manifest V3 service worker, popup, tab-management and quality helpers, and content scripts for Twitch pages.

## Runtime flow

1. The app obtains a Twitch access token through the browser OAuth flow, then refreshes followed channels and live status from Helix while a separate PubSub connection receives live events.
2. The app combines channel state, selections, and player/chat readiness with the scheduler to choose the next managed stream turn.
3. Browser commands and status messages use the shared Rust protocol types. The app bridge listens on loopback; the native host connects that bridge to Chrome native messaging.
4. The extension service worker manages Twitch tabs and returns tab/player/chat state to the app. Content scripts read the state needed from Twitch pages.
5. The app persists preferences and the activity log in eframe storage. The extension persists its browser-specific state in `chrome.storage.local`.

See [How it works](HOW_IT_WORKS.md) for scheduling and recovery behavior, and [Privacy](PRIVACY.md) for the data handled by each boundary.
