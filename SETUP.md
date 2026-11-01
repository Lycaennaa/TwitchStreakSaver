# Setup

## Browser extension setup

1. For a Windows bundle, run `just package-windows-x86_64`. On macOS it cross-builds the x86-64 GNU target and writes `dist/twitch-streak-saver-windows-x86_64-gnu.zip`; on Windows it builds native x86-64 MSVC and writes `dist/twitch-streak-saver-windows-x86_64-msvc.zip`. Both ZIPs contain `twitch-streak-saver.exe`, `twitch_streak_saver_host.exe`, `chrome-extension/`, and `firefox-extension/`. The MSVC build statically links the CRT, so users should not need a separate VC runtime download. For other builds, run `cargo build --bins`; place both extension folders beside the desktop executable on Windows/portable installs or in `Contents/Resources/` for a macOS app. Keep the native-host sidecar beside the executable (`Contents/MacOS/` in a macOS app bundle).
2. Start the desktop app and click **Set up browser extension**. Choose a detected Chrome, Brave, Edge, Helium, or Firefox browser from the list; the app opens its extension page and reveals the corresponding shipped folder.
3. In the selected browser, enable Developer mode, choose **Load unpacked**, and select the revealed folder. If upgrading from an earlier build, remove the old unpacked extension first; native-host pairing is automatic.
4. For Chromium browsers, click the extension toolbar icon: its popup wakes the service worker, pairs through Chrome native messaging, and updates through the app handshake. Use **Wake / reconnect** to restart a pending native-host connection. Chrome can suspend idle Manifest V3 workers; an open native-messaging port keeps the worker alive, while the 30-second alarm retries after disconnects or stalled handshakes. The desktop app's **Check app bridge** button checks/restarts only the app side. The app reports connected only after the native-messaging handshake succeeds.
5. With Twitch and the browser bridge connected, selected live Streak channels open first, followed by eligible Points channels. Each channel opens as one combined Twitch page in its own window; the tab is selected without focusing the window, and the extension asks the browser not to automatically discard it. **Restore and focus browser when opening** restores and focuses each stream window and waits for the page to load; enabling it may interrupt other work. Background tabs can still be frozen or throttled, and playback/autoplay remain browser-controlled. Manually opened tabs occupy capacity and remain under user control. The app schedules up to two streams and reserves capacity for already-open external and app-managed tabs; it leaves existing tabs open. **Start**/**Stop All** control scheduling; **Auto-start on app launch** is enabled by default. Twitch and browser connections are still required.

## Firefox

On Apple Silicon, run `just package-macos` to build `dist/macos/Twitch Streak Saver.app`; this only creates the package. Run `just install-macos` to install or overwrite `/Applications/Twitch Streak Saver.app`; it stages the replacement before removing the existing app. The bundle includes `Contents/Resources/chrome-extension/`, `Contents/Resources/firefox-extension/`, and the native-host sidecar in `Contents/MacOS`. In Chromium, use Developer mode and **Load unpacked** on `chrome-extension/`; for Firefox setup, see the section above. Local packages are unsigned; CI release packages are ad-hoc signed, not Developer ID-signed or notarized.

Chrome loads the service worker's `importScripts` dependencies from `Contents/Resources/chrome-extension`; Firefox uses the background scripts in `Contents/Resources/firefox-extension`. Rebuild and reinstall the app after updating the extension so both packaged folders are present.

To try the Firefox extension, open `about:debugging#/runtime/this-firefox`, choose **Load Temporary Add-on**, and select `twitch-streak-saver-firefox-extension.xpi`. The Windows package contains `firefox-extension/`; the macOS app contains `Contents/Resources/firefox-extension/`.

Temporary add-ons are removed when Firefox closes. This XPI is unsigned; permanent installation requires a Mozilla-signed add-on.

## macOS app bundle

On Apple Silicon, run `just package-macos` to build `dist/macos/Twitch Streak Saver.app`; this only creates the package. Run `just install-macos` to install or overwrite `/Applications/Twitch Streak Saver.app`; it stages the replacement before removing the existing app. The bundle includes the Chromium extension in `Contents/Resources/chrome-extension` and the native-host sidecar in `Contents/MacOS`. In Chromium, use Developer mode and **Load unpacked** on the extension folder. Local packages are unsigned; CI release packages are ad-hoc signed, not Developer ID-signed or notarized.

Chrome loads the worker's `importScripts` dependencies from `Contents/Resources/chrome-extension`; rebuild and reinstall the app after updating the extension so every imported script is present.

