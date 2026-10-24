use eframe::egui;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use twitch_streak_saver::browser_bridge::{
    BridgeEvent, BrowserBridge, EXTENSION_REGISTRATION_ADDRESS,
};
use twitch_streak_saver::browser_protocol::{
    BrowserChannel, BrowserCommand, BrowserMessage, BrowserSurface, BrowserTabCloseReason,
    ChatState, KnownBrowserMessage, PlayerState,
};
use twitch_streak_saver::model::{
    AppPreferences, BrowserKind, BrowserLaunchTarget, FollowedChannel, PreferredQuality,
};
use twitch_streak_saver::scheduler::{
    QueueCandidate, QueuePlanner, QueueStage, StreamRotation, StreamSession, TurnEligibility,
    APP_STREAM_LIMIT, STREAM_TURN_DURATION,
};
use twitch_streak_saver::twitch as twitch_api;
use twitch_streak_saver::twitch::{TwitchNetworkSource, TwitchUpdate, TwitchUpdateFailure};
#[path = "main/activity_log.rs"]
mod activity_log;
use activity_log::{
    record_activity, record_claim_available_once, ActivityLogEntry, ActivityLogKind,
};
#[path = "main/automation.rs"]
mod automation;
#[path = "main/browser.rs"]
mod browser;
#[path = "main/browser_connection.rs"]
mod browser_connection;
#[path = "main/browser_setup.rs"]
mod browser_setup;
#[path = "main/channel_list.rs"]
mod channel_list;
#[path = "main/twitch.rs"]
mod twitch;
#[path = "main/ui.rs"]
mod ui;
#[path = "main/windows_startup.rs"]
mod windows_startup;
const AUTO_TAB_RETRY_DELAY: Duration = Duration::from_secs(30);
fn queue_candidates<'a>(
    channels: &'a [FollowedChannel],
    manual_open_channels: &HashSet<String>,
    external_stream_counts: &HashMap<String, usize>,
    live_channel_ids: &HashSet<String>,
    raid_redirected_channels: &HashSet<String>,
    streak_completed: &HashMap<String, u64>,
) -> Vec<QueueCandidate<'a>> {
    let external_channel_ids = external_stream_counts
        .iter()
        .filter(|(channel_id, count)| **count > 0 && live_channel_ids.contains(*channel_id))
        .map(|(channel_id, _)| channel_id.as_str())
        .collect::<HashSet<_>>();

    channels
        .iter()
        .filter(|channel| {
            !manual_open_channels.contains(&channel.id)
                && !external_channel_ids.contains(channel.id.as_str())
                && !raid_redirected_channels.contains(&channel.id)
        })
        .map(|channel| QueueCandidate {
            channel_id: &channel.id,
            is_live: live_channel_ids.contains(&channel.id),
            selection: channel.selection,
            streak_completed_for_live: streak_completed.contains_key(&channel.id),
        })
        .collect()
}

enum TwitchConnectionStatus {
    Disconnected,
    Connecting,
    Connected(String),
}

#[derive(Default)]
struct BrowserChannelState {
    player_tab_id: Option<i64>,
    chat_tab_id: Option<i64>,
    player_state: Option<PlayerState>,
    chat_state: Option<ChatState>,
    player_last_seen: Option<Instant>,
    chat_last_seen: Option<Instant>,
}

const BROWSER_STATE_STALE_AFTER: Duration = Duration::from_secs(15);
fn schedule_auto_open_retry(
    auto_open_channels: &HashSet<String>,
    retry_after: &mut HashMap<String, Instant>,
    channel_id: &str,
    now: Instant,
) -> bool {
    if !auto_open_channels.contains(channel_id) {
        return false;
    }
    retry_after.insert(channel_id.to_owned(), now + AUTO_TAB_RETRY_DELAY);
    true
}

