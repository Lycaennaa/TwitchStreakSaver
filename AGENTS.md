# Agent guidance

- Use [Development](DEVELOPMENT.md) for build and test commands and [Contributing](CONTRIBUTING.md) for pull-request expectations; do not duplicate those procedures here.
- Read [Architecture](ARCHITECTURE.md) and [How it works](HOW_IT_WORKS.md) before changing the scheduler or Rust/browser bridge.
- Treat `src/browser_protocol.rs`, the native host, and the extension message handlers as one shared contract. Update both sides and their tests together.
- Keep Twitch access tokens out of logs, fixtures, and browser-extension storage. Use synthetic account and channel data in tests.
