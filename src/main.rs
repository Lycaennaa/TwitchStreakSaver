use eframe::egui;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use twitch_streak_saver::browser_bridge::{BridgeEvent, BrowserBridge};
use twitch_streak_saver::model::{
    AppPreferences, FollowedChannel, SchedulingMode, TwitchTabLayout,
};
use twitch_streak_saver::scheduler::{
    PointsEligibility, PointsRotation, PointsSession, QueueCandidate, QueuePlanner, QueueStage,
    POINTS_TURN_DURATION,
};
use twitch_streak_saver::twitch::{self, TwitchUpdate};
const AUTO_TAB_RETRY_DELAY: Duration = Duration::from_secs(30);

enum TwitchConnectionStatus {
    Disconnected,
    Connecting,
    Connected(String),
}

#[derive(Default)]
struct BrowserChannelState {
    player_tab_id: Option<i64>,
    chat_tab_id: Option<i64>,
    player_state: String,
    chat_state: String,
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
) {
    let due = retry_after
        .iter()
        .filter_map(|(channel_id, retry_at)| {
            (auto_open_channels.contains(channel_id) && *retry_at <= now)
                .then(|| channel_id.clone())
        })
        .collect::<Vec<_>>();
    for channel_id in due {
        auto_open_channels.remove(&channel_id);
        retry_after.remove(&channel_id);
    }
}

