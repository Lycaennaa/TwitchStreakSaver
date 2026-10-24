worktree_root := justfile_directory()
worktree_build_dir := worktree_root + "/.build"
cargo_command := if `command -v distill >/dev/null 2>&1 && echo yes` == "yes" { "distill --cwd \"" + worktree_root + "\" cargo" } else { "cargo" }

default:
    just --list

build:
    {{cargo_command}} build --bins

package-windows-x86_64:
    @CARGO_TARGET_DIR="{{worktree_build_dir}}" RUSTFLAGS="-C target-feature=+crt-static -C link-arg=-static" {{cargo_command}} build --release --target x86_64-pc-windows-gnu --bins
    @rm -rf "{{worktree_root}}/dist/twitch-streak-saver-windows-x86_64-gnu"
    @mkdir -p "{{worktree_root}}/dist/twitch-streak-saver-windows-x86_64-gnu/chrome-extension"
    @cp "{{worktree_build_dir}}/x86_64-pc-windows-gnu/release/twitch-streak-saver.exe" "{{worktree_build_dir}}/x86_64-pc-windows-gnu/release/twitch_streak_saver_host.exe" "{{worktree_root}}/dist/twitch-streak-saver-windows-x86_64-gnu/"
    @cp "{{worktree_root}}/chrome-extension/manifest.json" "{{worktree_root}}/chrome-extension/background.js" "{{worktree_root}}/chrome-extension/content.js" "{{worktree_root}}/chrome-extension/managed_tabs.js" "{{worktree_root}}/chrome-extension/quality.js" "{{worktree_root}}/chrome-extension/popup.html" "{{worktree_root}}/chrome-extension/popup.js" "{{worktree_root}}/chrome-extension/twitch_tabs.js" "{{worktree_root}}/chrome-extension/trusted-extension-id" "{{worktree_root}}/dist/twitch-streak-saver-windows-x86_64-gnu/chrome-extension/"
    @cd "{{worktree_root}}/dist" && zip -r -FS twitch-streak-saver-windows-x86_64-gnu.zip twitch-streak-saver-windows-x86_64-gnu

run: build
     cargo run --bin twitch-streak-saver

fmt:
    {{cargo_command}} fmt

test:
    {{cargo_command}} test

extension-check:
    taskpolicy -b node --check chrome-extension/background.js
    taskpolicy -b node --check chrome-extension/content.js
    taskpolicy -b node --check chrome-extension/quality.js
    taskpolicy -b node --check chrome-extension/popup.js
    taskpolicy -b node --check chrome-extension/twitch_tabs.js
    taskpolicy -b node --test chrome-extension/content.test.js chrome-extension/quality.test.js chrome-extension/twitch_tabs.test.js
    taskpolicy -b node --check chrome-extension/background.test.js
    taskpolicy -b node --test chrome-extension/background.test.js
    python3 -m json.tool chrome-extension/manifest.json > /dev/null

check: fmt build test extension-check
    @true

package-macos:
    @CARGO_TARGET_DIR="{{worktree_build_dir}}" {{cargo_command}} build --release --target aarch64-apple-darwin --bins
    @rm -rf "{{worktree_root}}/dist/macos/Twitch Streak Saver.app"
    @mkdir -p "{{worktree_root}}/dist/macos/Twitch Streak Saver.app/Contents/MacOS" "{{worktree_root}}/dist/macos/Twitch Streak Saver.app/Contents/Resources/chrome-extension"
    @cp "{{worktree_build_dir}}/aarch64-apple-darwin/release/twitch-streak-saver" "{{worktree_root}}/dist/macos/Twitch Streak Saver.app/Contents/MacOS/twitch-streak-saver"
    @cp "{{worktree_build_dir}}/aarch64-apple-darwin/release/twitch_streak_saver_host" "{{worktree_root}}/dist/macos/Twitch Streak Saver.app/Contents/MacOS/twitch_streak_saver_host"
    @cp "{{worktree_root}}/chrome-extension/manifest.json" "{{worktree_root}}/chrome-extension/background.js" "{{worktree_root}}/chrome-extension/content.js" "{{worktree_root}}/chrome-extension/managed_tabs.js" "{{worktree_root}}/chrome-extension/quality.js" "{{worktree_root}}/chrome-extension/popup.html" "{{worktree_root}}/chrome-extension/popup.js" "{{worktree_root}}/chrome-extension/twitch_tabs.js" "{{worktree_root}}/chrome-extension/trusted-extension-id" "{{worktree_root}}/dist/macos/Twitch Streak Saver.app/Contents/Resources/chrome-extension/"
    @cp "{{worktree_root}}/packaging/macos/Info.plist" "{{worktree_root}}/dist/macos/Twitch Streak Saver.app/Contents/Info.plist"
    @plutil -lint "{{worktree_root}}/dist/macos/Twitch Streak Saver.app/Contents/Info.plist"
    @rm -rf "/Applications/.Twitch Streak Saver.app.new"
    @ditto "{{worktree_root}}/dist/macos/Twitch Streak Saver.app" "/Applications/.Twitch Streak Saver.app.new"
    @plutil -lint "/Applications/.Twitch Streak Saver.app.new/Contents/Info.plist"
    @rm -rf "/Applications/Twitch Streak Saver.app"
    @mv "/Applications/.Twitch Streak Saver.app.new" "/Applications/Twitch Streak Saver.app"