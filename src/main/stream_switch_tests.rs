use super::{
    BrowserChannelState, BrowserKind, BrowserLaunchTarget, BrowserSurface, BrowserTabCloseReason,
    Dashboard, FollowedChannel, KnownBrowserMessage, PreferredQuality, QueuePlanner,
    TwitchConnectionStatus, TwitchNetworkSource, TwitchUpdate, TwitchUpdateFailure,
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
        preferred_quality: PreferredQuality::default(),
        browser_launch_target: None,
        browser_launch_attempted: None,
        browser_profiles: Vec::new(),
        browser_profile_roots: Vec::new(),
        browser_profile_data_directories: Vec::new(),
        automation_enabled: true,
        channels: vec![
            FollowedChannel::new("old", "Old Streamer"),
            new_streamer.clone(),
        ],
        followed_search: String::new(),
        started_at: Instant::now(),
        connection_status: TwitchConnectionStatus::Connected("test".to_owned()),
        connection_error: None,
        twitch_network_unavailable: false,
        twitch_network_errors: HashMap::new(),
        reload_managed_player_tabs_pending: false,
        pending_manual_open: false,
        pending_manual_open_inventory_wait_since: None,
        next_browser_command_id: 1,
        browser_command_session_id: "test-session".to_owned(),
        pending_reload_command: None,
        pending_manual_open_command: None,
        live_channel_ids: HashSet::from(["new".to_owned()]),
        connection_generation: 0,
        twitch_cancel: None,
        twitch_sender: twitch_sender.clone(),
        twitch_updates,
        browser_bridge: None,
        browser_bridge_retry_after: None,
        browser_connected: true,
        browser_command_ack_supported: true,
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
        twitch_baseline_confirmed_at: None,
        pubsub_state_confirmed_at: None,
        pubsub_last_event_at: None,
        pubsub_missed_events_since: None,
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

    reconcile_with_accepted_automatic_commands(&mut dashboard, Duration::ZERO);

    let pending_desired = dashboard
        .pending_stream_switch
        .as_ref()
        .map(|pending| pending.desired_channels.clone());
    if let Some(desired_channels) = pending_desired {
        // This fixture models the initial OpenSelected command as accepted by the bridge.
        dashboard.auto_open_channels.extend(desired_channels);
    }
    dashboard.auto_open_retry_after.clear();
    dashboard.browser_error = None;
    dashboard
}

fn reconcile_with_accepted_automatic_commands(dashboard: &mut Dashboard, now: Duration) {
    dashboard.reconcile_automatic_tabs_with(now, |_, _| true);
}

