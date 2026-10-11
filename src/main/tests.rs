use super::{
    active_external_stream_count, enqueue_points_notice, followed_channel_search_indices,
    points_session_for, resume_due_auto_open_retries, schedule_auto_open_retry,
    BrowserChannelState, ChatState, FollowedChannel, PlayerState, PointsNotice,
    AUTO_TAB_RETRY_DELAY, BROWSER_STATE_STALE_AFTER,
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
        player_state: Some(PlayerState::Playing),
        chat_state: Some(ChatState::Ready),
        player_last_seen: Some(now),
        chat_last_seen: Some(now),
    };

    assert!(points_session_for("a", Some(&state), now, true)
        .eligibility
        .is_eligible());

    state.player_state = Some(PlayerState::Buffering);
    assert!(!points_session_for("a", Some(&state), now, true)
        .eligibility
        .is_eligible());
    state.player_state = Some(PlayerState::Unknown);
    assert!(!points_session_for("a", Some(&state), now, true)
        .eligibility
        .is_eligible());
    state.player_state = Some(PlayerState::Playing);
    state.chat_state = Some(ChatState::Loading);
    assert!(!points_session_for("a", Some(&state), now, true)
        .eligibility
        .is_eligible());
    state.chat_state = Some(ChatState::Unknown);
    assert!(!points_session_for("a", Some(&state), now, true)
        .eligibility
        .is_eligible());
    state.chat_state = Some(ChatState::Ready);
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
