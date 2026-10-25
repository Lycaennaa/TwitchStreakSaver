use super::{
    BrowserChannelState, BrowserKind, BrowserLaunchTarget, BrowserSurface, BrowserTabCloseReason,
    Dashboard, FollowedChannel, KnownBrowserMessage, PreferredQuality, QueuePlanner,
    TwitchConnectionStatus, TwitchUpdate,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

fn dashboard_with_pending_stream_switch(source_state_available: bool) -> Dashboard {
    let mut new_streamer = FollowedChannel::new("new", "New Streamer");
    new_streamer.selection.streak = true;
    let (twitch_sender, twitch_updates) = std::sync::mpsc::channel();
    let (browser_port_check_sender, browser_port_check_receiver) = std::sync::mpsc::channel();
    let mut dashboard = Dashboard {
        #[cfg(target_os = "windows")]
        start_with_windows: false,
        #[cfg(target_os = "windows")]
        windows_startup_error: None,
        auto_start_on_launch: false,
        wake_browser_on_open: false,
        close_on_raid_redirect: false,
        adopt_existing_twitch_tabs: false,
        preferred_quality: PreferredQuality::default(),
        browser_launch_target: None,
        browser_launch_attempted: None,
        browser_profiles: Vec::new(),
        browser_profile_roots: Vec::new(),
        browser_profile_data_directories: Vec::new(),
        automation_enabled: true,
        automation_delays: super::automation::AutomationDelays::default(),
        channels: vec![
            FollowedChannel::new("old", "Old Streamer"),
            new_streamer.clone(),
        ],
        followed_search: String::new(),
        started_at: Instant::now(),
        connection_status: TwitchConnectionStatus::Connected("test".to_owned()),
        connection_error: None,
        live_channel_ids: HashSet::from(["new".to_owned()]),
        connection_generation: 0,
        twitch_cancel: None,
        twitch_sender: twitch_sender.clone(),
        twitch_updates,
        browser_bridge: None,
        browser_bridge_retry_after: None,
        browser_connected: true,
        browser_error: None,
        browser_notice: None,
        browser_port_check_sender,
        browser_port_check_receiver,
        browser_port_check_in_flight: false,
        browser_states: if source_state_available {
            HashMap::from([(
                "old".to_owned(),
                BrowserChannelState {
                    player_tab_id: Some(1),
                    ..BrowserChannelState::default()
                },
            )])
        } else {
            HashMap::new()
        },
        external_stream_counts: HashMap::new(),
        external_unmatched_streams: 0,
        external_inventory_received: true,
        pubsub_connected: false,
        pubsub_error: None,
        streak_completed: HashMap::new(),
        notified_streak_channels: HashSet::new(),
        activity_log: VecDeque::new(),
        activity_log_kind_filter: None,
        activity_log_search: String::new(),
        channel_points_balances: HashMap::new(),
        available_claim_channels: HashSet::new(),
        queue_planner: QueuePlanner::default(),
        points_rotation: super::StreamRotation::default(),
        streak_rotation: super::StreamRotation::default(),
        manual_open_channels: HashSet::new(),
        auto_open_channels: HashSet::from(["old".to_owned()]),
        raid_redirected_channels: HashSet::new(),
        removed_auto_streamer_names: HashMap::new(),
        pending_stream_switch: None,
        twitch_network_unavailable: false,
        twitch_network_errors: HashMap::new(),
        reload_managed_player_tabs_pending: false,
        pending_manual_open: false,
        pending_manual_open_inventory_wait_since: None,
        next_browser_command_id: 1,
        browser_command_session_id: "test-session".to_owned(),
        pending_reload_command: None,
        pending_manual_open_command: None,
        browser_command_ack_supported: true,
        twitch_baseline_confirmed_at: None,
        pubsub_state_confirmed_at: None,
        pubsub_last_event_at: None,
        pubsub_missed_events_since: None,
        auto_open_retry_after: HashMap::new(),
    };
    dashboard.browser_connected = false;
    assert!(twitch_sender
        .send(TwitchUpdate::Refreshed {
            generation: 0,
            channels: vec![new_streamer],
            live_channel_ids: HashSet::from(["new".to_owned()]),
        })
        .is_ok());
    dashboard.poll_twitch_updates();
    dashboard.browser_connected = true;
    dashboard.external_inventory_received = true;

    dashboard.reconcile_automatic_tabs_with(Duration::ZERO, |_, _| true);

    dashboard
}

#[test]
fn queue_visibility_keeps_candidates_visible_when_stream_capacity_is_full() {
    let mut dashboard = dashboard_with_pending_stream_switch(true);
    dashboard.external_unmatched_streams = super::APP_STREAM_LIMIT;

    let queue = dashboard.queue_visibility();

    assert_eq!(queue.open_capacity, 0);
    assert_eq!(queue.entries[0].channel_id, "new");
    assert!(queue
        .blockers
        .iter()
        .any(|blocker| blocker.contains("capacity is full")));
}

#[test]
fn offline_selected_channels_remain_excluded_from_queue_entries() {
    let mut dashboard = dashboard_with_pending_stream_switch(true);
    dashboard.live_channel_ids.clear();

    let queue = dashboard.queue_visibility();

    assert!(queue.entries.is_empty());
}

#[test]
fn queue_visibility_separates_turn_readiness_from_open_blockers() {
    let mut dashboard = dashboard_with_pending_stream_switch(true);
    dashboard.external_inventory_received = false;
    dashboard.connection_status = TwitchConnectionStatus::Disconnected;
    dashboard.automation_enabled = false;
    let now = std::time::Instant::now();
    dashboard.browser_states.insert(
        "new".to_owned(),
        BrowserChannelState {
            player_tab_id: Some(1),
            chat_tab_id: Some(2),
            player_state: Some(super::PlayerState::Playing),
            chat_state: Some(super::ChatState::Ready),
            player_last_seen: Some(now),
            chat_last_seen: Some(now),
        },
    );

    let queue = dashboard.queue_visibility();

    let entry = queue
        .entries
        .iter()
        .find(|entry| entry.channel_id == "new")
        .unwrap();
    assert!(entry.player_ready);
    assert!(entry.chat_ready);
    assert!(queue
        .blockers
        .iter()
        .any(|blocker| blocker.contains("Start automation")));
    assert!(queue
        .blockers
        .iter()
        .any(|blocker| blocker.contains("Connect Twitch")));
    assert!(queue
        .blockers
        .iter()
        .any(|blocker| blocker.contains("tab inventory")));
    assert!(!queue.blockers.iter().any(|blocker| {
        blocker.to_ascii_lowercase().contains("player")
            || blocker.to_ascii_lowercase().contains("chat")
    }));
}

#[test]
fn queue_preview_expires_deferred_streaks_while_browser_is_unavailable() {
    let mut dashboard = dashboard_with_pending_stream_switch(true);
    let mut points = FollowedChannel::new("points", "Points Channel");
    points.selection.points = true;
    dashboard.channels.push(points);
    dashboard.live_channel_ids.insert("points".to_owned());
    dashboard
        .queue_planner
        .record_streak_turn_timed_out("new", Duration::ZERO);
    dashboard.browser_connected = false;
    dashboard.external_inventory_received = false;

    assert_eq!(
        dashboard.queue_visibility().stage,
        super::QueueStage::Points
    );

    dashboard.reconcile_automatic_tabs_with(super::STREAM_TURN_DURATION, |_, _| true);

    let queue = dashboard.queue_visibility();
    assert_eq!(queue.stage, super::QueueStage::Streak);
    assert_eq!(queue.entries[0].channel_id, "new");
}

#[test]
fn queue_visibility_blocks_opening_without_command_acknowledgements() {
    let mut dashboard = dashboard_with_pending_stream_switch(true);
    dashboard.browser_command_ack_supported = false;

    let queue = dashboard.queue_visibility();

    assert!(!queue.entries.is_empty());
    assert!(queue
        .blockers
        .iter()
        .any(|blocker| blocker.contains("0.3.18") && blocker.contains("acknowledgements")));
}

#[test]
fn streak_deferral_survives_twitch_disconnect_until_expiry() {
    let mut dashboard = dashboard_with_pending_stream_switch(true);
    let mut points = FollowedChannel::new("points", "Points Channel");
    points.selection.points = true;
    dashboard.channels.push(points);
    dashboard.live_channel_ids.insert("points".to_owned());
    dashboard
        .queue_planner
        .record_streak_turn_timed_out("new", Duration::ZERO);
    dashboard.connection_status = TwitchConnectionStatus::Disconnected;
    dashboard.live_channel_ids.clear();

    dashboard.reconcile_automatic_tabs_with(Duration::from_secs(10), |_, _| true);

    dashboard.connection_status = TwitchConnectionStatus::Connected("test".to_owned());
    dashboard.live_channel_ids.insert("new".to_owned());
    dashboard.live_channel_ids.insert("points".to_owned());
    dashboard.reconcile_automatic_tabs_with(Duration::from_secs(10), |_, _| true);
    assert_eq!(
        dashboard.queue_visibility().stage,
        super::QueueStage::Points
    );

    dashboard.reconcile_automatic_tabs_with(super::STREAM_TURN_DURATION, |_, _| true);
    assert_eq!(
        dashboard.queue_visibility().stage,
        super::QueueStage::Streak
    );
}

#[test]
fn automatic_stream_switch_logs_only_after_both_player_tabs_confirm() {
    let mut dashboard = dashboard_with_pending_stream_switch(true);
    assert!(dashboard.activity_log.is_empty());

    dashboard.handle_browser_message(KnownBrowserMessage::TabOpened {
        channel_id: "new".to_owned(),
        tab_id: 2,
        surface: BrowserSurface::Player,
    });
    assert_eq!(dashboard.activity_log.len(), 1);
    assert_eq!(
        dashboard.activity_log.front().unwrap().event,
        "Opening New Streamer."
    );
    dashboard.handle_browser_message(KnownBrowserMessage::TabClosed {
        channel_id: "old".to_owned(),
        tab_id: 99,
        surface: BrowserSurface::Player,
        close_reason: None,
    });
    assert_eq!(dashboard.activity_log.len(), 1);
    assert_eq!(
        dashboard.activity_log.front().unwrap().event,
        "Opening New Streamer."
    );
    dashboard.handle_browser_message(KnownBrowserMessage::TabClosed {
        channel_id: "old".to_owned(),
        tab_id: 1,
        surface: BrowserSurface::Player,
        close_reason: None,
    });
    let entry = dashboard.activity_log.front().unwrap();
    assert_eq!(entry.streamer, "Old Streamer");
    assert_eq!(entry.event, "Switched from Old Streamer to New Streamer.");

    let mut dashboard = dashboard_with_pending_stream_switch(true);
    dashboard.handle_browser_message(KnownBrowserMessage::TabClosed {
        channel_id: "old".to_owned(),
        tab_id: 1,
        surface: BrowserSurface::Player,
        close_reason: None,
    });
    assert!(dashboard.activity_log.is_empty());
    dashboard.handle_browser_message(KnownBrowserMessage::TabOpened {
        channel_id: "new".to_owned(),
        tab_id: 2,
        surface: BrowserSurface::Player,
    });
    assert_eq!(
        dashboard.activity_log.front().unwrap().event,
        "Switched from Old Streamer to New Streamer."
    );
}
#[test]
fn raid_redirect_retires_a_live_auto_channel_until_extension_confirms_offline() {
    let mut dashboard = dashboard_with_pending_stream_switch(true);
    dashboard.channels[0].selection.streak = true;
    dashboard.live_channel_ids = HashSet::from(["new".to_owned()]);
    dashboard.browser_states = HashMap::from([(
        "new".to_owned(),
        BrowserChannelState {
            player_tab_id: Some(2),
            ..BrowserChannelState::default()
        },
    )]);
    dashboard.auto_open_channels = HashSet::from(["new".to_owned()]);
    dashboard.pending_stream_switch = None;
    dashboard.queue_planner = QueuePlanner::default();
    dashboard.points_rotation = super::StreamRotation::default();
    dashboard.streak_rotation = super::StreamRotation::default();
    dashboard.streak_completed.clear();
    dashboard.auto_open_retry_after.clear();
    dashboard.browser_error = None;

    dashboard.handle_browser_message(KnownBrowserMessage::TabClosed {
        channel_id: "new".to_owned(),
        tab_id: 2,
        surface: BrowserSurface::Player,
        close_reason: Some(BrowserTabCloseReason::RaidRedirect),
    });
    dashboard.reconcile_automatic_tabs(Duration::ZERO);

    assert!(dashboard.live_channel_ids.contains("new"));
    assert!(dashboard.raid_redirected_channels.contains("new"));
    assert!(!dashboard.auto_open_channels.contains("new"));
    assert!(dashboard.browser_error.is_none());

    dashboard.connection_status = TwitchConnectionStatus::Disconnected;
    dashboard.live_channel_ids.clear();
    dashboard.reconcile_automatic_tabs(Duration::ZERO);
    assert!(dashboard.raid_redirected_channels.contains("new"));

    dashboard.connection_status = TwitchConnectionStatus::Connected("test".to_owned());
    dashboard.clear_offline_raid_redirects();
    assert!(dashboard.raid_redirected_channels.contains("new"));
    dashboard.handle_browser_message(KnownBrowserMessage::ExternalStreams {
        channel_counts: HashMap::new(),
        unmatched_count: 0,
        managed_channel_ids: Some(Vec::new()),
        raid_redirected_channel_ids: Some(Vec::new()),
    });
    assert!(!dashboard.raid_redirected_channels.contains("new"));

    dashboard.live_channel_ids.insert("new".to_owned());
    dashboard.reconcile_automatic_tabs_with(Duration::ZERO, |_, _| true);
    assert!(dashboard.auto_open_channels.contains("new"));
}

#[test]
fn external_inventory_restores_raid_suppression_before_reconciliation() {
    let mut dashboard = dashboard_with_pending_stream_switch(true);
    dashboard.pending_stream_switch = None;
    dashboard.auto_open_channels.clear();
    dashboard.raid_redirected_channels.clear();
    dashboard.browser_error = None;
    dashboard.handle_browser_message(KnownBrowserMessage::ExternalStreams {
        channel_counts: HashMap::new(),
        unmatched_count: 0,
        managed_channel_ids: Some(Vec::new()),
        raid_redirected_channel_ids: Some(vec!["new".to_owned()]),
    });

    dashboard.reconcile_automatic_tabs(Duration::ZERO);

    assert!(dashboard.raid_redirected_channels.contains("new"));
    assert!(!dashboard.auto_open_channels.contains("new"));
}

#[test]
fn unfollowed_source_close_ack_without_cached_state_completes_switch() {
    let mut dashboard = dashboard_with_pending_stream_switch(false);
    dashboard.handle_browser_message(KnownBrowserMessage::TabClosed {
        channel_id: "old".to_owned(),
        tab_id: 7,
        surface: BrowserSurface::Player,
        close_reason: None,
    });
    assert!(dashboard.activity_log.is_empty());
    dashboard.handle_browser_message(KnownBrowserMessage::TabOpened {
        channel_id: "new".to_owned(),
        tab_id: 2,
        surface: BrowserSurface::Player,
    });
    assert_eq!(
        dashboard.activity_log.front().unwrap().event,
        "Switched from Old Streamer to New Streamer."
    );
}

#[test]
fn browser_errors_wait_for_confirmations() {
    for failed_channel_id in [Some("old"), Some("new"), None] {
        let mut dashboard = dashboard_with_pending_stream_switch(true);
        dashboard.handle_browser_message(KnownBrowserMessage::Error {
            channel_id: failed_channel_id.map(str::to_owned),
            message: Some("tab operation failed".to_owned()),
            close_reason: None,
        });
        assert!(dashboard.activity_log.is_empty());
        assert!(dashboard.pending_stream_switch.is_some());
    }
}

#[test]
fn destination_open_retry_preserves_switch_context() {
    let mut dashboard = dashboard_with_pending_stream_switch(true);
    dashboard.handle_browser_message(KnownBrowserMessage::Error {
        channel_id: Some("new".to_owned()),
        message: Some("tab open failed".to_owned()),
        close_reason: None,
    });
    assert!(dashboard.pending_stream_switch.is_some());

    dashboard.handle_browser_message(KnownBrowserMessage::TabClosed {
        channel_id: "old".to_owned(),
        tab_id: 1,
        surface: BrowserSurface::Player,
        close_reason: None,
    });
    assert!(dashboard.activity_log.is_empty());

    dashboard
        .auto_open_retry_after
        .insert("new".to_owned(), Instant::now() - Duration::from_secs(1));
    dashboard.reconcile_automatic_tabs_with(Duration::ZERO, |_, _| true);

    assert!(dashboard.auto_open_channels.contains("new"));
    assert!(dashboard.pending_stream_switch.is_some());
    assert!(dashboard.activity_log.is_empty());
    dashboard.handle_browser_message(KnownBrowserMessage::TabOpened {
        channel_id: "new".to_owned(),
        tab_id: 2,
        surface: BrowserSurface::Player,
    });
    assert_eq!(
        dashboard.activity_log.front().unwrap().event,
        "Switched from Old Streamer to New Streamer."
    );
}

#[test]
fn source_close_retry_preserves_switch_context() {
    let mut dashboard = dashboard_with_pending_stream_switch(true);
    dashboard.handle_browser_message(KnownBrowserMessage::TabOpened {
        channel_id: "new".to_owned(),
        tab_id: 2,
        surface: BrowserSurface::Player,
    });
    assert_eq!(dashboard.activity_log.len(), 1);
    assert_eq!(
        dashboard.activity_log.front().unwrap().event,
        "Opening New Streamer."
    );

    dashboard.handle_browser_message(KnownBrowserMessage::Error {
        channel_id: Some("old".to_owned()),
        message: Some("tab close failed".to_owned()),
        close_reason: None,
    });
    assert!(dashboard.pending_stream_switch.is_some());
    assert!(dashboard.auto_open_channels.contains("old"));
    assert!(dashboard.auto_open_retry_after.contains_key("old"));

    dashboard
        .auto_open_retry_after
        .insert("old".to_owned(), Instant::now() - Duration::from_secs(1));
    dashboard.reconcile_automatic_tabs(Duration::ZERO);

    assert!(dashboard.pending_stream_switch.is_some());
    assert!(dashboard.auto_open_channels.contains("old"));
    assert_eq!(dashboard.activity_log.len(), 1);
    assert_eq!(
        dashboard.activity_log.front().unwrap().event,
        "Opening New Streamer."
    );
    dashboard.handle_browser_message(KnownBrowserMessage::TabClosed {
        channel_id: "old".to_owned(),
        tab_id: 1,
        surface: BrowserSurface::Player,
        close_reason: None,
    });
    assert_eq!(
        dashboard.activity_log.front().unwrap().event,
        "Switched from Old Streamer to New Streamer."
    );
}

#[test]
fn unrelated_browser_error_does_not_cancel_pending_switch() {
    let mut dashboard = dashboard_with_pending_stream_switch(true);
    dashboard.handle_browser_message(KnownBrowserMessage::Error {
        channel_id: Some("unrelated".to_owned()),
        message: Some("unrelated tab operation failed".to_owned()),
        close_reason: None,
    });
    dashboard.handle_browser_message(KnownBrowserMessage::TabClosed {
        channel_id: "old".to_owned(),
        tab_id: 1,
        surface: BrowserSurface::Player,
        close_reason: None,
    });
    dashboard.handle_browser_message(KnownBrowserMessage::TabOpened {
        channel_id: "new".to_owned(),
        tab_id: 2,
        surface: BrowserSurface::Player,
    });
    assert_eq!(
        dashboard.activity_log.front().unwrap().event,
        "Switched from Old Streamer to New Streamer."
    );
}

#[test]
fn removed_retry_destination_keeps_source_close_retry_owned() {
    let mut dashboard = dashboard_with_pending_stream_switch(true);
    for channel_id in ["old", "new"] {
        dashboard.handle_browser_message(KnownBrowserMessage::Error {
            channel_id: Some(channel_id.to_owned()),
            message: Some("tab operation failed".to_owned()),
            close_reason: None,
        });
    }

    dashboard.browser_connected = false;
    assert!(dashboard
        .twitch_sender
        .send(TwitchUpdate::Refreshed {
            generation: 0,
            channels: Vec::new(),
            live_channel_ids: HashSet::new(),
        })
        .is_ok());
    dashboard.poll_twitch_updates();
    dashboard.browser_connected = true;
    dashboard.external_inventory_received = true;
    let retry_at = Instant::now() - Duration::from_secs(1);
    dashboard
        .auto_open_retry_after
        .insert("old".to_owned(), retry_at);
    dashboard
        .auto_open_retry_after
        .insert("new".to_owned(), retry_at);

    dashboard.reconcile_automatic_tabs(Duration::ZERO);

    assert!(dashboard.pending_stream_switch.is_some());
    assert!(dashboard.auto_open_channels.contains("old"));
    assert!(!dashboard.auto_open_channels.contains("new"));
    assert!(dashboard.activity_log.is_empty());
    dashboard.handle_browser_message(KnownBrowserMessage::TabClosed {
        channel_id: "old".to_owned(),
        tab_id: 1,
        surface: BrowserSurface::Player,
        close_reason: None,
    });
    assert!(dashboard.pending_stream_switch.is_none());
    assert!(dashboard.auto_open_channels.is_empty());
    assert!(dashboard.activity_log.is_empty());
}

#[test]
fn failed_raid_close_cancels_a_pending_auto_open_retry() {
    let mut dashboard = dashboard_with_pending_stream_switch(true);
    dashboard.handle_browser_message(KnownBrowserMessage::Error {
        channel_id: Some("new".to_owned()),
        message: Some("tab open failed".to_owned()),
        close_reason: None,
    });
    assert!(dashboard.auto_open_retry_after.contains_key("new"));
    assert!(dashboard
        .pending_stream_switch
        .as_ref()
        .is_some_and(|pending| pending.is_awaiting_open("new")));

    dashboard.handle_browser_message(KnownBrowserMessage::Error {
        channel_id: Some("new".to_owned()),
        message: Some("Could not close managed Twitch tab".to_owned()),
        close_reason: Some(BrowserTabCloseReason::RaidRedirect),
    });

    assert!(dashboard.raid_redirected_channels.contains("new"));
    assert!(!dashboard.auto_open_retry_after.contains_key("new"));
    assert!(!dashboard
        .pending_stream_switch
        .as_ref()
        .is_some_and(|pending| pending.is_awaiting_open("new")));
}

#[test]
fn failed_raid_close_preserves_pending_source_close_retry() {
    let mut dashboard = dashboard_with_pending_stream_switch(true);
    dashboard.handle_browser_message(KnownBrowserMessage::Error {
        channel_id: Some("old".to_owned()),
        message: Some("tab close failed".to_owned()),
        close_reason: None,
    });
    assert!(dashboard.auto_open_retry_after.contains_key("old"));
    assert!(dashboard
        .pending_stream_switch
        .as_ref()
        .is_some_and(|pending| pending.is_awaiting_close("old")));

    dashboard.handle_browser_message(KnownBrowserMessage::Error {
        channel_id: Some("old".to_owned()),
        message: Some("Could not close managed Twitch tab".to_owned()),
        close_reason: Some(BrowserTabCloseReason::RaidRedirect),
    });
    assert!(dashboard.raid_redirected_channels.contains("old"));
    assert!(dashboard.auto_open_retry_after.contains_key("old"));

    dashboard
        .auto_open_retry_after
        .insert("old".to_owned(), Instant::now() - Duration::from_secs(1));
    dashboard.reconcile_automatic_tabs_with(Duration::ZERO, |_, _| true);

    assert!(!dashboard.auto_open_retry_after.contains_key("old"));
    assert!(dashboard
        .pending_stream_switch
        .as_ref()
        .is_some_and(|pending| pending.is_awaiting_close("old")));
}

#[test]
fn raid_suppression_blocks_a_stale_pending_open_retry() {
    let mut dashboard = dashboard_with_pending_stream_switch(true);
    dashboard.raid_redirected_channels.insert("new".to_owned());
    dashboard
        .auto_open_retry_after
        .insert("new".to_owned(), Instant::now() - Duration::from_secs(1));

    dashboard.reconcile_automatic_tabs(Duration::ZERO);

    assert!(!dashboard.auto_open_channels.contains("new"));
    assert!(!dashboard.auto_open_retry_after.contains_key("new"));
    assert!(!dashboard
        .pending_stream_switch
        .as_ref()
        .is_some_and(|pending| pending.is_awaiting_open("new")));
}

#[test]
fn latest_stream_plan_waits_for_in_flight_switch_confirmation() {
    let mut dashboard = dashboard_with_pending_stream_switch(true);
    dashboard
        .channels
        .iter_mut()
        .find(|channel| channel.id == "new")
        .unwrap()
        .selection
        .streak = false;
    let mut latest = FollowedChannel::new("third", "Third Streamer");
    latest.selection.streak = true;
    dashboard.channels.push(latest);
    dashboard.live_channel_ids.insert("third".to_owned());

    dashboard.reconcile_automatic_tabs_with(Duration::ZERO, |_, _| true);

    assert_eq!(
        dashboard.auto_open_channels,
        HashSet::from(["old".to_owned(), "new".to_owned()])
    );
    let pending = dashboard.pending_stream_switch.as_ref().unwrap();
    assert!(pending.channels.contains_key("old"));
    assert!(pending.channels.contains_key("new"));
    assert!(!pending.channels.contains_key("third"));

    dashboard.handle_browser_message(KnownBrowserMessage::TabClosed {
        channel_id: "old".to_owned(),
        tab_id: 1,
        surface: BrowserSurface::Player,
        close_reason: None,
    });
    dashboard.handle_browser_message(KnownBrowserMessage::TabOpened {
        channel_id: "new".to_owned(),
        tab_id: 2,
        surface: BrowserSurface::Player,
    });
    assert_eq!(
        dashboard.activity_log.front().unwrap().event,
        "Switched from Old Streamer to New Streamer."
    );

    dashboard.reconcile_automatic_tabs_with(Duration::ZERO, |_, _| true);

    assert_eq!(
        dashboard.auto_open_channels,
        HashSet::from(["new".to_owned(), "third".to_owned()])
    );
    let pending = dashboard.pending_stream_switch.as_ref().unwrap();
    assert!(pending.channels.contains_key("new"));
    assert!(pending.channels.contains_key("third"));
}

#[test]
fn unrelated_due_retry_waits_for_pending_switch() {
    let mut dashboard = dashboard_with_pending_stream_switch(true);
    let mut other = FollowedChannel::new("other", "Other Streamer");
    other.selection.streak = true;
    dashboard.channels.push(other);
    dashboard.live_channel_ids.insert("other".to_owned());
    dashboard.auto_open_channels.insert("other".to_owned());
    dashboard
        .pending_stream_switch
        .as_mut()
        .unwrap()
        .desired_channels
        .insert("other".to_owned());
    dashboard
        .auto_open_retry_after
        .insert("other".to_owned(), Instant::now() - Duration::from_secs(1));

    dashboard.reconcile_automatic_tabs(Duration::ZERO);

    assert!(dashboard.auto_open_channels.contains("other"));
    assert!(dashboard.auto_open_retry_after.contains_key("other"));

    dashboard.handle_browser_message(KnownBrowserMessage::TabClosed {
        channel_id: "old".to_owned(),
        tab_id: 1,
        surface: BrowserSurface::Player,
        close_reason: None,
    });
    dashboard.handle_browser_message(KnownBrowserMessage::TabOpened {
        channel_id: "new".to_owned(),
        tab_id: 2,
        surface: BrowserSurface::Player,
    });
    assert_eq!(
        dashboard.activity_log.front().unwrap().event,
        "Switched from Old Streamer to New Streamer."
    );

    dashboard.reconcile_automatic_tabs_with(Duration::ZERO, |_, _| true);

    assert!(dashboard.auto_open_channels.contains("other"));
    assert!(!dashboard.auto_open_retry_after.contains_key("other"));
    assert!(dashboard.pending_stream_switch.is_none());
}

#[test]
fn unfollowed_destination_open_ack_completes_pending_switch() {
    let mut dashboard = dashboard_with_pending_stream_switch(true);
    dashboard.browser_connected = false;
    assert!(dashboard
        .twitch_sender
        .send(TwitchUpdate::Refreshed {
            generation: 0,
            channels: Vec::new(),
            live_channel_ids: HashSet::new(),
        })
        .is_ok());
    dashboard.poll_twitch_updates();
    dashboard.browser_connected = true;
    dashboard.external_inventory_received = true;
    dashboard.reconcile_automatic_tabs(Duration::ZERO);
    assert!(dashboard.pending_stream_switch.is_some());

    dashboard.handle_browser_message(KnownBrowserMessage::TabOpened {
        channel_id: "new".to_owned(),
        tab_id: 2,
        surface: BrowserSurface::Player,
    });
    assert!(dashboard.activity_log.is_empty());
    dashboard.handle_browser_message(KnownBrowserMessage::TabClosed {
        channel_id: "old".to_owned(),
        tab_id: 1,
        surface: BrowserSurface::Player,
        close_reason: None,
    });
    assert_eq!(
        dashboard.activity_log.front().unwrap().event,
        "Switched from Old Streamer to New Streamer."
    );

    dashboard.reconcile_automatic_tabs(Duration::ZERO);
    assert!(dashboard.auto_open_channels.is_empty());
}

#[test]
fn automatic_browser_launch_requires_a_live_selected_channel_and_profile() {
    let mut dashboard = dashboard_with_pending_stream_switch(false);
    dashboard.browser_connected = false;
    dashboard.browser_launch_target = Some(BrowserLaunchTarget {
        browser: BrowserKind::Chrome,
        profile_directory: "Default".to_owned(),
    });

    assert!(dashboard.should_launch_browser_for_automatic_channels());

    dashboard.live_channel_ids.clear();
    assert!(!dashboard.should_launch_browser_for_automatic_channels());
    dashboard.live_channel_ids.insert("new".to_owned());

    dashboard.browser_connected = true;
    assert!(!dashboard.should_launch_browser_for_automatic_channels());
    dashboard.browser_connected = false;

    dashboard.automation_enabled = false;
    assert!(!dashboard.should_launch_browser_for_automatic_channels());
    dashboard.automation_enabled = true;

    dashboard.browser_launch_target = None;
    assert!(!dashboard.should_launch_browser_for_automatic_channels());
}
#[test]
fn activity_log_records_app_managed_player_open_once() {
    let mut dashboard = dashboard_with_pending_stream_switch(false);
    dashboard.pending_stream_switch = None;
    dashboard.auto_open_channels.clear();
    dashboard.manual_open_channels = HashSet::from(["new".to_owned()]);
    dashboard.browser_states.clear();

    dashboard.handle_browser_message(KnownBrowserMessage::TabOpened {
        channel_id: "new".to_owned(),
        tab_id: 1,
        surface: BrowserSurface::Chat,
    });
    assert!(dashboard.activity_log.is_empty());

    dashboard.handle_browser_message(KnownBrowserMessage::TabOpened {
        channel_id: "new".to_owned(),
        tab_id: 2,
        surface: BrowserSurface::Player,
    });
    let entry = dashboard.activity_log.front().unwrap();
    assert_eq!(entry.streamer, "New Streamer");
    assert_eq!(entry.event, "Opening New Streamer.");

    dashboard.handle_browser_message(KnownBrowserMessage::TabOpened {
        channel_id: "new".to_owned(),
        tab_id: 3,
        surface: BrowserSurface::Player,
    });
    assert_eq!(dashboard.activity_log.len(), 1);

    dashboard.manual_open_channels.clear();
    dashboard.browser_states.clear();
    dashboard.handle_browser_message(KnownBrowserMessage::TabOpened {
        channel_id: "old".to_owned(),
        tab_id: 4,
        surface: BrowserSurface::Player,
    });
    assert_eq!(dashboard.activity_log.len(), 1);
}

#[test]
fn temporary_global_pause_preserves_tabs_and_resumes_at_expiry() {
    let mut dashboard = dashboard_with_pending_stream_switch(true);
    dashboard.pending_stream_switch = None;
    let now = dashboard.started_at.elapsed();
    let existing_auto_open_channels = dashboard.auto_open_channels.clone();
    let auto_start_on_launch = dashboard.auto_start_on_launch;

    dashboard
        .automation_delays
        .pause_for(now, Duration::from_secs(60));
    dashboard
        .streak_rotation
        .observe_active_sessions(
            &[super::StreamSession {
                channel_id: "new".to_owned(),
                eligibility: super::TurnEligibility {
                    player_playing: true,
                    chat_ready: true,
                    capacity_available: true,
                },
            }],
            now,
        )
        .unwrap();
    dashboard.streak_rotation.pause_active_sessions(now);

    let mut commands = Vec::new();
    dashboard.reconcile_automatic_tabs_with(now + Duration::from_secs(59), |_, command| {
        commands.push(command);
        true
    });

    assert!(commands.is_empty());
    assert_eq!(dashboard.auto_open_channels, existing_auto_open_channels);
    assert!(dashboard.automation_enabled);
    assert_eq!(dashboard.auto_start_on_launch, auto_start_on_launch);
    assert_eq!(
        dashboard
            .streak_rotation
            .elapsed_eligible("new", now + Duration::from_secs(59)),
        Some(Duration::ZERO)
    );
    assert!(dashboard
        .queue_visibility()
        .blockers
        .iter()
        .any(|blocker| blocker.contains("paused")));

    dashboard.reconcile_automatic_tabs_with(now + Duration::from_secs(60), |_, command| {
        commands.push(command);
        true
    });

    assert!(!commands.is_empty());
}

#[test]
fn channel_snooze_skips_automation_until_its_expiry_without_changing_selection() {
    let mut dashboard = dashboard_with_pending_stream_switch(true);
    dashboard.pending_stream_switch = None;
    dashboard.auto_open_channels.clear();
    dashboard.auto_open_retry_after.clear();
    let now = dashboard.started_at.elapsed();
    let original_selection = dashboard
        .channels
        .iter()
        .find(|channel| channel.id == "new")
        .expect("new channel remains followed")
        .selection
        .clone();
    dashboard
        .automation_delays
        .snooze_channel_for("new", now, Duration::from_secs(60));

    assert!(dashboard.queue_visibility().entries.is_empty());

    let mut commands = Vec::new();
    dashboard.reconcile_automatic_tabs_with(now + Duration::from_secs(59), |_, command| {
        commands.push(command);
        true
    });
    assert!(commands.is_empty());
    assert!(dashboard.auto_open_channels.is_empty());

    dashboard.reconcile_automatic_tabs_with(now + Duration::from_secs(60), |_, command| {
        commands.push(command);
        true
    });

    assert!(!commands.is_empty());
    assert!(dashboard.auto_open_channels.contains("new"));
    assert_eq!(
        dashboard
            .channels
            .iter()
            .find(|channel| channel.id == "new")
            .expect("new channel remains followed")
            .selection,
        original_selection
    );
}
#[test]
fn followed_channel_list_ends_with_right_settings_in_compact_view() {
    let context = eframe::egui::Context::default();
    let mut dashboard = dashboard_with_pending_stream_switch(true);
    let screen_size = eframe::egui::vec2(748.0, 394.0);
    let make_input = || eframe::egui::RawInput {
        screen_rect: Some(eframe::egui::Rect::from_min_size(
            eframe::egui::Pos2::ZERO,
            screen_size,
        )),
        ..Default::default()
    };
    let mut initial_list_rect = None;
    let mut initial_viewport_top = 0.0;
    let mut initial_panel_ends = (0.0, 0.0);

    let _ = context.run(make_input(), |context| {
        eframe::egui::TopBottomPanel::top("app_header").show(context, |ui| {
            ui.add_space(50.0);
        });
        eframe::egui::CentralPanel::default().show(context, |ui| {
            let available_height = ui.available_height();
            let reserved_log_height = (available_height * 0.3).max(48.0);
            let content_max_height = (available_height - reserved_log_height).max(0.0);
            initial_viewport_top = ui.available_rect_before_wrap().min.y;
            eframe::egui::ScrollArea::vertical()
                .max_height(content_max_height)
                .show(ui, |ui| {
                    ui.add_space(8.0);
                    ui.columns(2, |columns| {
                        dashboard.watch_settings(&mut columns[1]);
                        let right_panel_bottom = columns[1].min_rect().max.y;
                        initial_list_rect =
                            Some(dashboard.channel_list(&mut columns[0], right_panel_bottom));
                        initial_panel_ends = (columns[0].min_rect().max.y, right_panel_bottom);
                    });
                });
        });
    });

    let initial_list_rect = initial_list_rect.expect("followed-channel list was not rendered");
    let initial_end_gap = initial_panel_ends.0 - initial_panel_ends.1;
    assert!(
        initial_end_gap.abs() <= 2.0,
        "initial left panel ends {} points from right settings",
        initial_end_gap
    );
    let scroll_offset = (initial_list_rect.min.y - initial_viewport_top).max(0.0);
    let mut list_rect = None;
    let mut viewport_top = 0.0;
    let mut panel_ends = (0.0, 0.0);

    let _ = context.run(make_input(), |context| {
        eframe::egui::TopBottomPanel::top("app_header").show(context, |ui| {
            ui.add_space(50.0);
        });
        eframe::egui::CentralPanel::default().show(context, |ui| {
            let available_height = ui.available_height();
            let reserved_log_height = (available_height * 0.3).max(48.0);
            let content_max_height = (available_height - reserved_log_height).max(0.0);
            viewport_top = ui.available_rect_before_wrap().min.y;
            eframe::egui::ScrollArea::vertical()
                .max_height(content_max_height)
                .vertical_scroll_offset(scroll_offset)
                .show(ui, |ui| {
                    ui.add_space(8.0);
                    ui.columns(2, |columns| {
                        dashboard.watch_settings(&mut columns[1]);
                        let right_panel_bottom = columns[1].min_rect().max.y;
                        list_rect =
                            Some(dashboard.channel_list(&mut columns[0], right_panel_bottom));
                        panel_ends = (columns[0].min_rect().max.y, right_panel_bottom);
                    });
                });
        });
    });

    let list_rect = list_rect.expect("followed-channel list was not rendered");
    let top_gap = list_rect.min.y - viewport_top;
    let panel_end_gap = panel_ends.0 - panel_ends.1;
    assert!(
        top_gap.abs() <= 2.0,
        "list top differs from viewport top by {top_gap} points"
    );
    assert!(
        panel_end_gap.abs() <= 2.0,
        "scrolled left panel ends {} points from right settings",
        panel_end_gap
    );
}
#[test]
fn adopted_browser_tabs_are_tracked_without_logging_an_app_open_and_release_clears_state() {
    let mut dashboard = dashboard_with_pending_stream_switch(false);
    dashboard.auto_open_channels.clear();
    dashboard.manual_open_channels.clear();
    dashboard.activity_log.clear();

    dashboard.handle_browser_message(KnownBrowserMessage::TabAdopted {
        channel_id: "new".to_owned(),
        tab_id: 7,
        surface: BrowserSurface::Player,
    });
    assert_eq!(
        dashboard.browser_states.get("new").unwrap().player_tab_id,
        Some(7),
    );
    assert!(dashboard.manual_open_channels.contains("new"));
    assert!(dashboard.activity_log.is_empty());
    assert!(dashboard.pending_stream_switch.is_some());

    dashboard.handle_browser_message(KnownBrowserMessage::TabReleased {
        channel_id: "new".to_owned(),
        tab_id: 7,
        surface: BrowserSurface::Player,
    });
    assert!(!dashboard.browser_states.contains_key("new"));
    assert!(dashboard.pending_stream_switch.is_none());
    assert!(!dashboard.manual_open_channels.contains("new"));
    assert!(dashboard.activity_log.is_empty());
}
#[test]
fn sent_chat_message_removes_streamer_from_both_sixteen_minute_rotations() {
    let mut dashboard = dashboard_with_pending_stream_switch(false);
    dashboard.automation_enabled = true;
    dashboard.connection_status = TwitchConnectionStatus::Connected("viewer".to_owned());
    dashboard.pending_stream_switch = None;
    let eligibility = super::TurnEligibility {
        player_playing: true,
        chat_ready: true,
        capacity_available: true,
    };
    let sessions = [
        super::StreamSession {
            channel_id: "old".to_owned(),
            eligibility: eligibility.clone(),
        },
        super::StreamSession {
            channel_id: "new".to_owned(),
            eligibility,
        },
    ];

    dashboard
        .points_rotation
        .observe_active_sessions(&sessions, std::time::Duration::ZERO)
        .unwrap();
    dashboard
        .streak_rotation
        .observe_active_sessions(&sessions, std::time::Duration::ZERO)
        .unwrap();

    dashboard.handle_browser_message(KnownBrowserMessage::ChannelStatus {
        channel_id: "old".to_owned(),
        tab_id: 1,
        surface: BrowserSurface::Player,
        player_state: Some(super::PlayerState::Playing),
        chat_state: Some(super::ChatState::Ready),
        chat_message_author_login: Some("viewer".to_owned()),
    });

    let turn_end = std::time::Duration::from_secs(16 * 60);
    assert_eq!(
        dashboard
            .points_rotation
            .observe_active_sessions(&sessions, turn_end)
            .unwrap(),
        ["new"],
    );
    assert_eq!(
        dashboard
            .streak_rotation
            .observe_active_sessions(&sessions, turn_end)
            .unwrap(),
        ["new"],
    );
}
#[test]
fn chat_message_received_while_automation_is_stopped_does_not_exclude_next_turn() {
    let mut dashboard = dashboard_with_pending_stream_switch(false);
    dashboard.pending_stream_switch = None;
    dashboard.connection_status = TwitchConnectionStatus::Connected("viewer".to_owned());
    dashboard.automation_enabled = false;
    let session = super::StreamSession {
        channel_id: "old".to_owned(),
        eligibility: super::TurnEligibility {
            player_playing: true,
            chat_ready: true,
            capacity_available: true,
        },
    };

    dashboard.handle_browser_message(KnownBrowserMessage::ChannelStatus {
        channel_id: "old".to_owned(),
        tab_id: 1,
        surface: BrowserSurface::Player,
        player_state: Some(super::PlayerState::Playing),
        chat_state: Some(super::ChatState::Ready),
        chat_message_author_login: Some("viewer".to_owned()),
    });

    dashboard
        .points_rotation
        .observe_active_sessions(std::slice::from_ref(&session), std::time::Duration::ZERO)
        .unwrap();
    dashboard
        .streak_rotation
        .observe_active_sessions(std::slice::from_ref(&session), std::time::Duration::ZERO)
        .unwrap();
    let turn_end = std::time::Duration::from_secs(16 * 60);
    assert_eq!(
        dashboard
            .points_rotation
            .observe_active_sessions(std::slice::from_ref(&session), turn_end)
            .unwrap(),
        ["old"],
    );
    assert_eq!(
        dashboard
            .streak_rotation
            .observe_active_sessions(std::slice::from_ref(&session), turn_end)
            .unwrap(),
        ["old"],
    );
}
#[test]
fn chat_message_from_other_rust_account_does_not_exclude_streamer() {
    let mut dashboard = dashboard_with_pending_stream_switch(false);
    dashboard.automation_enabled = true;
    dashboard.pending_stream_switch = None;
    dashboard.connection_status = TwitchConnectionStatus::Connected("viewer".to_owned());
    let session = super::StreamSession {
        channel_id: "old".to_owned(),
        eligibility: super::TurnEligibility {
            player_playing: true,
            chat_ready: true,
            capacity_available: true,
        },
    };

    dashboard.handle_browser_message(KnownBrowserMessage::ChannelStatus {
        channel_id: "old".to_owned(),
        tab_id: 1,
        surface: BrowserSurface::Player,
        player_state: Some(super::PlayerState::Playing),
        chat_state: Some(super::ChatState::Ready),
        chat_message_author_login: Some("another_viewer".to_owned()),
    });

    dashboard
        .points_rotation
        .observe_active_sessions(std::slice::from_ref(&session), std::time::Duration::ZERO)
        .unwrap();
    dashboard
        .streak_rotation
        .observe_active_sessions(std::slice::from_ref(&session), std::time::Duration::ZERO)
        .unwrap();
    let turn_end = std::time::Duration::from_secs(16 * 60);
    assert_eq!(
        dashboard
            .points_rotation
            .observe_active_sessions(std::slice::from_ref(&session), turn_end)
            .unwrap(),
        ["old"],
    );
    assert_eq!(
        dashboard
            .streak_rotation
            .observe_active_sessions(std::slice::from_ref(&session), turn_end)
            .unwrap(),
        ["old"],
    );
}