fn reconcile_with_rejected_automatic_open(dashboard: &mut Dashboard, now: Duration) {
    dashboard.reconcile_automatic_tabs_with(now, |dashboard, command| {
        if dashboard.browser_command_ack_supported
            && matches!(
                &command,
                super::BrowserCommand::OpenSelected {
                    manual_request: false,
                    ..
                }
            )
        {
            false
        } else {
            dashboard.send_bridge_command(command)
        }
    });
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
    reconcile_with_accepted_automatic_commands(&mut dashboard, Duration::ZERO);

    assert!(dashboard.live_channel_ids.contains("new"));
    assert!(dashboard.raid_redirected_channels.contains("new"));
    assert!(!dashboard.auto_open_channels.contains("new"));
    assert!(dashboard.browser_error.is_none());

    dashboard.connection_status = TwitchConnectionStatus::Disconnected;
    dashboard.live_channel_ids.clear();
    reconcile_with_accepted_automatic_commands(&mut dashboard, Duration::ZERO);
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
    reconcile_with_accepted_automatic_commands(&mut dashboard, Duration::ZERO);
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

    reconcile_with_accepted_automatic_commands(&mut dashboard, Duration::ZERO);

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
    reconcile_with_rejected_automatic_open(&mut dashboard, Duration::ZERO);

    assert!(!dashboard.auto_open_channels.contains("new"));
    assert!(dashboard.auto_open_retry_after.contains_key("new"));
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
fn rejected_automatic_open_preserves_retry_state() {
    for ack_supported in [false, true] {
        let mut dashboard = dashboard_with_pending_stream_switch(true);
        dashboard.pending_stream_switch = None;
        dashboard.auto_open_channels.clear();
        dashboard.auto_open_retry_after.clear();
        dashboard.browser_states.clear();
        dashboard.browser_connected = true;
        dashboard.browser_command_ack_supported = ack_supported;

        reconcile_with_rejected_automatic_open(&mut dashboard, Duration::ZERO);

        assert!(!dashboard.auto_open_channels.contains("new"));
        let retry_at = dashboard.auto_open_retry_after["new"];
        assert!(retry_at > Instant::now());

        dashboard
            .auto_open_retry_after
            .insert("new".to_owned(), Instant::now() - Duration::from_secs(1));
        reconcile_with_rejected_automatic_open(&mut dashboard, Duration::ZERO);

        assert!(!dashboard.auto_open_channels.contains("new"));
        assert!(dashboard.auto_open_retry_after["new"] > Instant::now());
    }
}

#[test]
fn rejected_pending_switch_open_remains_retryable() {
    let mut dashboard = dashboard_with_pending_stream_switch(true);
    dashboard.browser_command_ack_supported = true;
    dashboard
        .auto_open_retry_after
        .insert("new".to_owned(), Instant::now() - Duration::from_secs(1));

    reconcile_with_rejected_automatic_open(&mut dashboard, Duration::ZERO);

    assert!(dashboard.pending_stream_switch.is_some());
    assert!(!dashboard.auto_open_channels.contains("new"));
    assert!(dashboard.auto_open_retry_after["new"] > Instant::now());
}

#[test]
fn rejected_unsupported_open_is_retried_after_reconnect() {
    let mut dashboard = dashboard_with_pending_stream_switch(true);
    dashboard.pending_stream_switch = None;
    dashboard.auto_open_channels.clear();
    dashboard.auto_open_retry_after.clear();
    dashboard.browser_states.clear();
    dashboard.browser_connected = true;
    dashboard.browser_command_ack_supported = false;

    dashboard.reconcile_automatic_tabs(Duration::ZERO);

    assert!(!dashboard.auto_open_channels.contains("new"));
    assert!(dashboard.auto_open_retry_after.contains_key("new"));

    dashboard.handle_browser_message(KnownBrowserMessage::Ready {
        command_acknowledgments: true,
    });
    dashboard.browser_connected = true;
    dashboard.external_inventory_received = true;
    dashboard.pending_stream_switch = None;
    dashboard.auto_open_retry_after.clear();
    dashboard.reconcile_automatic_tabs_with(Duration::ZERO, |_, command| {
        matches!(command, super::BrowserCommand::OpenSelected { .. })
    });

    assert!(dashboard.auto_open_channels.contains("new"));
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
    reconcile_with_accepted_automatic_commands(&mut dashboard, Duration::ZERO);

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

    reconcile_with_accepted_automatic_commands(&mut dashboard, Duration::ZERO);

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
    reconcile_with_accepted_automatic_commands(&mut dashboard, Duration::ZERO);

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

    reconcile_with_accepted_automatic_commands(&mut dashboard, Duration::ZERO);

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

    reconcile_with_accepted_automatic_commands(&mut dashboard, Duration::ZERO);

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

    reconcile_with_accepted_automatic_commands(&mut dashboard, Duration::ZERO);

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

    reconcile_with_accepted_automatic_commands(&mut dashboard, Duration::ZERO);

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

    reconcile_with_accepted_automatic_commands(&mut dashboard, Duration::ZERO);

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
    reconcile_with_accepted_automatic_commands(&mut dashboard, Duration::ZERO);
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

    reconcile_with_accepted_automatic_commands(&mut dashboard, Duration::ZERO);
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
fn pubsub_reconnect_keeps_missed_event_warning_and_separates_live_baseline() {
    let mut dashboard = dashboard_with_pending_stream_switch(true);
    assert!(dashboard
        .twitch_sender
        .send(TwitchUpdate::LiveStatus {
            generation: 0,
            live_channel_ids: HashSet::from(["new".to_owned()]),
        })
        .is_ok());
    dashboard.poll_twitch_updates();
    let baseline_confirmed_at = dashboard.twitch_baseline_confirmed_at;
    assert!(baseline_confirmed_at.is_some());

    assert!(dashboard
        .twitch_sender
        .send(TwitchUpdate::PubSubStatus {
            generation: 0,
            connected: false,
            detail: Some("Connection interrupted".to_owned()),
        })
        .is_ok());
    dashboard.poll_twitch_updates();
    let missed_events_since = dashboard.pubsub_missed_events_since;
    assert!(missed_events_since.is_some());

    assert!(dashboard
        .twitch_sender
        .send(TwitchUpdate::PubSubStatus {
            generation: 0,
            connected: true,
            detail: None,
        })
        .is_ok());
    dashboard.poll_twitch_updates();
    assert!(dashboard.pubsub_connected);
    assert_eq!(dashboard.pubsub_missed_events_since, missed_events_since);

    assert!(dashboard
        .twitch_sender
        .send(TwitchUpdate::PointsEarned {
            generation: 0,
            channel_id: "new".to_owned(),
            points_earned: 50,
            balance: 150,
            reason_code: "WATCH_STREAK".to_owned(),
        })
        .is_ok());
    dashboard.poll_twitch_updates();
    assert!(dashboard.pubsub_last_event_at.is_some());
    assert_eq!(
        dashboard.twitch_baseline_confirmed_at,
        baseline_confirmed_at
    );
    assert_eq!(dashboard.pubsub_missed_events_since, missed_events_since);
}

#[test]
fn starting_pubsub_gap_is_visible_before_first_status_confirmation() {
    let mut dashboard = dashboard_with_pending_stream_switch(true);
    dashboard.note_pubsub_gap(Instant::now());

    assert!(dashboard.pubsub_missed_events_since.is_some());
    assert!(dashboard.pubsub_state_confirmed_at.is_none());
    assert!(!dashboard.pubsub_connected);
}

#[test]
fn successful_helix_snapshot_marks_network_recovered_and_queues_managed_reload() {
    let mut dashboard = dashboard_with_pending_stream_switch(true);
    assert!(dashboard
        .twitch_sender
        .send(TwitchUpdate::NetworkUnavailable {
            generation: 0,
            source: TwitchNetworkSource::LiveStatus,
            message: "network dropped".to_owned(),
        })
        .is_ok());
    dashboard.poll_twitch_updates();
    assert!(dashboard.twitch_network_unavailable);
    assert!(!dashboard.reload_managed_player_tabs_pending);

    assert!(dashboard
        .twitch_sender
        .send(TwitchUpdate::LiveStatus {
            generation: 0,
            live_channel_ids: HashSet::from(["new".to_owned()]),
        })
        .is_ok());
    dashboard.poll_twitch_updates();
    assert!(!dashboard.twitch_network_unavailable);
    assert!(dashboard.reload_managed_player_tabs_pending);
}

#[test]
fn follower_and_live_status_recover_independently() {
    let mut dashboard = dashboard_with_pending_stream_switch(true);
    dashboard
        .twitch_sender
        .send(TwitchUpdate::NetworkUnavailable {
            generation: 0,
            source: TwitchNetworkSource::FollowedChannels,
            message: "live status unavailable".to_owned(),
        })
        .unwrap();
    dashboard.poll_twitch_updates();

    let refreshed_channels = vec![FollowedChannel::new("fresh", "Fresh")];
    dashboard
        .twitch_sender
        .send(TwitchUpdate::FollowedChannels {
            generation: 0,
            channels: refreshed_channels,
            live_status_failure: TwitchUpdateFailure::Retryable(
                "live status unavailable".to_owned(),
            ),
        })
        .unwrap();
    dashboard.poll_twitch_updates();
    assert_eq!(dashboard.channels[0].id, "fresh");
    assert!(dashboard.twitch_network_unavailable);
    dashboard.browser_error = None;
    dashboard.service_network_recovery();
    assert!(dashboard.browser_error.is_none());
    assert!(dashboard.reload_managed_player_tabs_pending);

    dashboard
        .twitch_sender
        .send(TwitchUpdate::LiveStatus {
            generation: 0,
            live_channel_ids: HashSet::from(["fresh".to_owned()]),
        })
        .unwrap();
    dashboard.poll_twitch_updates();

    assert!(!dashboard.twitch_network_unavailable);
    assert!(dashboard.reload_managed_player_tabs_pending);
}

#[test]
fn live_status_warning_blocks_recovery_actions() {
    let mut dashboard = dashboard_with_pending_stream_switch(true);
    dashboard.browser_error = None;
    dashboard
        .twitch_sender
        .send(TwitchUpdate::NetworkUnavailable {
            generation: 0,
            source: TwitchNetworkSource::FollowedChannels,
            message: "followed channels unavailable".to_owned(),
        })
        .unwrap();
    dashboard.poll_twitch_updates();

    dashboard
        .twitch_sender
        .send(TwitchUpdate::FollowedChannels {
            generation: 0,
            channels: vec![FollowedChannel::new("fresh", "Fresh")],
            live_status_failure: TwitchUpdateFailure::Warning(
                "live status response was invalid".to_owned(),
            ),
        })
        .unwrap();
    dashboard.poll_twitch_updates();

    assert!(!dashboard.twitch_network_unavailable);
    assert!(dashboard.reload_managed_player_tabs_pending);
    assert!(dashboard.connection_error.is_some());
    dashboard.browser_error = None;

    dashboard.service_network_recovery();

    assert!(dashboard.reload_managed_player_tabs_pending);
    assert!(dashboard.browser_error.is_none());
}

#[test]
fn manual_open_is_queued_until_twitch_and_browser_are_ready() {
    let mut dashboard = dashboard_with_pending_stream_switch(true);
    dashboard.channels[0].login = "new_streamer".to_owned();
    dashboard.twitch_network_unavailable = true;
    dashboard.open_selected_twitch_channels();
    assert!(dashboard.pending_manual_open);

    dashboard.twitch_network_unavailable = false;
    dashboard.browser_connected = false;
    dashboard.service_pending_manual_open();
    assert!(dashboard.pending_manual_open);

    dashboard.browser_error = None;
    dashboard.browser_connected = true;
    dashboard.external_inventory_received = true;
    let mut replayed_channels: Option<Vec<serde_json::Value>> = None;
    let mut request_id = None;
    dashboard.service_pending_manual_open_with(|_, command| {
        let super::BrowserCommand::OpenSelected {
            channels,
            command_id,
            ..
        } = command
        else {
            return false;
        };
        request_id = Some(command_id);
        replayed_channels = Some(
            channels
                .into_iter()
                .map(|channel| serde_json::to_value(channel).unwrap())
                .collect(),
        );
        true
    });
    dashboard.handle_browser_message(KnownBrowserMessage::CommandAcknowledged {
        command_id: request_id.unwrap(),
    });
    assert!(!dashboard.pending_manual_open);
    let replayed_channels = replayed_channels.unwrap();
    assert_eq!(replayed_channels.len(), 1);
    assert_eq!(replayed_channels[0]["id"], "new");
    assert_eq!(replayed_channels[0]["login"], "new_streamer");
    assert!(dashboard.manual_open_channels.contains("new"));
}
#[test]
fn queued_manual_open_reports_missing_inventory_without_dropping_request() {
    let mut dashboard = dashboard_with_pending_stream_switch(true);
    dashboard.pending_manual_open = true;
    dashboard.external_inventory_received = false;
    dashboard.browser_error = None;
    dashboard.pending_manual_open_inventory_wait_since =
        Some(Instant::now() - Duration::from_secs(60));

    dashboard.service_pending_manual_open();

    assert!(dashboard.pending_manual_open);
    assert!(
        dashboard
            .browser_error
            .as_deref()
            .is_some_and(|error| error.contains("inventory")),
        "browser_error={:?}, inventory_wait_since={:?}, connected={}, inventory_received={}, network_unavailable={}, connection_error={:?}",
        dashboard.browser_error,
        dashboard.pending_manual_open_inventory_wait_since,
        dashboard.browser_connected,
        dashboard.external_inventory_received,
        dashboard.twitch_network_unavailable,
        dashboard.connection_error,
    );
}

#[test]
fn managed_player_reload_command_uses_extension_wire_name() {
    assert_eq!(
        serde_json::to_value(super::BrowserCommand::ReloadManagedPlayerTabs {
            command_id: "session-reload".to_owned(),
        })
        .unwrap(),
        serde_json::json!({
            "type": "reload_managed_player_tabs",
            "command_id": "session-reload",
        })
    );
}

#[test]
fn cancelling_twitch_connection_clears_queued_recovery_actions() {
    let mut dashboard = dashboard_with_pending_stream_switch(true);
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    dashboard.twitch_cancel = Some(std::sync::Arc::clone(&cancel));
    dashboard.connection_status = TwitchConnectionStatus::Connecting;
    dashboard.twitch_network_unavailable = true;
    dashboard.reload_managed_player_tabs_pending = true;
    dashboard.pending_manual_open = true;
    dashboard.connection_error = Some("network unavailable".to_owned());
    let generation = dashboard.connection_generation;

    dashboard.cancel_twitch_connection();

    assert!(cancel.load(std::sync::atomic::Ordering::Acquire));
    assert_eq!(dashboard.connection_generation, generation.wrapping_add(1));
    assert!(matches!(
        dashboard.connection_status,
        TwitchConnectionStatus::Disconnected
    ));
    assert!(!dashboard.twitch_network_unavailable);
    assert!(!dashboard.reload_managed_player_tabs_pending);
    assert!(!dashboard.pending_manual_open);
    assert!(dashboard.connection_error.is_none());
}
#[test]
fn failed_reload_command_is_requeued_after_bridge_disconnect() {
    let mut dashboard = dashboard_with_pending_stream_switch(true);
    dashboard.external_inventory_received = true;
    let recovery_generation = dashboard.connection_generation;
    let request_id = "reload-request".to_owned();
    dashboard.reload_managed_player_tabs_pending = true;
    dashboard.pending_reload_command = Some(super::PendingBridgeCommand {
        request_id: request_id.clone(),
        connection_generation: recovery_generation,
        sent_at: Some(Instant::now()),
        command: None,
    });

    dashboard.handle_failed_bridge_command(
        super::BrowserCommand::ReloadManagedPlayerTabs {
            command_id: request_id,
        },
        Some(recovery_generation),
        "socket write failed".to_owned(),
    );

    assert!(dashboard.reload_managed_player_tabs_pending);
    assert!(!dashboard.browser_connected);
    assert!(!dashboard.external_inventory_received);
    assert!(dashboard
        .pending_reload_command
        .as_ref()
        .is_some_and(|pending| pending.sent_at.is_none()));
    dashboard.handle_browser_message(KnownBrowserMessage::CommandAcknowledged {
        command_id: "reload-request".to_owned(),
    });
    assert!(!dashboard.reload_managed_player_tabs_pending);
    assert!(dashboard.pending_reload_command.is_none());
}

#[test]
fn failed_manual_open_replays_original_request_after_reconnect() {
    let mut dashboard = dashboard_with_pending_stream_switch(true);
    dashboard.channels[0].login = "original".to_owned();

    let recovery_generation = dashboard.connection_generation;
    let request_id = "manual-request".to_owned();
    let original_command = super::BrowserCommand::OpenSelected {
        wake_browser: true,
        channels: vec![super::BrowserChannel::from(&dashboard.channels[0])],
        command_id: request_id.clone(),
        manual_request: true,
    };
    dashboard.pending_manual_open = true;
    dashboard.pending_manual_open_command = Some(super::PendingBridgeCommand {
        request_id,
        connection_generation: recovery_generation,
        sent_at: Some(Instant::now()),
        command: Some(original_command.clone()),
    });
    dashboard.handle_failed_bridge_command(
        original_command.clone(),
        Some(recovery_generation),
        "socket write failed".to_owned(),
    );

    assert!(dashboard.pending_manual_open);
    assert!(!dashboard.browser_connected);
    assert!(dashboard.pending_manual_open_inventory_wait_since.is_none());

    dashboard.channels[0].login = "reselected".to_owned();
    dashboard.browser_error = None;
    dashboard.browser_connected = true;
    dashboard.external_inventory_received = true;
    let mut replayed_command = None;
    dashboard.service_pending_manual_open_with(|_, command| {
        replayed_command = Some(serde_json::to_value(command).unwrap());
        true
    });

    let replayed_command = replayed_command.unwrap();
    assert_eq!(
        replayed_command,
        serde_json::to_value(original_command).unwrap()
    );
    dashboard.handle_browser_message(KnownBrowserMessage::CommandAcknowledged {
        command_id: replayed_command["command_id"].as_str().unwrap().to_owned(),
    });
    assert!(!dashboard.pending_manual_open);
}

#[test]
fn late_bridge_failure_cannot_restore_canceled_recovery_intents() {
    let mut dashboard = dashboard_with_pending_stream_switch(true);
    let recovery_generation = dashboard.connection_generation;
    dashboard.cancel_twitch_connection();

    dashboard.handle_failed_bridge_command(
        super::BrowserCommand::ReloadManagedPlayerTabs {
            command_id: "stale-reload".to_owned(),
        },
        Some(recovery_generation),
        "late socket write failure".to_owned(),
    );
    dashboard.handle_failed_bridge_command(
        super::BrowserCommand::OpenSelected {
            wake_browser: false,
            channels: Vec::new(),
            command_id: "stale-open".to_owned(),
            manual_request: true,
        },
        Some(recovery_generation),
        "late socket write failure".to_owned(),
    );

    assert!(!dashboard.reload_managed_player_tabs_pending);
    assert!(!dashboard.pending_manual_open);
    assert!(matches!(
        dashboard.connection_status,
        TwitchConnectionStatus::Disconnected
    ));
}
#[test]
fn supported_extension_tracks_manual_open_until_acknowledged() {
    let mut dashboard = dashboard_with_pending_stream_switch(true);
    dashboard.handle_browser_message(KnownBrowserMessage::Ready {
        command_acknowledgments: true,
    });
    dashboard.browser_error = None;
    dashboard.external_inventory_received = true;
    assert!(dashboard.browser_command_ack_supported);
    dashboard.pending_manual_open = true;
    dashboard.channels[0].login = "original".to_owned();
    dashboard.channels[0].selection.streak = true;
    dashboard.connection_status = TwitchConnectionStatus::Connected("test".to_owned());
    let mut sent_command = None;
    dashboard.browser_connected = true;
    dashboard.service_pending_manual_open_with(|_, command| {
        sent_command = Some(serde_json::to_value(command).unwrap());
        true
    });

    let sent_command = sent_command.unwrap();
    assert_eq!(sent_command["manual_request"], true);
    let command_id = sent_command["command_id"].as_str().unwrap().to_owned();
    assert_eq!(
        dashboard
            .pending_manual_open_command
            .as_ref()
            .unwrap()
            .request_id,
        command_id
    );
    dashboard.handle_browser_message(KnownBrowserMessage::CommandAcknowledged { command_id });
    assert!(dashboard.pending_manual_open_command.is_none());
    assert!(!dashboard.pending_manual_open);
}

#[test]
fn manual_open_timeout_retries_same_command_id_and_payload() {
    let mut dashboard = dashboard_with_pending_stream_switch(true);
    dashboard.channels[0].login = "original".to_owned();
    dashboard.channels[0].selection.streak = true;
    dashboard.pending_manual_open = true;
    let mut first_command = None;
    dashboard.browser_connected = true;
    dashboard.browser_error = None;
    dashboard.service_pending_manual_open_with(|_, command| {
        first_command = Some(serde_json::to_value(command).unwrap());
        true
    });
    let first_command = first_command.unwrap();

    dashboard.channels[0].login = "reselected".to_owned();
    dashboard
        .pending_manual_open_command
        .as_mut()
        .unwrap()
        .sent_at = Some(Instant::now() - Duration::from_secs(11));
    let mut retry = None;
    dashboard.service_pending_manual_open_with(|_, command| {
        retry = Some(serde_json::to_value(command).unwrap());
        true
    });

    let retry = retry.unwrap();
    assert_eq!(retry["command_id"], first_command["command_id"]);
    assert_eq!(retry, first_command);
    dashboard.handle_browser_message(KnownBrowserMessage::CommandAcknowledged {
        command_id: retry["command_id"].as_str().unwrap().to_owned(),
    });
    assert!(!dashboard.pending_manual_open);
}

#[test]
fn legacy_extension_blocks_direct_manual_automation_and_reload_commands() {
    let mut dashboard = dashboard_with_pending_stream_switch(true);
    dashboard.handle_browser_message(KnownBrowserMessage::Ready {
        command_acknowledgments: false,
    });
    dashboard.browser_connected = true;
    dashboard.channels[0].selection.streak = true;
    assert!(!dashboard.external_inventory_received);
    assert!(!dashboard.browser_command_ack_supported);
    assert!(dashboard
        .browser_error
        .as_deref()
        .is_some_and(|error| error.contains("0.3.18")));

    dashboard.open_selected_twitch_channels();
    assert!(dashboard.pending_manual_open);
    assert!(dashboard.pending_manual_open_command.is_none());
    assert!(dashboard
        .browser_error
        .as_deref()
        .is_some_and(|error| error.contains("0.3.18")));

    assert!(
        !dashboard.send_bridge_command(super::BrowserCommand::OpenSelected {
            wake_browser: false,
            channels: Vec::new(),
            command_id: "automatic-open".to_owned(),
            manual_request: false,
        })
    );
    assert!(
        !dashboard.send_bridge_command(super::BrowserCommand::ReloadManagedPlayerTabs {
            command_id: "managed-reload".to_owned(),
        })
    );
    assert!(dashboard
        .browser_error
        .as_deref()
        .is_some_and(|error| error.contains("0.3.18")));
}
#[test]
fn reconnect_requeues_original_manual_command_for_deduplication() {
    let mut dashboard = dashboard_with_pending_stream_switch(true);
    dashboard.channels[0].login = "original".to_owned();
    dashboard.channels[0].selection.streak = true;
    let request_id = "reconnect-request".to_owned();
    let original_command = super::BrowserCommand::OpenSelected {
        wake_browser: true,
        channels: vec![super::BrowserChannel::from(&dashboard.channels[0])],
        command_id: request_id.clone(),
        manual_request: true,
    };
    dashboard.pending_manual_open_command = Some(super::PendingBridgeCommand {
        request_id,
        connection_generation: dashboard.connection_generation,
        sent_at: Some(Instant::now()),
        command: Some(original_command.clone()),
    });
    dashboard.pending_manual_open = false;

    dashboard.requeue_unacknowledged_browser_commands();
    assert!(dashboard.pending_manual_open);
    assert!(dashboard
        .pending_manual_open_command
        .as_ref()
        .is_some_and(|pending| pending.sent_at.is_none()));
    dashboard.channels[0].login = "reselected".to_owned();
    let mut replayed = None;
    dashboard.browser_connected = true;
    dashboard.browser_error = None;
    dashboard.external_inventory_received = true;
    dashboard.browser_command_ack_supported = true;
    dashboard.service_pending_manual_open_with(|_, command| {
        replayed = Some(serde_json::to_value(command).unwrap());
        true
    });

    assert_eq!(
        replayed.unwrap(),
        serde_json::to_value(original_command).unwrap()
    );
    dashboard.handle_browser_message(KnownBrowserMessage::CommandAcknowledged {
        command_id: "reconnect-request".to_owned(),
    });
    assert!(!dashboard.pending_manual_open);
}
#[test]
fn failed_twitch_update_invalidates_pending_recovery_commands() {
    let mut dashboard = dashboard_with_pending_stream_switch(false);
    dashboard.reload_managed_player_tabs_pending = true;
    dashboard.pending_reload_command = Some(super::PendingBridgeCommand {
        request_id: "reload-request".to_owned(),
        connection_generation: dashboard.connection_generation,
        sent_at: None,
        command: None,
    });
    let generation = dashboard.connection_generation;

    assert!(dashboard
        .twitch_sender
        .send(TwitchUpdate::Failed {
            generation,
            message: "connection failed".to_owned(),
        })
        .is_ok());
    dashboard.poll_twitch_updates();

    assert_eq!(dashboard.connection_generation, generation.wrapping_add(1));
    assert!(!dashboard.reload_managed_player_tabs_pending);
    assert!(dashboard.pending_reload_command.is_none());
}
