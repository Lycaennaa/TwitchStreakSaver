use super::{
    QueueCandidate, QueuePlanner, QueueStage, StreamRotation, StreamRotationError, StreamSession,
    StreamTurnTimer, TurnEligibility, MAX_TRACKED_STREAM_SESSIONS, STREAM_TURN_DURATION,
};
use crate::model::ChannelSelection;
use std::collections::HashSet;
use std::time::Duration;

fn candidate<'a>(
    channel_id: &'a str,
    is_live: bool,
    streak: bool,
    points: bool,
    streak_completed_for_live: bool,
) -> QueueCandidate<'a> {
    QueueCandidate {
        channel_id,
        is_live,
        selection: ChannelSelection { streak, points },
        streak_completed_for_live,
    }
}

#[test]
fn earn_mode_prioritizes_streaks_and_respects_manual_capacity() {
    let channels = [
        candidate("streak-a", true, true, false, false),
        candidate("points-a", true, false, true, false),
        candidate("streak-b", true, true, false, false),
    ];
    let planner = QueuePlanner::default();
    let plan = planner.plan(channels.iter().copied(), 1);
    assert_eq!(plan.stage, QueueStage::Streak);
    assert_eq!(plan.channel_ids, ["streak-a"]);
}
#[test]
fn planner_accepts_a_single_pass_candidate_source() {
    let channels = [
        candidate("streak-a", true, true, false, false),
        candidate("streak-b", true, true, false, false),
    ];
    let mut source = channels.into_iter();
    let plan = QueuePlanner::default().plan(std::iter::from_fn(|| source.next()), 0);

    assert_eq!(plan.stage, QueueStage::Streak);
    assert_eq!(plan.channel_ids, ["streak-a", "streak-b"]);
}

#[test]
fn completed_streak_channels_enter_points_queue_after_streak_phase() {
    let channels = [
        candidate("both-a", true, true, true, true),
        candidate("both-b", true, true, true, true),
        candidate("points-only", true, false, true, false),
    ];
    let mut planner = QueuePlanner::default();
    planner.record_streak_completed("both-b");
    let plan = planner.plan(channels.iter().copied(), 0);

    assert_eq!(plan.stage, QueueStage::Points);
    assert_eq!(plan.channel_ids, ["both-a", "both-b"]);
}

#[test]
fn points_turn_advances_round_robin_from_the_completed_channel() {
    let channels = [
        candidate("a", true, false, true, false),
        candidate("b", true, false, true, false),
        candidate("c", true, false, true, false),
    ];
    let mut planner = QueuePlanner::default();

    let first = planner.plan(channels.iter().copied(), 0);
    assert_eq!(first.channel_ids, ["a", "b"]);

    planner.record_points_turn_finished("a");
    let next = planner.plan(channels.iter().copied(), 0);
    assert_eq!(next.channel_ids, ["b", "c"]);
}
#[test]
fn timed_streak_turn_yields_to_points_without_a_streak_event() {
    let channels = [
        candidate("a", true, true, true, false),
        candidate("b", true, true, true, false),
        candidate("c", true, false, true, false),
    ];
    let mut planner = QueuePlanner::default();
    let first = planner.plan(channels.iter().copied(), 0);
    assert_eq!(first.stage, QueueStage::Streak);
    assert_eq!(first.channel_ids, ["a", "b"]);

    let eligible = TurnEligibility {
        player_playing: true,
        chat_ready: true,
        capacity_available: true,
    };
    let sessions = first
        .channel_ids
        .iter()
        .map(|channel_id| StreamSession {
            channel_id: channel_id.clone(),
            eligibility: eligible,
        })
        .collect::<Vec<_>>();
    let mut rotation = StreamRotation::default();
    assert_eq!(
        rotation.observe_active_sessions(&sessions, Duration::ZERO),
        Ok(Vec::new())
    );
    let completed = rotation
        .observe_active_sessions(&sessions, STREAM_TURN_DURATION)
        .unwrap();
    assert_eq!(completed, ["a", "b"]);

    for channel_id in completed {
        planner.record_streak_turn_timed_out(channel_id.clone(), STREAM_TURN_DURATION);
        rotation.reset_completed_turn(&channel_id);
    }
    let points = planner.plan(channels.iter().copied(), 0);
    assert_eq!(points.stage, QueueStage::Points);
    assert_eq!(points.channel_ids, ["c", "a"]);

    planner.record_points_turn_finished("c");
    let streak = planner.plan(channels.iter().copied(), 0);
    assert_eq!(streak.stage, QueueStage::Streak);
    assert_eq!(streak.channel_ids, ["a", "b"]);
}

