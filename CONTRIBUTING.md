# Contributing

## Development setup

Install a Rust toolchain and [`just`](https://just.systems/). Node.js is needed for the browser-extension checks. See [Development](DEVELOPMENT.md) for the project commands and [Architecture](ARCHITECTURE.md) for the main component boundaries.

## Before opening a pull request

- Run `just fmt` to format Rust code.
- Run `just check` to run formatting, build, Rust tests, and browser-extension checks. The recipe includes formatting and may update Rust files.
- Keep changes to the shared Rust/browser message protocol synchronized across both implementations and their tests.
- Use synthetic account, channel, and browser data in tests. Do not commit OAuth tokens, real chat content, or private browser-profile paths.
- In the pull-request description, summarize the change, list the checks you ran, and disclose whether AI assistance was used.