fn resume_due_auto_open_retries(
    auto_open_channels: &mut HashSet<String>,
    retry_after: &mut HashMap<String, Instant>,
    now: Instant,
    defer: bool,
) -> Vec<String> {
    let due = retry_after
        .iter()
        .filter_map(|(channel_id, retry_at)| (*retry_at <= now).then(|| channel_id.clone()))
        .collect::<Vec<_>>();
    if !defer {
        for channel_id in &due {
            auto_open_channels.remove(channel_id);
            retry_after.remove(channel_id);
        }
    }
    due
}

fn stream_turn_session_for(
    channel_id: &str,
    state: Option<&BrowserChannelState>,
    now: Instant,
    capacity_available: bool,
) -> StreamSession {
    let (player_playing, chat_ready) = state.map_or((false, false), |state| {
        let player_fresh = state.player_last_seen.is_some_and(|last_seen| {
            now.saturating_duration_since(last_seen) <= BROWSER_STATE_STALE_AFTER
        });
        let chat_fresh = state.chat_last_seen.is_some_and(|last_seen| {
            now.saturating_duration_since(last_seen) <= BROWSER_STATE_STALE_AFTER
        });
        (
            state.player_tab_id.is_some()
                && player_fresh
                && state.player_state == Some(PlayerState::Playing),
            state.chat_tab_id.is_some() && chat_fresh && state.chat_state == Some(ChatState::Ready),
        )
    });
    StreamSession {
        channel_id: channel_id.to_owned(),
        eligibility: TurnEligibility {
            player_playing,
            chat_ready,
            capacity_available,
        },
    }
}

fn active_external_stream_count(
    channel_counts: &HashMap<String, usize>,
    unmatched_count: usize,
    live_channel_ids: &HashSet<String>,
) -> usize {
    channel_counts
        .iter()
        .filter(|(channel_id, count)| live_channel_ids.contains(*channel_id) && **count > 0)
        .fold(unmatched_count, |total, (_, count)| {
            total.saturating_add(*count)
        })
}

fn active_app_stream_count(
    app_open_channels: &HashSet<String>,
    followed_channels: &[FollowedChannel],
    live_channel_ids: &HashSet<String>,
) -> usize {
    app_open_channels
        .iter()
        .filter(|channel_id| {
            let channel_id = channel_id.as_str();
            live_channel_ids.contains(channel_id)
                || !followed_channels
                    .iter()
                    .any(|channel| channel.id.as_str() == channel_id)
        })
        .count()
}

