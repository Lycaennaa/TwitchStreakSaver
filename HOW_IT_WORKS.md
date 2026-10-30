# How it works

## Desktop app

Cross-platform Rust desktop app built with egui, with sidebar pages for the watch queue, followed channels, activity history, and settings.

On startup, the app immediately begins connecting to Twitch instead of waiting for **Connect Twitch**.

The app signs in through Twitch’s browser OAuth flow, stores the token in the macOS Keychain or Windows Credential Manager, loads followed channels, and refreshes live status every minute. A read-only live PubSub listener detects `WATCH_STREAK`, points earned/spent, and bonus-claim availability; events missed while disconnected cannot be reconstructed reliably. Point balances show the latest received event, not a startup snapshot. Bonus claims remain manual. Selected live Streak channels take priority. If no `WATCH_STREAK` arrives, each turn yields after 16 eligible minutes of playing video, ready chat UI, and an available app slot. While Points is scheduled, timed-out Streak channels are deprioritized until a Points turn completes or 16 elapsed minutes pass. Points turns use the same eligibility conditions and advance after 16 eligible minutes. Point, Streak, and automatic stream-switch activity appears in an in-app log with local time, streamer, and event details. Separate player tabs are muted by default. The per-channel Unmuted option starts them with audio enabled. Channel selections and settings remain local. OAuth uses Streamlink Twitch GUI’s public Client ID and fixed localhost callback.

After a PubSub interruption, the app resumes receiving live events but does not recover events missed while disconnected. It flags the interruption as a possible gap and shows when the current live/offline baseline was last confirmed through Twitch's Helix Streams API. This current-state baseline does not reconstruct historical points or Streak events.

Transient Twitch network failures during initial connection and active live/follow refreshes retry with capped exponential backoff (2 seconds to 60 seconds), while the app retains the last confirmed data. PubSub reconnects separately. After a successful Helix snapshot confirms connectivity, the app reloads only app-managed Twitch player tabs and resumes a queued **Open selected channels** request. External tabs are never reloaded, and missed PubSub events are not recovered. Manual open requests made while Twitch or the browser bridge is unavailable wait for connectivity; replay uses channels still selected at that time.

Transient sign-in callback timeouts retry with the same capped backoff. Live-status and followed-channel refreshes recover independently; successful follower updates are applied even if live status is still unavailable, and automation stays paused until every failed Twitch feed recovers. A queued manual open remains pending until the browser extension reports tab inventory; if the extension is connected but has not reported inventory within 10 seconds, the app shows an extension diagnostic while retaining the request.

Managed-player reloads and `OpenSelected` commands are sent only to extension version 0.3.18 or newer, which advertises acknowledgment support. Requests stay pending until acknowledged; after a timeout or disconnect, retries reuse the same command ID and unchanged payload so a lost acknowledgment cannot reopen a user-closed tab. Manual requests that have not yet been sent use the channels selected when they are dispatched.
The extension persists its latest 128 completed recoverable browser command IDs in `chrome.storage.local`, so duplicates received after an MV3 service-worker restart are acknowledged only from known completions.



The followed-channel list is sorted alphabetically by display name, ignoring letter case.

The in-app activity log retains the most recent 500 events locally across launches, including app-managed stream openings. Expand **Daily summaries** to review retained streak, points-event, bonus, and stream activity by date. The log can also be filtered by event type and searched with case-insensitive fuzzy matching across event text, streamer names, and timestamps.

**Select all** buttons continue to apply to every followed channel regardless of the current search. The separate **Select matching** buttons affect only followed channels matching the search (with an empty search, all followed channels match). Each button toggles to **Unselect all** or **Unselect matching** once every channel in its scope has the option selected; unselecting affects only that scope.

## Browser extension and stream automation

**Preferred stream quality** is chosen in the Rust desktop app under **Stream quality**. It defaults to 160p to save bandwidth and applies automatically to Twitch player pages. Choose 160p, 360p, 480p, 720p, 1080p, Source, or Twitch Auto; if a rendition is unavailable, the extension chooses the lowest available quality. Saved quality and per-channel **Unmuted** preferences apply to open Twitch pages and are restored after reload. Refresh already-open Twitch pages after upgrading extension version 0.3.18.
   Audio automation stops when a managed player leaves its assigned channel. Opening another channel in an app-managed player applies that channel's **Unmuted** setting.
