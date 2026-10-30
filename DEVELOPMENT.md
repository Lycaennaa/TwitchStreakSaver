# Development

## Build and test

Use the project recipes to build the app and run Rust tests:

```sh
just build
just test
```

These recipes use [Distill](https://github.com/lycaennaa/distill) when installed and Cargo otherwise; Distill is optional.

To invoke Cargo directly:

```sh
cargo build --bins
cargo test
```
