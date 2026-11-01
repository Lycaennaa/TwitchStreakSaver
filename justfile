worktree_root := justfile_directory()
worktree_build_dir := worktree_root + "/.build"
cargo_command := if os() == "windows" { "cargo" } else if `command -v distill >/dev/null 2>&1 && echo yes` == "yes" { "distill --cwd \"" + worktree_root + "\" cargo" } else { "cargo" }

default:
    just --list

build:
    {{cargo_command}} build --bins

package-chrome-extension:
    @python3 "{{worktree_root}}/scripts/package_chrome_extension.py"

package-windows-x86_64:
    @{{if os() == "windows" { "py -3" } else { "python3" }}} "{{worktree_root}}/scripts/package_windows.py"


run: build
     cargo run --bin twitch-streak-saver

fmt:
    {{cargo_command}} fmt

fmt-check:
    {{cargo_command}} fmt --all -- --check
test:
    {{cargo_command}} test

extension-check:
    taskpolicy -b node --check chrome-extension/background.js
    taskpolicy -b node --check chrome-extension/content.js
    taskpolicy -b node --check chrome-extension/managed_tabs.js
    taskpolicy -b node --check chrome-extension/quality.js
    taskpolicy -b node --check chrome-extension/popup.js
    taskpolicy -b node --check chrome-extension/twitch_tabs.js
    taskpolicy -b node --test chrome-extension/content.test.js chrome-extension/quality.test.js chrome-extension/twitch_tabs.test.js
    taskpolicy -b node --check chrome-extension/background.test.js
    taskpolicy -b node --test chrome-extension/background.test.js
    python3 -m json.tool chrome-extension/manifest.json > /dev/null

check: fmt-check build test extension-check package-chrome-extension


package-macos: package-chrome-extension
    @CARGO_TARGET_DIR="{{worktree_build_dir}}" {{cargo_command}} build --release --target aarch64-apple-darwin --bins
    @rm -rf "{{worktree_root}}/dist/macos/Twitch Streak Saver.app"
    @mkdir -p "{{worktree_root}}/dist/macos/Twitch Streak Saver.app/Contents/MacOS" "{{worktree_root}}/dist/macos/Twitch Streak Saver.app/Contents/Resources/chrome-extension"
    @cp "{{worktree_build_dir}}/aarch64-apple-darwin/release/twitch-streak-saver" "{{worktree_root}}/dist/macos/Twitch Streak Saver.app/Contents/MacOS/twitch-streak-saver"
    @cp "{{worktree_build_dir}}/aarch64-apple-darwin/release/twitch_streak_saver_host" "{{worktree_root}}/dist/macos/Twitch Streak Saver.app/Contents/MacOS/twitch_streak_saver_host"
    @cp -R "{{worktree_root}}/dist/chrome-extension/." "{{worktree_root}}/dist/macos/Twitch Streak Saver.app/Contents/Resources/chrome-extension/"
    @cp "{{worktree_root}}/packaging/macos/Info.plist" "{{worktree_root}}/dist/macos/Twitch Streak Saver.app/Contents/Info.plist"
    @plutil -lint "{{worktree_root}}/dist/macos/Twitch Streak Saver.app/Contents/Info.plist"

install-macos: package-macos
    @rm -rf "/Applications/.Twitch Streak Saver.app.new"
    @ditto "{{worktree_root}}/dist/macos/Twitch Streak Saver.app" "/Applications/.Twitch Streak Saver.app.new"
    @plutil -lint "/Applications/.Twitch Streak Saver.app.new/Contents/Info.plist"
    @rm -rf "/Applications/Twitch Streak Saver.app"
    @mv "/Applications/.Twitch Streak Saver.app.new" "/Applications/Twitch Streak Saver.app"