fn restore_managed_channel_ownership(
    reported_channels: impl IntoIterator<Item = String>,
    auto_open_channels: &mut HashSet<String>,
    manual_open_channels: &mut HashSet<String>,
) {
    let reported_channels = reported_channels
        .into_iter()
        .filter(|channel_id| !channel_id.is_empty())
        .collect::<HashSet<_>>();
    auto_open_channels.retain(|channel_id| reported_channels.contains(channel_id));
    manual_open_channels.retain(|channel_id| reported_channels.contains(channel_id));
    manual_open_channels.extend(reported_channels.difference(auto_open_channels).cloned());
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PendingStreamChannelState {
    Closing,
    Opening,
    Closed,
    Opened,
    Abandoned,
}

struct PendingStreamSwitch {
    streamer: String,
    event: String,
    desired_channels: HashSet<String>,
    channels: HashMap<String, PendingStreamChannelState>,
}

impl PendingStreamSwitch {
    fn confirm_closed(&mut self, channel_id: &str) {
        if let Some(state) = self.channels.get_mut(channel_id) {
            if *state == PendingStreamChannelState::Closing {
                *state = PendingStreamChannelState::Closed;
            }
        }
    }
    fn is_awaiting_close(&self, channel_id: &str) -> bool {
        self.channels.get(channel_id) == Some(&PendingStreamChannelState::Closing)
    }

    fn confirm_opened(&mut self, channel_id: &str) {
        if let Some(state) = self.channels.get_mut(channel_id) {
            if *state == PendingStreamChannelState::Opening {
                *state = PendingStreamChannelState::Opened;
            }
        }
    }
    fn is_awaiting_open(&self, channel_id: &str) -> bool {
        self.channels.get(channel_id) == Some(&PendingStreamChannelState::Opening)
    }
    fn abandon_open(&mut self, channel_id: &str) {
        if let Some(state) = self.channels.get_mut(channel_id) {
            if *state == PendingStreamChannelState::Opening {
                *state = PendingStreamChannelState::Abandoned;
            }
        }
    }

    fn is_complete(&self) -> bool {
        self.channels.values().all(|state| {
            matches!(
                *state,
                PendingStreamChannelState::Closed
                    | PendingStreamChannelState::Opened
                    | PendingStreamChannelState::Abandoned
            )
        })
    }

    fn is_successful(&self) -> bool {
        self.channels.values().all(|state| {
            matches!(
                *state,
                PendingStreamChannelState::Closed | PendingStreamChannelState::Opened
            )
        })
    }
}

fn followed_channel_search_indices(channels: &[FollowedChannel], query: &str) -> Vec<usize> {
    let query = query.trim();
    let mut indices: Vec<usize> = if query.is_empty() {
        (0..channels.len()).collect()
    } else {
        channels
            .iter()
            .enumerate()
            .filter_map(|(index, channel)| {
                (fuzzy_contains(&channel.display_name, query)
                    || fuzzy_contains(&channel.login, query)
                    || fuzzy_contains(&channel.id, query))
                .then_some(index)
            })
            .collect()
    };
    indices.sort_by(|left, right| {
        channels[*left]
            .display_name
            .chars()
            .flat_map(char::to_lowercase)
            .cmp(
                channels[*right]
                    .display_name
                    .chars()
                    .flat_map(char::to_lowercase),
            )
    });
    indices
}

fn fuzzy_contains(candidate: &str, query: &str) -> bool {
    let mut query_chars = query
        .chars()
        .flat_map(|character| character.to_lowercase())
        .peekable();
    for candidate_char in candidate
        .chars()
        .flat_map(|character| character.to_lowercase())
    {
        if query_chars.peek() == Some(&candidate_char) {
            query_chars.next();
        }
    }
    query_chars.peek().is_none()
}

fn selected_channel_indices(channels: &[FollowedChannel], limit: usize) -> Vec<usize> {
    let mut seen = HashSet::new();
    channels
        .iter()
        .enumerate()
        .filter(|(_, channel)| channel.selection.streak && !channel.login.is_empty())
        .chain(
            channels
                .iter()
                .enumerate()
                .filter(|(_, channel)| channel.selection.points && !channel.login.is_empty()),
        )
        .filter(|(_, channel)| seen.insert(channel.id.clone()))
        .take(limit)
        .map(|(index, _)| index)
        .collect()
}
const PREFERENCES_KEY: &str = "preferences";
const ACTIVITY_LOG_KEY: &str = "activity_log";
const BROWSER_EXTENSION_ID: &str = include_str!("../chrome-extension/trusted-extension-id");
const BROWSER_BRIDGE_RETRY_DELAY: Duration = Duration::from_secs(5);

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1120.0, 760.0])
            .with_min_inner_size([800.0, 600.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Twitch Streak Saver",
        options,
        Box::new(|creation_context| Ok(Box::new(Dashboard::new(creation_context)))),
    )
}

#[derive(Clone)]
struct PendingBridgeCommand {
    request_id: String,
    connection_generation: u64,
    sent_at: Option<Instant>,
    command: Option<BrowserCommand>,
}

