# Development

## Build and test

Use the project recipes to build the app and run checks:

```sh
just build
just test
just check
```

`just check` aggregates formatting, build, Rust tests, extension tests, and extension-package validation.
`just package-extensions` builds the Chromium and Firefox extension packages in `dist/`. The Firefox package is unsigned and can be loaded temporarily in Firefox for local testing.

These recipes use [Distill](https://github.com/lycaennaa/distill) when installed and Cargo otherwise; Distill is optional.

To invoke Cargo directly:

```sh
cargo build --bins
cargo test
```

## Releases

Run the **Release** workflow from the default branch and enter the Cargo version without the leading `v`. It requires a successful CI run on the current default-branch commit, validates the Cargo and macOS bundle versions, creates the matching `v<version>` tag, and publishes the release automatically. Releases contain an ad-hoc-signed macOS app ZIP, a Windows x86-64 MSVC app ZIP, and separate Chromium and Firefox extension ZIPs. Public repository releases receive build-provenance attestations for all four assets. The macOS app is not Developer ID-signed or notarized.
Published releases are immutable on this repository. The workflow uploads and verifies all assets on a draft before publishing. If a published release is incomplete, choose a new version; the same tag cannot be reused.

