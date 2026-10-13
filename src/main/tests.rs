use super::{
    active_app_stream_count, active_external_stream_count, followed_channel_search_indices,
    push_activity_log, record_claim_available_once, restore_managed_channel_ownership,
    resume_due_auto_open_retries, schedule_auto_open_retry, stream_turn_session_for,
    ActivityLogEntry, BrowserChannelState, ChatState, FollowedChannel, PlayerState, QueueCandidate,
    QueuePlanner, AUTO_TAB_RETRY_DELAY, BROWSER_STATE_STALE_AFTER, MAX_ACTIVITY_LOG_ENTRIES,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};
use twitch_streak_saver::model::ChannelSelection;

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
fn activity_log_keeps_timestamp_streamer_and_latest_events() {
    let mut log = VecDeque::new();
    let entry = ActivityLogEntry {
        time: "21:15:09".to_owned(),
        streamer: "First streamer".to_owned(),
        event: "Watch Streak completed; reward: 400 points.".to_owned(),
    };
    push_activity_log(&mut log, entry.clone());
    assert_eq!(log.front(), Some(&entry));

    for index in 1..=MAX_ACTIVITY_LOG_ENTRIES {
        push_activity_log(
            &mut log,
            ActivityLogEntry {
                time: format!("00:00:{:02}", index % 60),
                streamer: format!("streamer-{index}"),
                event: format!("event-{index}"),
            },
        );
    }

    assert_eq!(log.len(), MAX_ACTIVITY_LOG_ENTRIES);
    assert_eq!(log.front().unwrap().event, "event-200");
    assert_eq!(log.back().unwrap().event, "event-1");
}

#[test]
fn claim_availability_is_logged_once_per_available_bonus() {
    let mut available = HashSet::new();
    let mut log = VecDeque::new();

    for _ in 0..2 {
        record_claim_available_once(
            &mut available,
            &mut log,
            "channel-1".to_owned(),
            "Streamer One".to_owned(),
        );
    }
    assert_eq!(log.len(), 1);
    assert_eq!(log.front().unwrap().streamer, "Streamer One");
    assert_eq!(
        log.front().unwrap().event,
        "Bonus available to claim manually on Twitch."
    );

    record_claim_available_once(
        &mut available,
        &mut log,
        "channel-2".to_owned(),
        "Streamer Two".to_owned(),
    );
    assert_eq!(log.len(), 2);
    assert_eq!(log.front().unwrap().streamer, "Streamer Two");
}

#[test]
fn points_eligibility_uses_fresh_playing_video_ready_chat_and_capacity() {
    let now = Instant::now();
    let mut state = BrowserChannelState {
        player_tab_id: Some(1),
        chat_tab_id: Some(2),
        player_state: Some(PlayerState::Playing),
        chat_state: Some(ChatState::Ready),
        player_last_seen: Some(now),
        chat_last_seen: Some(now),
    };

    assert!(stream_turn_session_for("a", Some(&state), now, true)
        .eligibility
        .is_eligible());

    state.player_state = Some(PlayerState::Buffering);
    assert!(!stream_turn_session_for("a", Some(&state), now, true)
        .eligibility
        .is_eligible());
    state.player_state = Some(PlayerState::Unknown);
    assert!(!stream_turn_session_for("a", Some(&state), now, true)
        .eligibility
        .is_eligible());
    state.player_state = Some(PlayerState::Playing);
    state.chat_state = Some(ChatState::Loading);
    assert!(!stream_turn_session_for("a", Some(&state), now, true)
        .eligibility
        .is_eligible());
    state.chat_state = Some(ChatState::Unknown);
    assert!(!stream_turn_session_for("a", Some(&state), now, true)
        .eligibility
        .is_eligible());
    state.chat_state = Some(ChatState::Ready);
    state.player_last_seen = Some(
        now.checked_sub(BROWSER_STATE_STALE_AFTER + Duration::from_secs(1))
            .unwrap(),
    );
    assert!(!stream_turn_session_for("a", Some(&state), now, true)
        .eligibility
        .is_eligible());
    assert!(!stream_turn_session_for("a", Some(&state), now, false)
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
fn reconnect_inventory_restores_managed_channel_ownership() {
    let mut automatic = HashSet::from(["automatic".to_owned(), "closed-auto".to_owned()]);
    let mut manual = HashSet::from(["manual".to_owned(), "closed-manual".to_owned()]);

    restore_managed_channel_ownership(
        [
            "automatic".to_owned(),
            "manual".to_owned(),
            "orphan".to_owned(),
            String::new(),
        ],
        &mut automatic,
        &mut manual,
    );

    assert_eq!(automatic, HashSet::from(["automatic".to_owned()]));
    assert_eq!(
        manual,
        HashSet::from(["manual".to_owned(), "orphan".to_owned()])
    );
}

#[test]
fn reconnected_managed_streams_reserve_automatic_open_slots() {
    let followed = vec![
        FollowedChannel::new("restored-live", "Restored live"),
        FollowedChannel::new("offline", "Offline"),
    ];
    let live = HashSet::from(["restored-live".to_owned()]);
    let candidates = [
        QueueCandidate {
            channel_id: "candidate-a",
            is_live: true,
            selection: ChannelSelection {
                streak: false,
                points: true,
            },
            streak_completed_for_live: false,
        },
        QueueCandidate {
            channel_id: "candidate-b",
            is_live: true,
            selection: ChannelSelection {
                streak: false,
                points: true,
            },
            streak_completed_for_live: false,
        },
    ];
    let mut automatic = HashSet::new();
    let mut manual = HashSet::new();
    restore_managed_channel_ownership(["restored-live".to_owned()], &mut automatic, &mut manual);
    let managed_count = active_app_stream_count(&manual, &followed, &live);
    assert_eq!(managed_count, 1);
    let plan = QueuePlanner::default().plan(candidates.iter().copied(), managed_count);
    assert_eq!(plan.channel_ids, ["candidate-a"]);

    restore_managed_channel_ownership(
        [
            "restored-live".to_owned(),
            "unfollowed".to_owned(),
            "offline".to_owned(),
        ],
        &mut automatic,
        &mut manual,
    );
    let managed_count = active_app_stream_count(&manual, &followed, &live);
    assert_eq!(managed_count, 2);
    let plan = QueuePlanner::default().plan(candidates.iter().copied(), managed_count);
    assert!(plan.channel_ids.is_empty());
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

    let due = resume_due_auto_open_retries(&mut active, &mut retries, now, false);
    assert_eq!(due, vec!["due".to_owned()]);

    assert_eq!(active, HashSet::from(["later".to_owned()]));
    assert_eq!(
        retries,
        HashMap::from([("later".to_owned(), now + Duration::from_secs(10))]),
    );
}