struct Dashboard {
    #[cfg(target_os = "windows")]
    start_with_windows: bool,
    #[cfg(target_os = "windows")]
    windows_startup_error: Option<String>,
    auto_start_on_launch: bool,
    browser_launch_target: Option<BrowserLaunchTarget>,
    browser_launch_attempted: Option<BrowserLaunchTarget>,
    browser_profiles: Vec<browser_setup::DetectedBrowserProfile>,
    browser_profile_roots: Vec<browser_setup::DetectedBrowserProfileRoot>,
    browser_profile_data_directories: Vec<(BrowserKind, String)>,
    wake_browser_on_open: bool,
    close_on_raid_redirect: bool,
    preferred_quality: PreferredQuality,
    automation_enabled: bool,
    channels: Vec<FollowedChannel>,
    followed_search: String,
    started_at: Instant,
    connection_status: TwitchConnectionStatus,
    connection_error: Option<String>,
    twitch_network_unavailable: bool,
    twitch_network_errors: HashMap<TwitchNetworkSource, String>,
    reload_managed_player_tabs_pending: bool,
    pending_manual_open: bool,
    pending_manual_open_inventory_wait_since: Option<Instant>,
    next_browser_command_id: u64,
    browser_command_session_id: String,
    pending_reload_command: Option<PendingBridgeCommand>,
    pending_manual_open_command: Option<PendingBridgeCommand>,
    live_channel_ids: HashSet<String>,
    connection_generation: u64,
    twitch_cancel: Option<Arc<AtomicBool>>,
    twitch_sender: Sender<TwitchUpdate>,
    twitch_updates: Receiver<TwitchUpdate>,
    browser_port_check_sender: Sender<Result<(), String>>,
    browser_port_check_receiver: Receiver<Result<(), String>>,
    browser_port_check_in_flight: bool,
    browser_bridge: Option<BrowserBridge>,
    browser_bridge_retry_after: Option<Instant>,
    browser_connected: bool,
    browser_command_ack_supported: bool,
    browser_error: Option<String>,
    browser_notice: Option<String>,
    browser_states: HashMap<String, BrowserChannelState>,
    external_stream_counts: HashMap<String, usize>,
    external_unmatched_streams: usize,
    external_inventory_received: bool,
    pubsub_connected: bool,
    pubsub_error: Option<String>,
    twitch_baseline_confirmed_at: Option<Instant>,
    pubsub_state_confirmed_at: Option<Instant>,
    pubsub_last_event_at: Option<Instant>,
    pubsub_missed_events_since: Option<Instant>,
    streak_completed: HashMap<String, u64>,
    notified_streak_channels: HashSet<String>,
    activity_log: VecDeque<ActivityLogEntry>,
    activity_log_kind_filter: Option<ActivityLogKind>,
    activity_log_search: String,
    channel_points_balances: HashMap<String, u64>,
    available_claim_channels: HashSet<String>,
    queue_planner: QueuePlanner,
    points_rotation: StreamRotation,
    streak_rotation: StreamRotation,
    manual_open_channels: HashSet<String>,
    auto_open_channels: HashSet<String>,
    raid_redirected_channels: HashSet<String>,
    removed_auto_streamer_names: HashMap<String, String>,
    pending_stream_switch: Option<PendingStreamSwitch>,
    auto_open_retry_after: HashMap<String, Instant>,
}
impl Dashboard {
    fn new(creation_context: &eframe::CreationContext<'_>) -> Self {
        let preferences = creation_context
            .storage
            .and_then(|storage| eframe::get_value::<AppPreferences>(storage, PREFERENCES_KEY))
            .unwrap_or_default();
        let mut activity_log = creation_context
            .storage
            .and_then(|storage| {
                eframe::get_value::<VecDeque<ActivityLogEntry>>(storage, ACTIVITY_LOG_KEY)
            })
            .unwrap_or_default();
        activity_log.truncate(activity_log::MAX_ACTIVITY_LOG_ENTRIES);
        let browser_profile_data_directories = preferences.browser_profile_data_directories;
        let browser_profile_roots =
            browser_setup::detected_browser_profile_roots(&browser_profile_data_directories);
        let browser_profiles =
            browser_setup::detected_browser_profiles_for_roots(&browser_profile_roots);
        #[cfg(target_os = "windows")]
        let (start_with_windows, windows_startup_error) = windows_startup::initialize();
        let (browser_bridge, browser_error) = match BrowserBridge::start() {
            Ok(bridge) => (Some(bridge), None),
            Err(error) => (None, Some(error)),
        };
        let browser_bridge_retry_after = browser_bridge
            .is_none()
            .then(|| Instant::now() + BROWSER_BRIDGE_RETRY_DELAY);
        let browser_notice = browser_error
            .as_ref()
            .map(|error| format!("Browser bridge unavailable; retrying automatically: {error}"));
        let (twitch_sender, twitch_updates) = mpsc::channel();
        let (browser_port_check_sender, browser_port_check_receiver) = mpsc::channel();

        let mut dashboard = Self {
            #[cfg(target_os = "windows")]
            start_with_windows,
            #[cfg(target_os = "windows")]
            windows_startup_error,
            auto_start_on_launch: preferences.auto_start_on_launch,
            browser_launch_target: preferences.browser_launch_target,
            browser_launch_attempted: None,
            browser_profiles,
            browser_profile_roots,
            browser_profile_data_directories,
            wake_browser_on_open: preferences.wake_browser_on_open,
            close_on_raid_redirect: preferences.close_on_raid_redirect,
            preferred_quality: preferences.preferred_quality,
            automation_enabled: preferences.auto_start_on_launch,
            channels: preferences.channels,
            followed_search: String::new(),
            started_at: Instant::now(),
            connection_status: TwitchConnectionStatus::Disconnected,
            connection_error: None,
            twitch_network_unavailable: false,
            twitch_network_errors: HashMap::new(),
            reload_managed_player_tabs_pending: false,
            pending_manual_open: false,
            pending_manual_open_inventory_wait_since: None,
            next_browser_command_id: 1,
            browser_command_session_id: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default()
                .to_string(),
            pending_reload_command: None,
            pending_manual_open_command: None,
            live_channel_ids: HashSet::new(),
            connection_generation: 0,
            twitch_cancel: None,
            twitch_sender,
            twitch_updates,
            browser_bridge,
            browser_bridge_retry_after,
            browser_connected: false,
            browser_command_ack_supported: false,
            browser_error,
            browser_port_check_sender,
            browser_port_check_receiver,
            browser_port_check_in_flight: false,
            browser_notice,
            browser_states: HashMap::new(),
            external_stream_counts: HashMap::new(),
            external_unmatched_streams: 0,
            external_inventory_received: false,
            pubsub_connected: false,
            pubsub_error: None,
            twitch_baseline_confirmed_at: None,
            pubsub_state_confirmed_at: None,
            pubsub_last_event_at: None,
            pubsub_missed_events_since: None,
            streak_completed: HashMap::new(),
            notified_streak_channels: HashSet::new(),
            activity_log,
            activity_log_kind_filter: None,
            activity_log_search: String::new(),
            channel_points_balances: HashMap::new(),
            available_claim_channels: HashSet::new(),
            queue_planner: QueuePlanner::default(),
            points_rotation: StreamRotation::default(),
            streak_rotation: StreamRotation::default(),
            manual_open_channels: HashSet::new(),
            auto_open_channels: HashSet::new(),
            raid_redirected_channels: HashSet::new(),
            removed_auto_streamer_names: HashMap::new(),
            auto_open_retry_after: HashMap::new(),
            pending_stream_switch: None,
        };
        dashboard.start_twitch_connection();
        dashboard
    }

    fn preferences(&self) -> AppPreferences {
        AppPreferences {
            auto_start_on_launch: self.auto_start_on_launch,
            wake_browser_on_open: self.wake_browser_on_open,
            close_on_raid_redirect: self.close_on_raid_redirect,
            preferred_quality: self.preferred_quality,
            browser_launch_target: self.browser_launch_target.clone(),
            browser_profile_data_directories: self.browser_profile_data_directories.clone(),
            channels: self.channels.clone(),
        }
    }
}

impl Drop for Dashboard {
    fn drop(&mut self) {
        if let Some(cancel) = self.twitch_cancel.take() {
            cancel.store(true, Ordering::Release);
        }
    }
}

#[cfg(test)]
#[path = "main/stream_switch_tests.rs"]
mod stream_switch_tests;
#[cfg(test)]
#[path = "main/tests.rs"]
mod tests;
