use super::{
    BrowserChannelState, BrowserSurface, BrowserTabCloseReason, Dashboard, FollowedChannel,
    KnownBrowserMessage, QueuePlanner, TwitchConnectionStatus, TwitchUpdate,
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
        automation_enabled: true,
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

    dashboard.reconcile_automatic_tabs(Duration::ZERO);

    dashboard
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
    assert!(dashboard.activity_log.is_empty());
    dashboard.handle_browser_message(KnownBrowserMessage::TabClosed {
        channel_id: "old".to_owned(),
        tab_id: 99,
        surface: BrowserSurface::Player,
        close_reason: None,
    });
    assert!(dashboard.activity_log.is_empty());
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
    dashboard.reconcile_automatic_tabs(Duration::ZERO);
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
    dashboard.reconcile_automatic_tabs(Duration::ZERO);

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
    assert!(dashboard.activity_log.is_empty());

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
    dashboard.reconcile_automatic_tabs(Duration::ZERO);

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

    dashboard.reconcile_automatic_tabs(Duration::ZERO);

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

    dashboard.reconcile_automatic_tabs(Duration::ZERO);

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

    dashboard.reconcile_automatic_tabs(Duration::ZERO);

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
