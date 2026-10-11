use super::{
    PointsEligibility, PointsRotation, PointsRotationError, PointsSession, PointsTurnTimer,
    QueueCandidate, QueuePlanner, QueueStage, MAX_TRACKED_POINTS_SESSIONS, POINTS_TURN_DURATION,
};
use crate::model::{ChannelSelection, SchedulingMode};
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
    let plan = planner.plan(channels.iter().copied(), SchedulingMode::Earn, 1);
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
    let plan = QueuePlanner::default().plan(
        std::iter::from_fn(|| source.next()),
        SchedulingMode::Earn,
        0,
    );

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
    let plan = planner.plan(channels.iter().copied(), SchedulingMode::Earn, 0);

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

    let first = planner.plan(channels.iter().copied(), SchedulingMode::Earn, 0);
    assert_eq!(first.channel_ids, ["a", "b"]);

    planner.record_points_turn_finished("a");
    let next = planner.plan(channels.iter().copied(), SchedulingMode::Earn, 0);
    assert_eq!(next.channel_ids, ["b", "c"]);
}

#[test]
fn open_all_respects_capacity() {
    let channels = [
        candidate("a", true, false, true, false),
        candidate("b", true, false, true, false),
        candidate("c", true, false, true, false),
        candidate("d", true, false, true, false),
        candidate("e", true, false, true, false),
    ];
    let plan = QueuePlanner::default().plan(channels.iter().copied(), SchedulingMode::OpenAll, 0);

    assert_eq!(plan.stage, QueueStage::Points);
    assert_eq!(plan.channel_ids, ["a", "b", "c", "d"]);
}

#[test]
fn open_all_reserves_capacity_for_manually_open_streams() {
    let channels = [
        candidate("a", true, false, true, false),
        candidate("b", true, false, true, false),
        candidate("c", true, false, true, false),
        candidate("d", true, false, true, false),
    ];
    let plan = QueuePlanner::default().plan(channels.iter().copied(), SchedulingMode::OpenAll, 1);
    assert_eq!(plan.channel_ids, ["a", "b", "c"]);
}
#[test]
fn offline_channels_do_not_enter_the_queue() {
    let channels = [candidate("offline", false, true, true, false)];
    let plan = QueuePlanner::default().plan(channels.iter().copied(), SchedulingMode::Earn, 0);

    assert_eq!(plan.stage, QueueStage::Idle);
    assert!(plan.channel_ids.is_empty());
}
#[test]
fn points_turn_requires_playing_video_chat_and_capacity() {
    let mut timer = PointsTurnTimer::default();
    let without_capacity = PointsEligibility {
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

    let eligible = PointsEligibility {
        capacity_available: true,
        ..without_capacity
    };
    assert!(!timer.observe(eligible, Duration::from_secs(1200)));
    assert!(timer.observe(eligible, Duration::from_secs(1200) + POINTS_TURN_DURATION));
    assert!(!timer.observe(eligible, Duration::from_secs(4000)));
}

#[test]
fn points_turn_pauses_when_a_prerequisite_fails_and_resumes() {
    let eligible = PointsEligibility {
        player_playing: true,
        chat_ready: true,
        capacity_available: true,
    };
    let disconnected = PointsEligibility {
        chat_ready: false,
        ..eligible
    };
    let mut timer = PointsTurnTimer::default();

    assert!(!timer.observe(eligible, Duration::ZERO));
    assert!(!timer.observe(disconnected, Duration::from_secs(5 * 60)));
    assert_eq!(
        timer.elapsed_eligible(Duration::from_secs(5 * 60)),
        Duration::from_secs(5 * 60)
    );
    assert!(!timer.observe(eligible, Duration::from_secs(60 * 60)));
    assert!(timer.observe(
        eligible,
        Duration::from_secs(60 * 60) + POINTS_TURN_DURATION - Duration::from_secs(5 * 60)
    ));
}
#[test]
fn pausing_rotation_preserves_progress_without_counting_paused_time() {
    let eligible = PointsEligibility {
        player_playing: true,
        chat_ready: true,
        capacity_available: true,
    };
    let session = PointsSession {
        channel_id: "a".to_owned(),
        eligibility: eligible,
    };
    let mut rotation = PointsRotation::default();

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
            &[PointsSession {
                channel_id: "a".to_owned(),
                eligibility: eligible,
            }],
            Duration::from_secs(71 * 60),
        ),
        Ok(vec!["a".to_owned()])
    );
}

#[test]
fn completed_points_turn_can_start_another_rotation() {
    let session = PointsSession {
        channel_id: "a".to_owned(),
        eligibility: PointsEligibility {
            player_playing: true,
            chat_ready: true,
            capacity_available: true,
        },
    };
    let mut rotation = PointsRotation::default();

    assert!(rotation
        .observe_active_sessions(std::slice::from_ref(&session), Duration::ZERO)
        .unwrap()
        .is_empty());
    assert_eq!(
        rotation.observe_active_sessions(&[session.clone()], POINTS_TURN_DURATION),
        Ok(vec!["a".to_owned()])
    );
    rotation.reset_completed_turn("a");
    assert!(rotation
        .observe_active_sessions(&[session.clone()], POINTS_TURN_DURATION)
        .unwrap()
        .is_empty());
    assert_eq!(
        rotation.observe_active_sessions(&[session], POINTS_TURN_DURATION * 2,),
        Ok(vec!["a".to_owned()])
    );
}

#[test]
fn points_rotation_forgets_closed_sessions_and_bounds_tracking() {
    let eligible = PointsEligibility {
        player_playing: true,
        chat_ready: true,
        capacity_available: true,
    };
    let mut rotation = PointsRotation::default();
    let first_session = [PointsSession {
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

    let too_many = (0..=MAX_TRACKED_POINTS_SESSIONS)
        .map(|index| PointsSession {
            channel_id: index.to_string(),
            eligibility: eligible,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        rotation.observe_active_sessions(&too_many, Duration::from_secs(10 * 60)),
        Err(PointsRotationError::TooManySessions {
            limit: MAX_TRACKED_POINTS_SESSIONS,
            actual: MAX_TRACKED_POINTS_SESSIONS + 1,
        })
    );
}

#[test]
fn points_rotation_rejects_duplicate_active_channel_ids() {
    let session = PointsSession {
        channel_id: "duplicate".to_owned(),
        eligibility: PointsEligibility::default(),
    };
    let sessions = [session.clone(), session];

    assert_eq!(
        PointsRotation::default().observe_active_sessions(&sessions, Duration::ZERO),
        Err(PointsRotationError::DuplicateChannel)
    );
}
