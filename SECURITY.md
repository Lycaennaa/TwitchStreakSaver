# Security

## Safe use

- Install the desktop app and browser extension from a trusted build of this project, and use the extension folder packaged with that build. The native host and extension are designed to work together.
- The extension has `nativeMessaging`, `storage`, and `tabs` permissions and runs on Twitch channel and player pages. Load it only in browser profiles you trust.
- The app stores the Twitch access token in the operating system credential service. Do not share credential-store exports, diagnostic output containing account details, or browser profile paths.
- Screenshot privacy only hides selected information from the app's display; it does not erase saved data or conceal information shown by the browser.
- Keep the app, browser, and extension updated together. After replacing the app, follow [Setup](SETUP.md) to reload the extension if needed.