fn points_session_for(
    channel_id: &str,
    state: Option<&BrowserChannelState>,
    now: Instant,
    capacity_available: bool,
) -> PointsSession {
    let (player_playing, chat_ready) = state.map_or((false, false), |state| {
        let player_fresh = state.player_last_seen.is_some_and(|last_seen| {
            now.saturating_duration_since(last_seen) <= BROWSER_STATE_STALE_AFTER
        });
        let chat_fresh = state.chat_last_seen.is_some_and(|last_seen| {
            now.saturating_duration_since(last_seen) <= BROWSER_STATE_STALE_AFTER
        });
        (
            state.player_tab_id.is_some() && player_fresh && state.player_state == "playing",
            state.chat_tab_id.is_some() && chat_fresh && state.chat_state == "ready",
        )
    });
    PointsSession {
        channel_id: channel_id.to_owned(),
        eligibility: PointsEligibility {
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

#[derive(Clone)]
struct StreakNotice {
    channel_name: String,
    points_earned: u64,
}

#[derive(Clone)]
enum PointsNotice {
    TurnCompleted {
        channel_name: String,
    },
    Earned {
        channel_name: String,
        points: u64,
        reason_code: String,
        balance: u64,
    },
    Spent {
        channel_name: String,
        balance: u64,
    },
    ClaimAvailable {
        channel_name: String,
    },
}

const MAX_QUEUED_ALERTS: usize = 8;

fn enqueue_points_notice(notices: &mut VecDeque<PointsNotice>, notice: PointsNotice) {
    if notices.len() >= MAX_QUEUED_ALERTS {
        notices.pop_front();
    }
    notices.push_back(notice);
}

fn followed_channel_search_indices(channels: &[FollowedChannel], query: &str) -> Vec<usize> {
    let query = query.trim();
    if query.is_empty() {
        return (0..channels.len()).collect();
    }
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
}

fn fuzzy_contains(candidate: &str, query: &str) -> bool {
    let query = query.to_lowercase();
    let mut query_chars = query.chars().peekable();
    for candidate_char in candidate.to_lowercase().chars() {
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
const EXTENSION_ID_KEY: &str = "chrome_extension_id";

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

struct Dashboard {
    mode: SchedulingMode,
    auto_start_on_launch: bool,
    tab_layout: TwitchTabLayout,
    wake_browser_on_open: bool,
    automation_enabled: bool,
    channels: Vec<FollowedChannel>,
    followed_search: String,
    started_at: Instant,
    connection_status: TwitchConnectionStatus,
    connection_error: Option<String>,
    live_channel_ids: HashSet<String>,
    connection_generation: u64,
    twitch_cancel: Option<Arc<AtomicBool>>,
    twitch_sender: Sender<TwitchUpdate>,
    twitch_updates: Receiver<TwitchUpdate>,
    browser_bridge: Option<BrowserBridge>,
    browser_connected: bool,
    browser_error: Option<String>,
    browser_notice: Option<String>,
    extension_id: String,
    browser_states: HashMap<String, BrowserChannelState>,
    external_stream_counts: HashMap<String, usize>,
    external_unmatched_streams: usize,
    external_inventory_received: bool,
    pubsub_connected: bool,
    pubsub_error: Option<String>,
    streak_completed: HashMap<String, u64>,
    notified_streak_channels: HashSet<String>,
    streak_notices: VecDeque<StreakNotice>,
    channel_points_balances: HashMap<String, u64>,
    available_claim_channels: HashSet<String>,
    points_notices: VecDeque<PointsNotice>,
    queue_planner: QueuePlanner,
    points_rotation: PointsRotation,
    manual_open_channels: HashSet<String>,
    auto_open_channels: HashSet<String>,
    auto_open_retry_after: HashMap<String, Instant>,
}
impl Dashboard {
    fn new(creation_context: &eframe::CreationContext<'_>) -> Self {
        let preferences = creation_context
            .storage
            .and_then(|storage| eframe::get_value::<AppPreferences>(storage, PREFERENCES_KEY))
            .unwrap_or_default();
        let extension_id = creation_context
            .storage
            .and_then(|storage| eframe::get_value::<String>(storage, EXTENSION_ID_KEY))
            .unwrap_or_default();
        let (browser_bridge, browser_error) = match BrowserBridge::start() {
            Ok(bridge) => (Some(bridge), None),
            Err(error) => (None, Some(error)),
        };
        let (twitch_sender, twitch_updates) = mpsc::channel();

        Self {
            mode: preferences.mode,
            auto_start_on_launch: preferences.auto_start_on_launch,
            tab_layout: preferences.tab_layout,
            wake_browser_on_open: preferences.wake_browser_on_open,
            automation_enabled: preferences.auto_start_on_launch,
            channels: preferences.channels,
            followed_search: String::new(),
            started_at: Instant::now(),
            connection_status: TwitchConnectionStatus::Disconnected,
            connection_error: None,
            live_channel_ids: HashSet::new(),
            connection_generation: 0,
            twitch_cancel: None,
            twitch_sender,
            twitch_updates,
            browser_bridge,
            browser_connected: false,
            browser_error,
            browser_notice: None,
            extension_id,
            browser_states: HashMap::new(),
            external_stream_counts: HashMap::new(),
            external_unmatched_streams: 0,
            external_inventory_received: false,
            pubsub_connected: false,
            pubsub_error: None,
            streak_completed: HashMap::new(),
            notified_streak_channels: HashSet::new(),
            streak_notices: VecDeque::new(),
            channel_points_balances: HashMap::new(),
            available_claim_channels: HashSet::new(),
            points_notices: VecDeque::new(),
            queue_planner: QueuePlanner::default(),
            points_rotation: PointsRotation::default(),
            manual_open_channels: HashSet::new(),
            auto_open_channels: HashSet::new(),
            auto_open_retry_after: HashMap::new(),
        }
    }

    fn preferences(&self) -> AppPreferences {
        AppPreferences {
            mode: self.mode,
            auto_start_on_launch: self.auto_start_on_launch,
            tab_layout: self.tab_layout,
            wake_browser_on_open: self.wake_browser_on_open,
            channels: self.channels.clone(),
        }
    }

    fn start_twitch_connection(&mut self) {
        if let Some(cancel) = self.twitch_cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        self.connection_generation = self.connection_generation.wrapping_add(1);
        let generation = self.connection_generation;
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let updates = self.twitch_sender.clone();
        self.connection_status = TwitchConnectionStatus::Connecting;
        self.connection_error = None;
        self.live_channel_ids.clear();
        self.twitch_cancel = Some(cancel);

        if let Err(error) = thread::Builder::new()
            .name("twitch-connection".to_owned())
            .spawn(move || twitch::run_connection(worker_cancel, generation, updates))
        {
            self.twitch_cancel = None;
            self.connection_status = TwitchConnectionStatus::Disconnected;
            self.connection_error = Some(format!("Could not start Twitch connection: {error}"));
        }
    }

    fn disconnect_twitch(&mut self) {
        if let Some(cancel) = self.twitch_cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        self.connection_generation = self.connection_generation.wrapping_add(1);
        self.connection_status = TwitchConnectionStatus::Disconnected;
        self.pubsub_connected = false;
        self.pubsub_error = None;
        self.live_channel_ids.clear();
        self.streak_completed.clear();
        self.notified_streak_channels.clear();
        self.queue_planner = QueuePlanner::default();
        self.points_rotation = PointsRotation::default();
        let close_ids = self
            .auto_open_channels
            .difference(&self.manual_open_channels)
            .cloned()
            .collect::<Vec<_>>();
        self.auto_open_channels.clear();
        self.auto_open_retry_after.clear();
        if !close_ids.is_empty() {
            self.send_bridge_command(json!({"type": "close_channels", "channel_ids": close_ids}));
        }
        self.connection_error = twitch::clear_saved_token().err();
    }

    fn replace_followed_channels(&mut self, incoming: Vec<FollowedChannel>) {
        self.channels =
            twitch_streak_saver::model::merge_followed_channels(&self.channels, incoming);
        let followed_ids = self
            .channels
            .iter()
            .map(|channel| channel.id.as_str())
            .collect::<HashSet<_>>();
        self.channel_points_balances
            .retain(|channel_id, _| followed_ids.contains(channel_id.as_str()));
        self.available_claim_channels
            .retain(|channel_id| followed_ids.contains(channel_id.as_str()));
    }

    fn poll_twitch_updates(&mut self) {
        while let Ok(update) = self.twitch_updates.try_recv() {
            match update {
                TwitchUpdate::Connected {
                    generation,
                    login,
                    channels,
                    live_channel_ids,
                } if generation == self.connection_generation => {
                    self.replace_followed_channels(channels);
                    self.streak_completed
                        .retain(|channel_id, _| live_channel_ids.contains(channel_id));
                    self.notified_streak_channels
                        .retain(|channel_id| live_channel_ids.contains(channel_id));
                    self.live_channel_ids = live_channel_ids;
                    self.connection_status = TwitchConnectionStatus::Connected(login);
                    self.connection_error = None;
                    self.sync_extension_channels();
                }
                TwitchUpdate::Refreshed {
                    generation,
                    channels,
                    live_channel_ids,
                } if generation == self.connection_generation => {
                    self.replace_followed_channels(channels);
                    self.streak_completed
                        .retain(|channel_id, _| live_channel_ids.contains(channel_id));
                    self.notified_streak_channels
                        .retain(|channel_id| live_channel_ids.contains(channel_id));
                    self.live_channel_ids = live_channel_ids;
                    self.connection_error = None;
                    self.sync_extension_channels();
                }
                TwitchUpdate::LiveStatus {
                    generation,
                    live_channel_ids,
                } if generation == self.connection_generation => {
                    self.streak_completed
                        .retain(|channel_id, _| live_channel_ids.contains(channel_id));
                    self.notified_streak_channels
                        .retain(|channel_id| live_channel_ids.contains(channel_id));
                    self.live_channel_ids = live_channel_ids;
                    self.connection_error = None;
                }
                TwitchUpdate::PubSubStatus {
                    generation,
                    connected,
                    detail,
                } if generation == self.connection_generation => {
                    if connected && !self.pubsub_connected {
                        self.available_claim_channels.clear();
                    }
                    self.pubsub_connected = connected;
                    self.pubsub_error = detail;
                }
                TwitchUpdate::StreakCompleted {
                    generation,
                    channel_id,
                    points_earned,
                } if generation == self.connection_generation => {
                    if let Some(channel_name) = self
                        .channels
                        .iter()
                        .find(|channel| channel.id == channel_id)
                        .map(|channel| channel.display_name.clone())
                    {
                        if self
                            .streak_completed
                            .insert(channel_id.clone(), points_earned)
                            .is_none()
                        {
                            self.queue_planner
                                .record_streak_completed(channel_id.clone());
                        }
                        if self.notified_streak_channels.insert(channel_id) {
                            if self.streak_notices.len() >= MAX_QUEUED_ALERTS {
                                self.streak_notices.pop_front();
                            }
                            self.streak_notices.push_back(StreakNotice {
                                channel_name,
                                points_earned,
                            });
                        }
                    }
                }
                TwitchUpdate::PointsEarned {
                    generation,
                    channel_id,
                    points_earned,
                    balance,
                    reason_code,
                } if generation == self.connection_generation => {
                    let Some(channel_name) = self
                        .channels
                        .iter()
                        .find(|channel| channel.id == channel_id)
                        .map(|channel| channel.display_name.clone())
                    else {
                        continue;
                    };
                    let balance_changed = self
                        .channel_points_balances
                        .insert(channel_id.clone(), balance)
                        != Some(balance);
                    if reason_code == "CLAIM" {
                        self.available_claim_channels.remove(&channel_id);
                    }
                    if balance_changed && reason_code != "WATCH_STREAK" {
                        enqueue_points_notice(
                            &mut self.points_notices,
                            PointsNotice::Earned {
                                channel_name,
                                points: points_earned,
                                reason_code,
                                balance,
                            },
                        );
                    }
                }
                TwitchUpdate::PointsSpent {
                    generation,
                    channel_id,
                    balance,
                } if generation == self.connection_generation => {
                    let Some(channel_name) = self
                        .channels
                        .iter()
                        .find(|channel| channel.id == channel_id)
                        .map(|channel| channel.display_name.clone())
                    else {
                        continue;
                    };
                    if self.channel_points_balances.insert(channel_id, balance) != Some(balance) {
                        enqueue_points_notice(
                            &mut self.points_notices,
                            PointsNotice::Spent {
                                channel_name,
                                balance,
                            },
                        );
                    }
                }
                TwitchUpdate::ClaimAvailable {
                    generation,
                    channel_id,
                } if generation == self.connection_generation => {
                    let Some(channel_name) = self
                        .channels
                        .iter()
                        .find(|channel| channel.id == channel_id)
                        .map(|channel| channel.display_name.clone())
                    else {
                        continue;
                    };
                    if self.available_claim_channels.insert(channel_id) {
                        enqueue_points_notice(
                            &mut self.points_notices,
                            PointsNotice::ClaimAvailable { channel_name },
                        );
                    }
                }
                TwitchUpdate::Warning {
                    generation,
                    message,
                } if generation == self.connection_generation => {
                    self.connection_error = Some(message);
                }
                TwitchUpdate::Failed {
                    generation,
                    message,
                } if generation == self.connection_generation => {
                    self.twitch_cancel = None;
                    self.connection_status = TwitchConnectionStatus::Disconnected;
                    self.live_channel_ids.clear();
                    self.pubsub_connected = false;
                    self.pubsub_error = None;
                    self.streak_completed.clear();
                    self.notified_streak_channels.clear();
                    self.queue_planner = QueuePlanner::default();
                    let close_ids = self
                        .auto_open_channels
                        .difference(&self.manual_open_channels)
                        .cloned()
                        .collect::<Vec<_>>();
                    self.auto_open_channels.clear();
                    self.auto_open_retry_after.clear();
                    if !close_ids.is_empty() {
                        self.send_bridge_command(
                            json!({"type": "close_channels", "channel_ids": close_ids}),
                        );
                    }
                    self.connection_error = Some(message);
                }
                _ => {}
            }
        }
    }
    fn poll_browser_bridge(&mut self) {
        let mut events = Vec::new();
        if let Some(bridge) = &self.browser_bridge {
            while let Ok(event) = bridge.try_recv() {
                events.push(event);
            }
        }
        for event in events {
            match event {
                BridgeEvent::Connected => {
                    self.browser_connected = true;
                    self.external_stream_counts.clear();
                    self.external_unmatched_streams = 0;
                    self.external_inventory_received = false;
                    self.auto_open_channels.clear();
                    self.auto_open_retry_after.clear();
                    self.browser_error = None;
                    self.browser_notice = None;
                    self.send_bridge_command(json!({"type": "bridge_ready"}));
                    self.sync_extension_channels();
                }
                BridgeEvent::Disconnected => {
                    self.browser_connected = false;
                    self.external_stream_counts.clear();
                    self.external_unmatched_streams = 0;
                    self.external_inventory_received = false;
                    self.browser_states.clear();
                    self.browser_notice = Some("Browser extension disconnected.".to_owned());
                    self.points_rotation
                        .pause_active_sessions(self.started_at.elapsed());
                }
                BridgeEvent::Message(message) => self.handle_browser_message(message),
                BridgeEvent::Failed(message) => self.browser_error = Some(message),
            }
        }
    }

    fn handle_browser_message(&mut self, message: Value) {
        let message_type = message
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match message_type {
            "external_streams" => {
                let Some(channel_counts) = message.get("channel_counts").and_then(Value::as_object)
                else {
                    return;
                };
                let Some(unmatched_count) = message
                    .get("unmatched_count")
                    .and_then(Value::as_u64)
                    .and_then(|count| usize::try_from(count).ok())
                else {
                    return;
                };
                let known_channel_ids = self
                    .channels
                    .iter()
                    .map(|channel| channel.id.as_str())
                    .collect::<HashSet<_>>();
                self.external_stream_counts = channel_counts
                    .iter()
                    .filter_map(|(channel_id, count)| {
                        let count = count
                            .as_u64()
                            .and_then(|count| usize::try_from(count).ok())?;
                        (count > 0 && known_channel_ids.contains(channel_id.as_str()))
                            .then(|| (channel_id.clone(), count))
                    })
                    .collect();
                self.external_unmatched_streams = unmatched_count;
                self.external_inventory_received = true;
            }
            "ready" => {
                self.sync_extension_channels();
                self.apply_tab_layout_to_managed_tabs();
            }
            "tab_opened" | "channel_status" => {
                let channel_id = message
                    .get("channel_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let tab_id = message.get("tab_id").and_then(Value::as_i64);
                let surface = message
                    .get("surface")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if channel_id.is_empty()
                    || tab_id.is_none_or(|id| id <= 0)
                    || !["player", "chat"].contains(&surface)
                    || !self.channels.iter().any(|channel| channel.id == channel_id)
                {
                    return;
                }
                if message_type == "tab_opened" {
                    self.auto_open_retry_after.remove(channel_id);
                }
                let tab_id = tab_id.unwrap_or_default();
                let state = self
                    .browser_states
                    .entry(channel_id.to_owned())
                    .or_default();
                match surface {
                    "player" => {
                        if state.player_tab_id != Some(tab_id) {
                            state.player_state = "waiting".to_owned();
                            state.player_last_seen = None;
                        }
                        state.player_tab_id = Some(tab_id);
                        if message_type == "channel_status" {
                            let player_state = message
                                .get("player_state")
                                .and_then(Value::as_str)
                                .unwrap_or_default();
                            if ["playing", "paused", "buffering", "ended", "missing"]
                                .contains(&player_state)
                            {
                                state.player_state = player_state.to_owned();
                                state.player_last_seen = Some(Instant::now());
                            }
                            if let Some(chat_state) =
                                message.get("chat_state").and_then(Value::as_str)
                            {
                                if ["ready", "loading", "missing"].contains(&chat_state) {
                                    state.chat_tab_id = Some(tab_id);
                                    state.chat_state = chat_state.to_owned();
                                    state.chat_last_seen = Some(Instant::now());
                                }
                            }
                        }
                    }
                    "chat" => {
                        if state.chat_tab_id != Some(tab_id) {
                            state.chat_state = "waiting".to_owned();
                            state.chat_last_seen = None;
                        }
                        state.chat_tab_id = Some(tab_id);
                        if message_type == "channel_status" {
                            let chat_state = message
                                .get("chat_state")
                                .and_then(Value::as_str)
                                .unwrap_or_default();
                            if ["ready", "loading", "missing"].contains(&chat_state) {
                                state.chat_state = chat_state.to_owned();
                                state.chat_last_seen = Some(Instant::now());
                            }
                        }
                    }
                    _ => {}
                }
            }
            "tab_closed" => {
                let channel_id = message
                    .get("channel_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let tab_id = message.get("tab_id").and_then(Value::as_i64);
                let surface = message
                    .get("surface")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if channel_id.is_empty()
                    || tab_id.is_none_or(|id| id <= 0)
                    || !["player", "chat"].contains(&surface)
                {
                    return;
                }
                let remove_state = if let Some(state) = self.browser_states.get_mut(channel_id) {
                    match surface {
                        "player" if state.player_tab_id == tab_id => {
                            state.player_tab_id = None;
                            state.player_state = "closed".to_owned();
                            state.player_last_seen = None;
                        }
                        "chat" if state.chat_tab_id == tab_id => {
                            state.chat_tab_id = None;
                            state.chat_state = "closed".to_owned();
                            state.chat_last_seen = None;
                        }
                        _ => return,
                    }
                    state.player_tab_id.is_none() && state.chat_tab_id.is_none()
                } else {
                    false
                };
                if remove_state {
                    self.browser_states.remove(channel_id);
                    self.manual_open_channels.remove(channel_id);
                }
                self.auto_open_channels.remove(channel_id);
                self.auto_open_retry_after.remove(channel_id);
            }
            "error" => {
                if let Some(channel_id) = message.get("channel_id").and_then(Value::as_str) {
                    schedule_auto_open_retry(
                        &self.auto_open_channels,
                        &mut self.auto_open_retry_after,
                        channel_id,
                        Instant::now(),
                    );
                }
                if let Some(message) = message.get("message").and_then(Value::as_str) {
                    self.browser_error = Some(message.chars().take(240).collect());
                }
            }
            _ => {}
        }
    }

    fn send_bridge_command(&mut self, command: Value) {
        if !self.browser_connected {
            self.browser_error = Some("Browser extension is not connected.".to_owned());
            return;
        }
        if let Some(bridge) = &self.browser_bridge {
            if let Err(error) = bridge.send(command) {
                self.browser_error = Some(error);
            }
        } else {
            self.browser_error = Some("Browser bridge listener is unavailable.".to_owned());
        }
    }

    fn sync_extension_channels(&mut self) {
        if !self.browser_connected {
            return;
        }
        self.external_inventory_received = false;
        let channels = self
            .channels
            .iter()
            .filter(|channel| !channel.login.is_empty())
            .map(|channel| json!({"id": channel.id, "login": channel.login}))
            .collect::<Vec<_>>();
        self.send_bridge_command(json!({"type": "sync_channels", "channels": channels}));
    }

    fn open_selected_twitch_channels(&mut self, layout: TwitchTabLayout) {
        if !self.browser_connected {
            self.browser_error = Some("Browser extension is not connected.".to_owned());
            return;
        }
        if !self.external_inventory_received {
            self.browser_notice =
                Some("Scanning existing Twitch tabs; try again shortly.".to_owned());
            return;
        }

        let limit = self.mode.app_stream_limit();
        let selected_count = self
            .channels
            .iter()
            .filter(|channel| channel.selection.is_selected() && !channel.login.is_empty())
            .count();
        let mut app_open_channels = self.manual_open_channels.clone();
        app_open_channels.extend(self.auto_open_channels.iter().cloned());
        let app_open_streams = app_open_channels
            .iter()
            .filter(|channel_id| self.live_channel_ids.contains(*channel_id))
            .count();
        let external_streams = active_external_stream_count(
            &self.external_stream_counts,
            self.external_unmatched_streams,
            &self.live_channel_ids,
        );
        let available_slots =
            limit.saturating_sub(app_open_streams.saturating_add(external_streams));
        let external_channel_ids = self
            .external_stream_counts
            .iter()
            .filter(|(channel_id, count)| {
                **count > 0 && self.live_channel_ids.contains(*channel_id)
            })
            .map(|(channel_id, _)| channel_id.clone())
            .collect::<HashSet<_>>();
        let mut additional_channels = 0;
        let selected_indices = selected_channel_indices(&self.channels, self.channels.len())
            .into_iter()
            .filter(|index| {
                let channel_id = &self.channels[*index].id;
                if app_open_channels.contains(channel_id) {
                    true
                } else if external_channel_ids.contains(channel_id) {
                    false
                } else if additional_channels < available_slots {
                    additional_channels += 1;
                    true
                } else {
                    false
                }
            })
            .collect::<Vec<_>>();
        if selected_indices.is_empty() {
            self.browser_error = Some(if selected_count == 0 {
                "Select followed Twitch channels to open.".to_owned()
            } else {
                "No stream slots are available after counting existing Twitch tabs.".to_owned()
            });
            return;
        }

        let channels = selected_indices
            .iter()
            .map(|index| &self.channels[*index])
            .map(|channel| json!({"id": channel.id, "login": channel.login}))
            .collect::<Vec<_>>();
        let opened_count = channels.len();
        self.manual_open_channels.extend(
            selected_indices
                .iter()
                .map(|index| self.channels[*index].id.clone()),
        );
        self.send_bridge_command(json!({
            "type": "open_selected",
            "layout": layout.as_protocol_value(),
            "wake_browser": self.wake_browser_on_open,
            "channels": channels,
        }));
        let view = match layout {
            TwitchTabLayout::SeparatePlayerChat => "separate player + chat tabs",
            TwitchTabLayout::RegularWebsite => "regular Twitch pages",
        };
        self.browser_notice = Some(format!(
            "Opening {view} for {} of {selected_count} selected channels. App-opened player audio is muted.",
            opened_count,
        ));
    }
    fn apply_tab_layout_to_managed_tabs(&mut self) {
        if !self.browser_connected {
            return;
        }
        self.send_bridge_command(json!({
            "type": "apply_layout",
            "layout": self.tab_layout.as_protocol_value(),
            "wake_browser": self.wake_browser_on_open,
        }));
        self.browser_notice =
            Some("Applying the selected layout to app-managed Twitch tabs.".to_owned());
    }

    fn start_automation(&mut self) {
        self.automation_enabled = true;
    }

    fn stop_automation(&mut self) {
        self.automation_enabled = false;
        self.manual_open_channels.clear();
        self.auto_open_channels.clear();
        self.auto_open_retry_after.clear();
        self.points_rotation = PointsRotation::default();
        self.send_bridge_command(json!({"type": "close_all"}));
        self.browser_notice = Some("Automation stopped and app-managed tabs closed.".to_owned());
    }

    fn reconcile_automatic_tabs(&mut self, now: Duration) {
        if !self.automation_enabled
            || !self.browser_connected
            || !self.external_inventory_received
            || !matches!(
                &self.connection_status,
                TwitchConnectionStatus::Connected(_)
            )
        {
            self.points_rotation.pause_active_sessions(now);
            return;
        }
        resume_due_auto_open_retries(
            &mut self.auto_open_channels,
            &mut self.auto_open_retry_after,
            Instant::now(),
        );

        let external_streams = active_external_stream_count(
            &self.external_stream_counts,
            self.external_unmatched_streams,
            &self.live_channel_ids,
        );
        let manually_open_streams = self
            .manual_open_channels
            .iter()
            .filter(|channel_id| self.live_channel_ids.contains(*channel_id))
            .count();
        let manual_active_streams = manually_open_streams.saturating_add(external_streams);
        let external_channel_ids = self
            .external_stream_counts
            .iter()
            .filter(|(channel_id, count)| {
                **count > 0 && self.live_channel_ids.contains(*channel_id)
            })
            .map(|(channel_id, _)| channel_id.clone())
            .collect::<HashSet<_>>();
        let candidates = self
            .channels
            .iter()
            .filter(|channel| {
                !self.manual_open_channels.contains(&channel.id)
                    && !external_channel_ids.contains(&channel.id)
            })
            .map(|channel| QueueCandidate {
                channel_id: channel.id.clone(),
                is_live: self.live_channel_ids.contains(&channel.id),
                selection: channel.selection,
                streak_completed_for_live: self.streak_completed.contains_key(&channel.id),
            })
            .collect::<Vec<_>>();
        let mut plan = self
            .queue_planner
            .plan(&candidates, self.mode, manual_active_streams);

        match plan.stage {
            QueueStage::Points if !plan.channel_ids.is_empty() => {
                let browser_now = Instant::now();
                let sessions = plan
                    .channel_ids
                    .iter()
                    .map(|channel_id| {
                        points_session_for(
                            channel_id,
                            self.browser_states.get(channel_id),
                            browser_now,
                            true,
                        )
                    })
                    .collect::<Vec<_>>();
                match self.points_rotation.observe_active_sessions(&sessions, now) {
                    Ok(completed) if !completed.is_empty() => {
                        for channel_id in completed {
                            self.queue_planner
                                .record_points_turn_finished(channel_id.clone());
                            self.points_rotation.reset_completed_turn(&channel_id);
                            if let Some(channel_name) = self
                                .channels
                                .iter()
                                .find(|channel| channel.id == channel_id)
                                .map(|channel| channel.display_name.clone())
                            {
                                enqueue_points_notice(
                                    &mut self.points_notices,
                                    PointsNotice::TurnCompleted { channel_name },
                                );
                            }
                        }
                        plan =
                            self.queue_planner
                                .plan(&candidates, self.mode, manual_active_streams);
                    }
                    Ok(_) => {}
                    Err(error) => {
                        self.browser_error = Some(format!("Points rotation paused: {error:?}."));
                        self.points_rotation.pause_active_sessions(now);
                        plan.channel_ids.clear();
                    }
                }
            }
            QueueStage::Points => {
                self.points_rotation.pause_active_sessions(now);
            }
            QueueStage::Streak => self.points_rotation.pause_active_sessions(now),
            QueueStage::Idle => {
                let _ = self.points_rotation.observe_active_sessions(&[], now);
            }
        }

        let desired = if matches!(plan.stage, QueueStage::Streak | QueueStage::Points) {
            plan.channel_ids.into_iter().collect::<HashSet<_>>()
        } else {
            HashSet::new()
        };
        let close_ids = self
            .auto_open_channels
            .difference(&desired)
            .filter(|channel_id| !self.manual_open_channels.contains(*channel_id))
            .cloned()
            .collect::<Vec<_>>();
        let open_channels = desired
            .difference(&self.auto_open_channels)
            .filter(|channel_id| !self.auto_open_retry_after.contains_key(*channel_id))
            .filter_map(|channel_id| {
                self.channels
                    .iter()
                    .find(|channel| channel.id == *channel_id)
                    .map(|channel| json!({"id": channel.id, "login": channel.login}))
            })
            .collect::<Vec<_>>();
        self.auto_open_retry_after
            .retain(|channel_id, _| desired.contains(channel_id));

        if !close_ids.is_empty() {
            self.send_bridge_command(json!({"type": "close_channels", "channel_ids": close_ids}));
        }
        if !open_channels.is_empty() {
            self.send_bridge_command(json!({
                "type": "open_selected",
                "layout": self.tab_layout.as_protocol_value(),
                "wake_browser": self.wake_browser_on_open,
                "channels": open_channels,
            }));
        }
        self.auto_open_channels = desired;
    }

    fn install_chrome_native_host(&mut self) {
        match twitch_streak_saver::browser_bridge::install_native_host(&self.extension_id) {
            Ok(path) => {
                self.browser_error = None;
                self.browser_notice = Some(format!(
                    "Native host installed for Chrome and Brave. Reload the extension. Manifests: {path}"
                ));
            }
            Err(error) => self.browser_error = Some(error),
        }
    }
}
impl Dashboard {
    fn channel_list(&mut self, ui: &mut egui::Ui) {
        let connected = matches!(
            &self.connection_status,
            TwitchConnectionStatus::Connected(_)
        );
        let browser_connected = self.browser_connected;
        let external_inventory_received = self.external_inventory_received;
        let has_sample_data = self
            .channels
            .iter()
            .any(|channel| channel.id.starts_with("sample-"));
        let has_no_channels = self.channels.is_empty();
        let filtered_indices =
            followed_channel_search_indices(&self.channels, &self.followed_search);
        let browser_error = self.browser_error.clone();
        let browser_notice = self.browser_notice.clone();
        let pubsub_connected = self.pubsub_connected;
        let pubsub_error = self.pubsub_error.clone();
        let streak_completed = &self.streak_completed;
        let points_balances = &self.channel_points_balances;
        let mut open_selected = false;

        {
            let channels = &mut self.channels;
            let search = &mut self.followed_search;
            let live_channel_ids = &self.live_channel_ids;
            let browser_states = &self.browser_states;
            let points_rotation = &self.points_rotation;
            let points_now = self.started_at.elapsed();
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.strong("Followed channels");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if has_sample_data {
                            ui.label(egui::RichText::new("SAMPLE DATA").small().weak());
                        } else if connected {
                            ui.label(egui::RichText::new("LIVE STATUS · 1 MIN").small().weak());
                        }
                    });
                });
                ui.add_space(8.0);
                ui.add(egui::TextEdit::singleline(search).hint_text("Fuzzy search channels"));
                ui.horizontal_wrapped(|ui| {
                    open_selected = ui.button("Open selected channels").clicked();
                    ui.label(if browser_connected && external_inventory_received {
                        "Browser connected"
                    } else if browser_connected {
                        "Scanning Twitch tabs"
                    } else {
                        "Browser not connected"
                    });
                    ui.label(if pubsub_connected {
                        "Points & Streak events connected"
                    } else if connected {
                        "Points & Streak events reconnecting"
                    } else {
                        "Points & Streak events disconnected"
                    });
                });
                ui.label("Separate player tabs are muted by default.");

                if let Some(error) = &browser_error {
                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                }
                if let Some(error) = &pubsub_error {
                    ui.colored_label(egui::Color32::LIGHT_RED, format!("Points events: {error}"));
                }
                if let Some(notice) = &browser_notice {
                    ui.label(egui::RichText::new(notice).small().weak());
                }
                if has_no_channels {
                    ui.label(if connected {
                        "No followed channels found on this Twitch account."
                    } else {
                        "Connect Twitch to load followed channels."
                    });
                } else if filtered_indices.is_empty() {
                    ui.label("No followed channels match the search.");
                }

                egui::ScrollArea::vertical()
                    .max_height(480.0)
                    .show(ui, |ui| {
                        for index in filtered_indices {
                            let channel = &mut channels[index];
                            ui.group(|ui| {
                                ui.horizontal(|ui| {
                                    ui.vertical(|ui| {
                                        ui.strong(&channel.display_name);
                                        let live_status = if live_channel_ids.contains(&channel.id)
                                        {
                                            "LIVE"
                                        } else if connected {
                                            "Offline"
                                        } else if channel.id.starts_with("sample-") {
                                            "Sample channel"
                                        } else {
                                            "Not connected"
                                        };
                                        ui.label(egui::RichText::new(live_status).small().weak());
                                        if let Some(points) = streak_completed.get(&channel.id) {
                                            ui.colored_label(
                                                egui::Color32::from_rgb(80, 210, 140),
                                                format!("WATCH_STREAK detected · {points} points"),
                                            );
                                        }
                                        if let Some(balance) = points_balances.get(&channel.id) {
                                            ui.label(format!(
                                                "Channel Points balance (latest event): {balance}"
                                            ));
                                        }
                                        if channel.selection.points {
                                            if let Some(elapsed) = points_rotation
                                                .elapsed_eligible(&channel.id, points_now)
                                            {
                                                ui.label(
                                                    egui::RichText::new(format!(
                                                        "Points turn: {}m {:02}s / {}m eligible",
                                                        elapsed.as_secs() / 60,
                                                        elapsed.as_secs() % 60,
                                                        POINTS_TURN_DURATION.as_secs() / 60,
                                                    ))
                                                    .small()
                                                    .weak(),
                                                );
                                            }
                                        }
                                        if let Some(state) = browser_states.get(&channel.id) {
                                            let player_state =
                                                match (state.player_tab_id, state.player_last_seen)
                                                {
                                                    (None, _) => "closed",
                                                    (Some(_), None) => "waiting",
                                                    (Some(_), Some(last_seen))
                                                        if last_seen.elapsed()
                                                            > Duration::from_secs(15) =>
                                                    {
                                                        "stale"
                                                    }
                                                    _ => state.player_state.as_str(),
                                                };
                                            let chat_state =
                                                match (state.chat_tab_id, state.chat_last_seen) {
                                                    (None, _) => "closed",
                                                    (Some(_), None) => "waiting",
                                                    (Some(_), Some(last_seen))
                                                        if last_seen.elapsed()
                                                            > Duration::from_secs(15) =>
                                                    {
                                                        "stale"
                                                    }
                                                    _ => state.chat_state.as_str(),
                                                };
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "Player: {player_state} · Chat: {chat_state}"
                                                ))
                                                .small()
                                                .weak(),
                                            );
                                        } else {
                                            ui.label(
                                                egui::RichText::new(if browser_connected {
                                                    "No app-managed tabs"
                                                } else {
                                                    "Browser status unavailable"
                                                })
                                                .small()
                                                .weak(),
                                            );
                                        }
                                    });
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.checkbox(&mut channel.selection.points, "Points");
                                            ui.checkbox(&mut channel.selection.streak, "Streak");
                                        },
                                    );
                                });
                            });
                            ui.add_space(4.0);
                        }
                    });
            });
        }
        if open_selected {
            self.open_selected_twitch_channels(self.tab_layout);
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

impl eframe::App for Dashboard {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, PREFERENCES_KEY, &self.preferences());
        eframe::set_value(storage, EXTENSION_ID_KEY, &self.extension_id);
    }

    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_twitch_updates();
        self.poll_browser_bridge();
        let now = self.started_at.elapsed();
        let poll_interval = Duration::from_secs(1);
        self.reconcile_automatic_tabs(now);
        context.request_repaint_after(poll_interval);
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = egui::Color32::BLACK;
        visuals.window_fill = egui::Color32::BLACK;
        visuals.faint_bg_color = egui::Color32::BLACK;
        visuals.extreme_bg_color = egui::Color32::BLACK;
        visuals.code_bg_color = egui::Color32::BLACK;
        for widget in [
            &mut visuals.widgets.noninteractive,
            &mut visuals.widgets.inactive,
            &mut visuals.widgets.hovered,
            &mut visuals.widgets.active,
            &mut visuals.widgets.open,
        ] {
            widget.bg_fill = egui::Color32::BLACK;
            widget.weak_bg_fill = egui::Color32::BLACK;
        }
        context.set_visuals(visuals);

        egui::TopBottomPanel::top("app_header").show(context, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.heading("Twitch Streak Saver");
                    ui.label("A quieter way to manage the channels you follow");
                });
                let status = match &self.connection_status {
                    TwitchConnectionStatus::Disconnected => "NOT CONNECTED".to_owned(),
                    TwitchConnectionStatus::Connecting => "CONNECTING TO TWITCH…".to_owned(),
                    TwitchConnectionStatus::Connected(login) => format!("CONNECTED · {login}"),
                };
                let connecting =
                    matches!(&self.connection_status, TwitchConnectionStatus::Connecting);
                let connected = matches!(
                    &self.connection_status,
                    TwitchConnectionStatus::Connected(_)
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(status).strong());
                    if connecting {
                        if ui.button("Cancel").clicked() {
                            if let Some(cancel) = self.twitch_cancel.take() {
                                cancel.store(true, Ordering::Release);
                            }
                            self.connection_generation = self.connection_generation.wrapping_add(1);
                            self.connection_status = TwitchConnectionStatus::Disconnected;
                        }
                    } else if connected {
                        if ui.button("Disconnect").clicked() {
                            self.disconnect_twitch();
                        }
                    } else if ui.button("Connect Twitch").clicked() {
                        self.start_twitch_connection();
                    }
                });
            });
            ui.add_space(8.0);
        });

        egui::CentralPanel::default().show(context, |ui| {
            ui.add_space(8.0);
            ui.heading("Your watch list");
            ui.label("Choose Streaks, Channel Points, or both for each followed channel.");
            ui.add_space(12.0);

            ui.columns(2, |columns| {
                self.channel_list(&mut columns[0]);
                self.watch_settings(&mut columns[1]);
            });

            ui.add_space(8.0);
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new(
                        "Selected live Streaks run first. Selected live Points streams open automatically after the Streak queue; a turn needs 16 eligible minutes with playing video, ready chat UI, and an available app slot.",
                    )
                    .weak(),
                );
            });
            if let Some(error) = &self.connection_error {
                ui.colored_label(egui::Color32::LIGHT_RED, format!("Twitch: {error}"));
            }
        });
        if let Some(notice) = self.streak_notices.front().cloned() {
            let mut is_open = true;
            let mut dismiss = false;
            egui::Window::new("Watch Streak detected")
                .id(egui::Id::new("watch_streak_event_dialog"))
                .order(egui::Order::Foreground)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .collapsible(false)
                .resizable(false)
                .open(&mut is_open)
                .show(context, |ui| {
                    ui.heading("Streak reward detected");
                    ui.label(format!("{} completed a Watch Streak.", notice.channel_name));
                    ui.label(format!("Reward: {} points", notice.points_earned));
                    ui.add_space(6.0);
                    dismiss = ui.button("Dismiss").clicked();
                });
            if dismiss || !is_open {
                self.streak_notices.pop_front();
            }
        } else if let Some(notice) = self.points_notices.front().cloned() {
            let (title, message) = match notice {
                PointsNotice::TurnCompleted { channel_name } => (
                    "Points rotation complete",
                    format!(
                        "{channel_name} completed an eligible 16-minute Points turn. The Points queue has advanced."
                    ),
                ),
                PointsNotice::Earned {
                    channel_name,
                    points,
                    reason_code,
                    balance,
                } => (
                    "Channel Points earned",
                    format!(
                        "{channel_name} earned {points} points ({reason_code}). New balance: {balance}."
                    ),
                ),
                PointsNotice::Spent {
                    channel_name,
                    balance,
                } => (
                    "Channel Points spent",
                    format!("{channel_name} points balance is now {balance}."),
                ),
                PointsNotice::ClaimAvailable { channel_name } => (
                    "Channel Points bonus available",
                    format!("A bonus is ready to claim on {channel_name}. Claim it manually on Twitch."),
                ),
            };
            let mut is_open = true;
            let mut dismiss = false;
            egui::Window::new(title)
                .id(egui::Id::new("points_event_dialog"))
                .order(egui::Order::Foreground)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .collapsible(false)
                .resizable(false)
                .open(&mut is_open)
                .show(context, |ui| {
                    ui.heading(title);
                    ui.label(message);
                    ui.add_space(6.0);
                    dismiss = ui.button("Dismiss").clicked();
                });
            if dismiss || !is_open {
                self.points_notices.pop_front();
            }
        }
    }
}

