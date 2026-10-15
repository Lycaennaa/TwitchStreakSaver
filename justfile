worktree_root := justfile_directory()
worktree_build_dir := worktree_root + "/.build"

default:
    just --list

build:
    distill --cwd . cargo build --bins

package-windows-x86_64:
    @CARGO_TARGET_DIR="{{worktree_build_dir}}" RUSTFLAGS="-C target-feature=+crt-static -C link-arg=-static" distill --cwd "{{worktree_root}}" cargo build --release --target x86_64-pc-windows-gnu --bins
    @rm -rf "{{worktree_root}}/dist/twitch-streak-saver-windows-x86_64-gnu"
    @mkdir -p "{{worktree_root}}/dist/twitch-streak-saver-windows-x86_64-gnu/chrome-extension"
    @cp "{{worktree_build_dir}}/x86_64-pc-windows-gnu/release/twitch-streak-saver.exe" "{{worktree_build_dir}}/x86_64-pc-windows-gnu/release/twitch_streak_saver_host.exe" "{{worktree_root}}/dist/twitch-streak-saver-windows-x86_64-gnu/"
    @cp "{{worktree_root}}/chrome-extension/manifest.json" "{{worktree_root}}/chrome-extension/background.js" "{{worktree_root}}/chrome-extension/content.js" "{{worktree_root}}/chrome-extension/popup.html" "{{worktree_root}}/chrome-extension/popup.js" "{{worktree_root}}/chrome-extension/twitch_tabs.js" "{{worktree_root}}/chrome-extension/trusted-extension-id" "{{worktree_root}}/dist/twitch-streak-saver-windows-x86_64-gnu/chrome-extension/"
    @cd "{{worktree_root}}/dist" && zip -r -FS twitch-streak-saver-windows-x86_64-gnu.zip twitch-streak-saver-windows-x86_64-gnu

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