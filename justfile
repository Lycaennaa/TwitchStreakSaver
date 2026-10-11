default:
    just --list

build:
    distill --cwd . cargo build --bins

run: build
     cargo run --bin twitch-streak-saver

fmt:
    buildproj cargo fmt

test:
    buildproj cargo test

extension-check:
    taskpolicy -b node --check chrome-extension/background.js
    taskpolicy -b node --check chrome-extension/content.js
    taskpolicy -b node --check chrome-extension/twitch_tabs.js
    taskpolicy -b node --test chrome-extension/twitch_tabs.test.js
    taskpolicy -b node --check chrome-extension/background.test.js
    taskpolicy -b node --test chrome-extension/background.test.js
    python3 -m json.tool chrome-extension/manifest.json > /dev/null

check: fmt build test extension-check
    @true