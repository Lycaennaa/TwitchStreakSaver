# Privacy

No analytics or telemetry integrations are included, and the app and extension do not send data to a developer-operated service. The app communicates with Twitch; browser-to-app messages use the local native-messaging and loopback bridge described below.

This guide describes the desktop app and browser extension as implemented in this repository. It does not describe data handling by Twitch, the browser, or the operating system after they receive data.

## Twitch account and network activity

- Twitch sign-in uses OAuth in a browser and a callback at `http://localhost:65432/redirect`, served by a loopback listener. The app requests the `user:read:follows` scope.
- The access token is stored through the operating system credential service: macOS Keychain or Windows Credential Manager. The app uses it to validate the Twitch session and make Helix requests; it is not sent to the browser extension.
- The browser extension passively observes `community-points-user-v1` notifications from a logged-in Twitch page’s `hermes.twitch.tv` WebSocket. It forwards normalized Points/Streak events with the subscription viewer ID; the app accepts events only when that ID matches its OAuth account. Events missed while no matching page source is connected cannot be reconstructed.
- The app’s configured Twitch endpoints are OAuth and Helix followed-channels and streams. The signed-in Twitch page handles its Hermes connection; the extension never parses, stores, or forwards the page’s authentication frame or token. The OAuth client ID in the app is public; it is not the user’s access token.

## Data stored on this device

- The desktop app persists preferences and its activity log through eframe storage. Preferences include followed channels, stream settings, browser launch choices, and configured browser profile paths.
- The activity log keeps up to 500 recent entries, including event type, time, streamer, and stream activity details.
- Browser-profile-local `storage.local` holds managed-tab and raid-redirect state, preferences, and up to 128 completed browser-command IDs. It is separate from app preferences and the activity log.
- The Twitch access token is stored separately in the operating system credential service, not in the app preferences or activity log.

## Browser integration

The extension's manifest grants access to Twitch player and channel pages. Its content scripts read player/chat state; a document-start main-world hook observes the Points subscription topic, its successful acknowledgment, and Points event notifications. It sends only source status and normalized event fields to the app over the local native-messaging bridge; it does not parse authentication frames or store/forward the page token. Chat text is inspected locally to identify messages from the connected viewer; the app bridge receives status and, when applicable, the matching author login—not the message text.

The extension connects to its browser-specific native-messaging host, which connects to the app's loopback TCP bridge at `127.0.0.1:43127`. The app also listens on `127.0.0.1:43128` for a separate registration/diagnostic endpoint; the extension does not connect directly to that port.

## Screenshot privacy

Screenshot privacy hides selected names, activity details, searches, and variable channel lists from the app's on-screen display. It does not delete or change saved preferences, activity history, browser state, or Twitch data.

Twitch, the browser, and the operating system handle their respective requests and stored data under their own policies.