#[test]
fn timed_out_streak_only_channels_continue_cycling_without_points() {
    let channels = [
        candidate("a", true, true, false, false),
        candidate("b", true, true, false, false),
        candidate("c", true, true, false, false),
    ];
    let mut planner = QueuePlanner::default();
    let first = planner.plan(channels.iter().copied(), 0);
    assert_eq!(first.channel_ids, ["a", "b"]);
    planner.record_streak_turn_timed_out("a", Duration::ZERO);
    planner.record_streak_turn_timed_out("b", Duration::ZERO);

    let remaining = planner.plan(channels.iter().copied(), 0);
    assert_eq!(remaining.stage, QueueStage::Streak);
    assert_eq!(remaining.channel_ids, ["c"]);
    planner.record_streak_turn_timed_out("c", Duration::ZERO);

    let next = planner.plan(channels.iter().copied(), 0);
    assert_eq!(next.stage, QueueStage::Streak);
    assert_eq!(next.channel_ids, ["a", "b"]);
}

#[test]
fn timed_out_streak_only_streams_yield_to_points_only_candidate() {
    let channels = [
        candidate("a", true, true, false, false),
        candidate("b", true, true, false, false),
        candidate("c", true, false, true, false),
    ];
    let mut planner = QueuePlanner::default();
    let first = planner.plan(channels.iter().copied(), 0);
    assert_eq!(first.stage, QueueStage::Streak);
    assert_eq!(first.channel_ids, ["a", "b"]);

    planner.record_streak_turn_timed_out("a", Duration::ZERO);
    planner.record_streak_turn_timed_out("b", Duration::ZERO);
    let next = planner.plan(channels.iter().copied(), 0);
    assert_eq!(next.stage, QueueStage::Points);
    assert_eq!(next.channel_ids, ["c"]);
}
#[test]
fn timed_out_streak_deferral_expires_after_the_channel_goes_offline() {
    let mut planner = QueuePlanner::default();
    planner.record_streak_turn_timed_out("a", Duration::ZERO);
    planner.retain_live_channels(&HashSet::from(["b".to_owned(), "c".to_owned()]));

    let channels = [
        candidate("a", true, true, false, false),
        candidate("b", false, true, false, false),
        candidate("c", true, false, true, false),
    ];
    let plan = planner.plan(channels.iter().copied(), 0);
    assert_eq!(plan.stage, QueueStage::Streak);
    assert_eq!(plan.channel_ids, ["a"]);
}
#[test]
fn stalled_points_queue_cannot_defer_streak_priority_forever() {
    let channels = [
        candidate("streak", true, true, false, false),
        candidate("points", true, false, true, false),
    ];
    let mut planner = QueuePlanner::default();
    planner.record_streak_turn_timed_out("streak", Duration::ZERO);

    let points = planner.plan(channels.iter().copied(), 0);
    assert_eq!(points.stage, QueueStage::Points);
    assert_eq!(points.channel_ids, ["points"]);

    planner.expire_streak_turn_deferrals(STREAM_TURN_DURATION - Duration::from_secs(1));
    let still_points = planner.plan(channels.iter().copied(), 0);
    assert_eq!(still_points.stage, QueueStage::Points);

    planner.expire_streak_turn_deferrals(STREAM_TURN_DURATION);
    let streak = planner.plan(channels.iter().copied(), 0);
    assert_eq!(streak.stage, QueueStage::Streak);
    assert_eq!(streak.channel_ids, ["streak"]);
}
#[test]
fn planner_caps_automatic_streams_at_two() {
    let channels = [
        candidate("a", true, false, true, false),
        candidate("b", true, false, true, false),
        candidate("c", true, false, true, false),
        candidate("d", true, false, true, false),
        candidate("e", true, false, true, false),
    ];
    let plan = QueuePlanner::default().plan(channels.iter().copied(), 0);

    assert_eq!(plan.stage, QueueStage::Points);
    assert_eq!(plan.channel_ids, ["a", "b"]);
}