impl Dashboard {
    fn watch_settings(&mut self, ui: &mut egui::Ui) {
        ui.group(|ui| {
            ui.strong("Watch strategy");
            ui.add_space(6.0);

            ui.selectable_value(&mut self.mode, SchedulingMode::Earn, "Earn");
            ui.label("Prioritize streaks and keep to two app-managed streams.");
            ui.add_space(6.0);

            ui.selectable_value(&mut self.mode, SchedulingMode::OpenAll, "Open All");
            ui.label(
                "Keep up to four app-managed streams open. More than two may not earn progress.",
            );
            ui.label(format!(
                "App-managed stream limit: {}",
                self.mode.app_stream_limit()
            ));
        });

        let browser_connected = self.browser_connected;
        let browser_notice = self.browser_notice.clone();
        let mut install_native_host = false;
        ui.add_space(8.0);
        ui.group(|ui| {
            ui.strong("Chrome / Brave integration");
            ui.label(if browser_connected {
                "Native host connected."
            } else {
                "Load chrome-extension/ unpacked in Chrome or Brave, then install its native host."
            });
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.extension_id)
                        .hint_text("Browser extension ID"),
                );
                install_native_host = ui.button("Install host").clicked();
            });
            if let Some(notice) = &browser_notice {
                ui.label(egui::RichText::new(notice).small().weak());
            }
        });
        if install_native_host {
            self.install_chrome_native_host();
        }
        let previous_layout = self.tab_layout;
        ui.group(|ui| {
            ui.strong("Twitch tab opening");
            ui.horizontal_wrapped(|ui| {
                ui.label("Open streams as:");
                ui.selectable_value(
                    &mut self.tab_layout,
                    TwitchTabLayout::SeparatePlayerChat,
                    "Player + chat tabs",
                );
                ui.selectable_value(
                    &mut self.tab_layout,
                    TwitchTabLayout::RegularWebsite,
                    "Regular Twitch tab",
                );
            });
            ui.checkbox(
                &mut self.wake_browser_on_open,
                "Restore and focus browser when opening",
            );
            ui.label(
                "Waking restores a minimized window, focuses each stream tab, and waits for it to load. It may interrupt other work.",
            );
        });
        if previous_layout != self.tab_layout {
            self.apply_tab_layout_to_managed_tabs();
        }

        ui.add_space(8.0);
        self.automation_settings(ui);

        let selected_streaks = self
            .channels
            .iter()
            .filter(|channel| channel.selection.streak)
            .count();
        let selected_points = self
            .channels
            .iter()
            .filter(|channel| channel.selection.points)
            .count();
        let selected_channels = self
            .channels
            .iter()
            .filter(|channel| channel.selection.is_selected())
            .count();

        ui.add_space(8.0);
        ui.group(|ui| {
            ui.strong("Selection summary");
            ui.label(format!("{selected_channels} selected channels"));
            ui.label(format!("{selected_streaks} Streak channels"));
            ui.label(format!("{selected_points} Points channels"));
        });
    }
}
impl Dashboard {
    fn automation_settings(&mut self, ui: &mut egui::Ui) {
        let mut start = false;
        let mut stop = false;
        ui.group(|ui| {
            ui.strong("Automation");
            ui.label(if self.automation_enabled {
                "Automation enabled."
            } else {
                "Automation stopped."
            });
            ui.checkbox(&mut self.auto_start_on_launch, "Auto-start on app launch");
            ui.label("Twitch and browser connections are required for automatic opening.");
            ui.horizontal(|ui| {
                start = ui
                    .add_enabled(!self.automation_enabled, egui::Button::new("Start"))
                    .clicked();
                stop = ui
                    .add_enabled(self.automation_enabled, egui::Button::new("Stop All"))
                    .clicked();
            });
            ui.label(
                egui::RichText::new(
                    "Stop All closes tabs opened by this app. External browser tabs are unaffected.",
                )
                .small()
                .weak(),
            );
        });
        if start {
            self.start_automation();
        }
        if stop {
            self.stop_automation();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        active_external_stream_count, enqueue_points_notice, followed_channel_search_indices,
        points_session_for, resume_due_auto_open_retries, schedule_auto_open_retry,
        BrowserChannelState, FollowedChannel, PointsNotice, AUTO_TAB_RETRY_DELAY,
        BROWSER_STATE_STALE_AFTER,
    };
    use std::collections::{HashMap, HashSet, VecDeque};
    use std::time::{Duration, Instant};

    #[test]
    fn followed_search_matches_case_insensitive_fuzzy_subsequences() {
        let channels = vec![
            FollowedChannel::new("12345", "Twitch"),
            FollowedChannel::new("67890", "Other channel"),
        ];

        assert_eq!(followed_channel_search_indices(&channels, "TWTCH"), vec![0]);
        assert_eq!(followed_channel_search_indices(&channels, "135"), vec![0]);
        assert!(followed_channel_search_indices(&channels, "missing").is_empty());
    }

    #[test]
    fn empty_followed_search_keeps_original_order() {
        let channels = vec![
            FollowedChannel::new("1", "First"),
            FollowedChannel::new("2", "Second"),
        ];

        assert_eq!(followed_channel_search_indices(&channels, "  "), vec![0, 1]);
    }

    #[test]
    fn selected_channels_prioritize_streaks_and_respect_limit_without_duplicates() {
        let mut points_only = FollowedChannel::new("points", "Points");
        points_only.login = "points_login".to_owned();
        points_only.selection.points = true;

        let mut both = FollowedChannel::new("both", "Both");
        both.login = "both_login".to_owned();
        both.selection.streak = true;
        both.selection.points = true;

        let mut streak_only = FollowedChannel::new("streak", "Streak");
        streak_only.login = "streak_login".to_owned();
        streak_only.selection.streak = true;

        let mut missing_login = FollowedChannel::new("missing", "Missing login");
        missing_login.selection.streak = true;

        let channels = vec![points_only, both, streak_only, missing_login];

        assert_eq!(super::selected_channel_indices(&channels, 2), vec![1, 2]);
        assert_eq!(super::selected_channel_indices(&channels, 4), vec![1, 2, 0]);
    }
    #[test]
    fn points_alert_queue_keeps_latest_eight() {
        let mut notices = VecDeque::<PointsNotice>::new();
        for index in 0..9 {
            enqueue_points_notice(
                &mut notices,
                PointsNotice::TurnCompleted {
                    channel_name: format!("channel-{index}"),
                },
            );
        }

        assert_eq!(notices.len(), 8);
        assert!(matches!(
            notices.front(),
            Some(PointsNotice::TurnCompleted { channel_name }) if channel_name == "channel-1"
        ));
        assert!(matches!(
            notices.back(),
            Some(PointsNotice::TurnCompleted { channel_name }) if channel_name == "channel-8"
        ));
    }

    #[test]
    fn points_eligibility_uses_fresh_playing_video_ready_chat_and_capacity() {
        let now = Instant::now();
        let mut state = BrowserChannelState {
            player_tab_id: Some(1),
            chat_tab_id: Some(2),
            player_state: "playing".to_owned(),
            chat_state: "ready".to_owned(),
            player_last_seen: Some(now),
            chat_last_seen: Some(now),
        };

        assert!(points_session_for("a", Some(&state), now, true)
            .eligibility
            .is_eligible());

        state.player_state = "buffering".to_owned();
        assert!(!points_session_for("a", Some(&state), now, true)
            .eligibility
            .is_eligible());
        state.player_state = "playing".to_owned();
        state.chat_state = "loading".to_owned();
        assert!(!points_session_for("a", Some(&state), now, true)
            .eligibility
            .is_eligible());
        state.chat_state = "ready".to_owned();
        state.player_last_seen = Some(
            now.checked_sub(BROWSER_STATE_STALE_AFTER + Duration::from_secs(1))
                .unwrap(),
        );
        assert!(!points_session_for("a", Some(&state), now, true)
            .eligibility
            .is_eligible());
        assert!(!points_session_for("a", Some(&state), now, false)
            .eligibility
            .is_eligible());
    }
    #[test]
    fn external_capacity_counts_live_followed_and_unmatched_tabs() {
        let channel_counts = HashMap::from([
            ("live-channel".to_owned(), 2),
            ("offline-channel".to_owned(), 4),
        ]);
        let live_channel_ids = HashSet::from(["live-channel".to_owned()]);

        assert_eq!(
            active_external_stream_count(&channel_counts, 3, &live_channel_ids),
            5
        );
    }
    #[test]
    fn automatic_tab_failures_retry_after_a_bounded_delay() {
        let active = HashSet::from(["automatic".to_owned()]);
        let mut retries = HashMap::new();
        let now = Instant::now();

        assert!(schedule_auto_open_retry(
            &active,
            &mut retries,
            "automatic",
            now,
        ));
        assert_eq!(
            retries.get("automatic"),
            Some(&(now + AUTO_TAB_RETRY_DELAY)),
        );
        assert!(!schedule_auto_open_retry(
            &active,
            &mut retries,
            "manual",
            now,
        ));
        assert!(!retries.contains_key("manual"));
    }

    #[test]
    fn only_due_automatic_tab_failures_are_retried() {
        let now = Instant::now();
        let mut active = HashSet::from(["due".to_owned(), "later".to_owned()]);
        let mut retries = HashMap::from([
            ("due".to_owned(), now - Duration::from_secs(1)),
            ("later".to_owned(), now + Duration::from_secs(10)),
        ]);

        resume_due_auto_open_retries(&mut active, &mut retries, now);

        assert_eq!(active, HashSet::from(["later".to_owned()]));
        assert_eq!(
            retries,
            HashMap::from([("later".to_owned(), now + Duration::from_secs(10))]),
        );
    }
}
