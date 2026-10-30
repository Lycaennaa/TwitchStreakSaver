# Privacy

No analytics or telemetry integrations are included, and the app and extension do not send data to a developer-operated service. The app communicates with Twitch; browser-to-app messages use the local native-messaging and loopback bridge described below.

This guide describes the desktop app and browser extension as implemented in this repository. It does not describe data handling by Twitch, the browser, or the operating system after they receive data.

## Twitch account and network activity

- Twitch sign-in uses OAuth in a browser and a callback at `http://localhost:65432/redirect`, served by a loopback listener. The app requests the `user:read:follows` scope.
- The access token is stored through the operating system credential service: macOS Keychain or Windows Credential Manager. The app uses it to validate the Twitch session, make Helix requests, and authenticate the PubSub listener.
- The app requests followed channels and live stream status from Twitch Helix, and listens for live Watch Streak and Points events over Twitch PubSub. The app does not reconstruct PubSub events missed while disconnected.
- The configured Twitch endpoints are OAuth, Helix followed-channels and streams, and PubSub. The OAuth client ID in the app is public; it is not the user's access token.

## Data stored on this device

- The desktop app persists preferences and its activity log through eframe storage. Preferences include followed channels, stream settings, browser launch choices, and configured browser profile paths.
- The activity log keeps up to 500 recent entries, including event type, time, streamer, and stream activity details.
- The browser extension uses `chrome.storage.local` for managed-tab state and settings, including stream quality, raid-redirect behavior, and adoption preferences. It also stores up to 128 completed recoverable browser-command IDs to suppress duplicates after a service-worker restart.
- The Twitch access token is stored separately in the operating system credential service, not in the app preferences or activity log.

## Browser integration

The extension's manifest grants access to Twitch player and channel pages. Its content script reads player and chat state from those pages. Chat text is inspected locally to identify messages from the connected viewer; the app bridge receives status and, when applicable, the matching author login—not the message text.

The extension connects to the native host through Chrome native messaging; the host connects to the app's loopback TCP bridge at `127.0.0.1:43127`. The app also listens on `127.0.0.1:43128` for its separate registration/diagnostic endpoint; the extension pairs over native messaging and does not connect directly to that port.

## Screenshot privacy

Screenshot privacy hides selected names, activity details, searches, and variable channel lists from the app's on-screen display. It does not delete or change saved preferences, activity history, browser state, or Twitch data.

Twitch, the browser, and the operating system handle their respective requests and stored data under their own policies.