#[test]
fn planner_reserves_manually_open_stream_slots() {
    let channels = [
        candidate("a", true, false, true, false),
        candidate("b", true, false, true, false),
        candidate("c", true, false, true, false),
        candidate("d", true, false, true, false),
    ];
    let plan = QueuePlanner::default().plan(channels.iter().copied(), 1);
    assert_eq!(plan.channel_ids, ["a"]);
}
#[test]
fn offline_channels_do_not_enter_the_queue() {
    let channels = [candidate("offline", false, true, true, false)];
    let plan = QueuePlanner::default().plan(channels.iter().copied(), 0);

    assert_eq!(plan.stage, QueueStage::Idle);
    assert!(plan.channel_ids.is_empty());
}
#[test]
fn stream_turn_requires_playing_video_chat_and_capacity() {
    let mut timer = StreamTurnTimer::default();
    let without_capacity = TurnEligibility {
        player_playing: true,
        chat_ready: true,
        capacity_available: false,
    };

    assert!(!timer.observe(without_capacity, Duration::ZERO));
    assert!(!timer.observe(without_capacity, Duration::from_secs(1200)));
    assert_eq!(
        timer.elapsed_eligible(Duration::from_secs(1200)),
        Duration::ZERO
    );

    let eligible = TurnEligibility {
        capacity_available: true,
        ..without_capacity
    };
    assert!(!timer.observe(eligible, Duration::from_secs(1200)));
    assert!(timer.observe(eligible, Duration::from_secs(1200) + STREAM_TURN_DURATION));
    assert!(!timer.observe(eligible, Duration::from_secs(4000)));
}

#[test]
fn stream_turn_pauses_when_a_prerequisite_fails_and_resumes() {
    let eligible = TurnEligibility {
        player_playing: true,
        chat_ready: true,
        capacity_available: true,
    };
    let disconnected = TurnEligibility {
        chat_ready: false,
        ..eligible
    };
    let mut timer = StreamTurnTimer::default();

    assert!(!timer.observe(eligible, Duration::ZERO));
    assert!(!timer.observe(disconnected, Duration::from_secs(5 * 60)));
    assert_eq!(
        timer.elapsed_eligible(Duration::from_secs(5 * 60)),
        Duration::from_secs(5 * 60)
    );
    assert!(!timer.observe(eligible, Duration::from_secs(60 * 60)));
    assert!(timer.observe(
        eligible,
        Duration::from_secs(60 * 60) + STREAM_TURN_DURATION - Duration::from_secs(5 * 60)
    ));
}
#[test]
fn pausing_rotation_preserves_progress_without_counting_paused_time() {
    let eligible = TurnEligibility {
        player_playing: true,
        chat_ready: true,
        capacity_available: true,
    };
    let session = StreamSession {
        channel_id: "a".to_owned(),
        eligibility: eligible,
    };
    let mut rotation = StreamRotation::default();

    assert!(rotation
        .observe_active_sessions(std::slice::from_ref(&session), Duration::ZERO)
        .unwrap()
        .is_empty());
    rotation.pause_active_sessions(Duration::from_secs(5 * 60));
    assert_eq!(
        rotation.elapsed_eligible("a", Duration::from_secs(60 * 60)),
        Some(Duration::from_secs(5 * 60))
    );
    assert!(rotation
        .observe_active_sessions(&[session], Duration::from_secs(60 * 60))
        .unwrap()
        .is_empty());
    assert_eq!(
        rotation.observe_active_sessions(
            &[StreamSession {
                channel_id: "a".to_owned(),
                eligibility: eligible,
            }],
            Duration::from_secs(71 * 60),
        ),
        Ok(vec!["a".to_owned()])
    );
}