The per-channel **Unmuted** preference also synchronizes the Twitch player's own audio controls, not just Chrome's tab mute state.


**Close app-opened stream tabs on Twitch raid redirects** is off by default. When enabled, the extension closes a managed player tab whose URL changes to include the query parameter `referrer=raid`; external tabs are unaffected. The channel stays suppressed from automatic reopening until Twitch confirms it is offline. At browser startup, a managed tab already on a raid URL without a verifiable saved identity (including tabs created before version 0.3.13 or tabs where session storage was blocked) stays open and its channel is held from reopening until the route changes, avoiding false closure of a reused tab ID.

On Windows, **Launch at Windows sign-in** adds or removes this app's per-user startup entry.

On macOS, **Launch at macOS sign-in** registers a per-user LaunchAgent that starts the app hidden. On Windows, the existing sign-in control also starts it hidden. The menu-bar/tray icon provides **Open Twitch Streak Saver** and **Quit** actions. Closing the window exits by default; enable **Keep running in the menu bar/tray when the window closes** to hide it instead. Minimize continues to use the normal Dock/taskbar behavior.

The extension inventories external Twitch channel/player tabs separately from app-managed players; VOD, non-channel, and chat-only pages are ignored. It reports player status and combined-page chat UI readiness, though UI readiness does not prove a live chat connection. External tabs count toward capacity but are not rotated by the app; unknown managed channels reserve capacity conservatively. Reload extension version 0.3.18 after updating. Older extensions cannot report restored app-managed streams, so the app pauses stream opening until inventory is available.
By default, manually opened Twitch tabs remain external and under your control. Enabling **Adopt existing Twitch player tabs** lets the extension adopt at most one eligible existing player tab per followed channel, preferring the active tab; unmatched, chat-only, or identity-unverifiable tabs remain external. Turning the option off releases adopted tabs without closing them.

The extension revalidates tab identity and player route after async adoption and raid-redirect processing; if either no longer matches, adoption is declined or the tab is left open.


**Automatic browser opening** can launch Chrome, Brave, Edge, or Helium with the selected profile when automation is enabled, a followed Twitch channel is live, and the extension is disconnected. Profiles with the extension enabled are marked. If several are enabled, the bridge cannot identify which profile is connected, and an already-connected profile is preserved.

**Queue visibility.** The followed-channel panel shows the upcoming managed Watch Streak or Channel Points queue and the player/chat readiness used for each 16-minute turn timer. Player/chat readiness does not block opening. The queue reports actionable opening blockers—including automation, Twitch/browser connection or tab-inventory state, and full stream capacity—and keeps upcoming channels visible while capacity is full.

**Chat engagement.** The extension checks the Twitch login attached to each rendered chat message against the Twitch account connected in the Rust app. Only matching messages from app-managed streams are excluded from the 16-minute rotation for the current automation run. Message text stays local.

**Temporary pause and snooze.** Pause automatic opening for 15 minutes, 1 hour, or 4 hours; it resumes automatically at expiry and leaves currently app-managed tabs open. Snooze an individual followed channel for the same durations to skip it until expiry; if it is already app-managed, it closes on the next automation update. Neither action changes persistent automation or channel selection. These timers run while the app is open and reset when it exits.

Offline selected channels are omitted from the upcoming managed-channel list until Twitch reports them live.

The extension popup shows bridge connectivity and managed player streams, can pause automatic opening for 15 minutes, and can request the app to open selected channels. It reports when the app cannot apply an action.

Screenshot privacy hides streamer and browser profile names, the connected Twitch login, activity dates and details, searches, channel-selection totals, and variable followed-channel and queue lists. Local browser paths and setup diagnostics remain visible. It changes only the app's on-screen display; it does not alter saved channel data.

In the desktop app, browser and Points & Streak connection statuses are shown on Watch queue; Screenshot privacy is in Settings. Watch queue also lists currently open Twitch player streamers, including tabs not managed by the app; followed channels use display names and other channels use their Twitch login. If the browser extension cannot provide names for open tabs, Watch queue shows that instead of claiming no streams are open.