#[test]
fn pausing_at_turn_boundary_preserves_completion_for_resume() {
    let session = StreamSession {
        channel_id: "a".to_owned(),
        eligibility: TurnEligibility {
            player_playing: true,
            chat_ready: true,
            capacity_available: true,
        },
    };
    let mut rotation = StreamRotation::default();
    assert!(rotation
        .observe_active_sessions(std::slice::from_ref(&session), Duration::ZERO)
        .unwrap()
        .is_empty());

    rotation.pause_active_sessions(STREAM_TURN_DURATION);
    assert_eq!(
        rotation.observe_active_sessions(&[session], STREAM_TURN_DURATION),
        Ok(vec!["a".to_owned()])
    );
}
#[test]
fn completed_stream_turn_can_start_another_rotation() {
    let session = StreamSession {
        channel_id: "a".to_owned(),
        eligibility: TurnEligibility {
            player_playing: true,
            chat_ready: true,
            capacity_available: true,
        },
    };
    let mut rotation = StreamRotation::default();

    assert!(rotation
        .observe_active_sessions(std::slice::from_ref(&session), Duration::ZERO)
        .unwrap()
        .is_empty());
    assert_eq!(
        rotation.observe_active_sessions(&[session.clone()], STREAM_TURN_DURATION),
        Ok(vec!["a".to_owned()])
    );
    rotation.reset_completed_turn("a");
    assert!(rotation
        .observe_active_sessions(&[session.clone()], STREAM_TURN_DURATION)
        .unwrap()
        .is_empty());
    assert_eq!(
        rotation.observe_active_sessions(&[session], STREAM_TURN_DURATION * 2,),
        Ok(vec!["a".to_owned()])
    );
}

#[test]
fn stream_rotation_forgets_closed_sessions_and_bounds_tracking() {
    let eligible = TurnEligibility {
        player_playing: true,
        chat_ready: true,
        capacity_available: true,
    };
    let mut rotation = StreamRotation::default();
    let first_session = [StreamSession {
        channel_id: "a".to_owned(),
        eligibility: eligible,
    }];

    assert!(rotation
        .observe_active_sessions(&first_session, Duration::ZERO)
        .unwrap()
        .is_empty());
    rotation
        .observe_active_sessions(&[], Duration::from_secs(5 * 60))
        .unwrap();
    assert_eq!(
        rotation.elapsed_eligible("a", Duration::from_secs(5 * 60)),
        None
    );

    let too_many = (0..=MAX_TRACKED_STREAM_SESSIONS)
        .map(|index| StreamSession {
            channel_id: index.to_string(),
            eligibility: eligible,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        rotation.observe_active_sessions(&too_many, Duration::from_secs(10 * 60)),
        Err(StreamRotationError::TooManySessions {
            limit: MAX_TRACKED_STREAM_SESSIONS,
            actual: MAX_TRACKED_STREAM_SESSIONS + 1,
        })
    );
}

#[test]
fn stream_rotation_rejects_duplicate_active_channel_ids() {
    let session = StreamSession {
        channel_id: "duplicate".to_owned(),
        eligibility: TurnEligibility::default(),
    };
    let sessions = [session.clone(), session];

    assert_eq!(
        StreamRotation::default().observe_active_sessions(&sessions, Duration::ZERO),
        Err(StreamRotationError::DuplicateChannel)
    );
}
