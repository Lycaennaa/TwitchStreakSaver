use crate::model::{ChannelSelection, SchedulingMode};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueCandidate {
    pub channel_id: String,
    pub is_live: bool,
    pub selection: ChannelSelection,
    pub streak_completed_for_live: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueStage {
    Streak,
    Points,
    Idle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueuePlan {
    pub stage: QueueStage,
    pub channel_ids: Vec<String>,
    pub waiting_count: usize,
    pub app_capacity: usize,
    pub over_capacity: bool,
}

#[derive(Default)]
pub struct QueuePlanner {
    last_streak_completed: Option<String>,
    last_points_turn_finished: Option<String>,
}

impl QueuePlanner {
    pub fn record_streak_completed(&mut self, channel_id: impl Into<String>) {
        self.last_streak_completed = Some(channel_id.into());
    }

    pub fn record_points_turn_finished(&mut self, channel_id: impl Into<String>) {
        self.last_points_turn_finished = Some(channel_id.into());
    }

    pub fn plan(
        &self,
        channels: &[QueueCandidate],
        mode: SchedulingMode,
        manual_active_streams: usize,
    ) -> QueuePlan {
        let app_capacity = match mode {
            SchedulingMode::Earn => 2usize.saturating_sub(manual_active_streams),
            SchedulingMode::OpenAll => mode
                .app_stream_limit()
                .saturating_sub(manual_active_streams),
        };

        let streak_candidates = channels
            .iter()
            .filter(|channel| {
                channel.is_live && channel.selection.streak && !channel.streak_completed_for_live
            })
            .collect::<Vec<_>>();
        if !streak_candidates.is_empty() {
            let candidates = rotate_after(streak_candidates, self.last_streak_completed.as_deref());
            return build_plan(
                QueueStage::Streak,
                candidates,
                app_capacity,
                manual_active_streams,
            );
        }

        let points_candidates = channels
            .iter()
            .filter(|channel| channel.is_live && channel.selection.points)
            .collect::<Vec<_>>();
        let candidates = rotate_after(points_candidates, self.last_points_turn_finished.as_deref());
        build_plan(
            if candidates.is_empty() {
                QueueStage::Idle
            } else {
                QueueStage::Points
            },
            candidates,
            app_capacity,
            manual_active_streams,
        )
    }
}
pub const POINTS_TURN_DURATION: Duration = Duration::from_secs(16 * 60);
pub const MAX_TRACKED_POINTS_SESSIONS: usize = 4;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PointsEligibility {
    pub player_playing: bool,
    pub chat_ready: bool,
    pub capacity_available: bool,
}

impl PointsEligibility {
    pub const fn is_eligible(self) -> bool {
        self.player_playing && self.chat_ready && self.capacity_available
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PointsSession {
    pub channel_id: String,
    pub eligibility: PointsEligibility,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointsRotationError {
    TooManySessions { limit: usize, actual: usize },
    DuplicateChannel,
}

#[derive(Default)]
pub struct PointsRotation {
    turns: Vec<TrackedPointsTurn>,
}

#[derive(Default)]
struct TrackedPointsTurn {
    channel_id: String,
    timer: PointsTurnTimer,
}

#[derive(Default)]
pub struct PointsTurnTimer {
    eligible_duration: Duration,
    eligible_since: Option<Duration>,
    complete: bool,
}

impl PointsTurnTimer {
    pub fn observe(&mut self, eligibility: PointsEligibility, now: Duration) -> bool {
        if self.complete {
            return false;
        }

        if eligibility.is_eligible() {
            self.eligible_since.get_or_insert(now);
        } else if let Some(eligible_since) = self.eligible_since.take() {
            self.eligible_duration = self
                .eligible_duration
                .saturating_add(now.saturating_sub(eligible_since));
        }

        if self.elapsed_eligible(now) >= POINTS_TURN_DURATION {
            self.eligible_duration = POINTS_TURN_DURATION;
            self.eligible_since = None;
            self.complete = true;
            true
        } else {
            false
        }
    }

    pub fn elapsed_eligible(&self, now: Duration) -> Duration {
        self.eligible_duration
            .saturating_add(
                self.eligible_since
                    .map_or(Duration::ZERO, |start| now.saturating_sub(start)),
            )
            .min(POINTS_TURN_DURATION)
    }
}

impl PointsRotation {
    pub fn observe_active_sessions(
        &mut self,
        sessions: &[PointsSession],
        now: Duration,
    ) -> Result<Vec<String>, PointsRotationError> {
        if sessions.len() > MAX_TRACKED_POINTS_SESSIONS {
            return Err(PointsRotationError::TooManySessions {
                limit: MAX_TRACKED_POINTS_SESSIONS,
                actual: sessions.len(),
            });
        }
        for (index, session) in sessions.iter().enumerate() {
            if sessions[..index]
                .iter()
                .any(|earlier| earlier.channel_id == session.channel_id)
            {
                return Err(PointsRotationError::DuplicateChannel);
            }
        }

        self.turns.retain(|turn| {
            sessions
                .iter()
                .any(|session| session.channel_id == turn.channel_id)
        });

        let mut completed = Vec::new();
        for session in sessions {
            let turn = if let Some(turn) = self
                .turns
                .iter_mut()
                .find(|turn| turn.channel_id == session.channel_id)
            {
                turn
            } else {
                self.turns.push(TrackedPointsTurn {
                    channel_id: session.channel_id.clone(),
                    timer: PointsTurnTimer::default(),
                });
                self.turns.last_mut().expect("turn was just inserted")
            };

            if turn.timer.observe(session.eligibility, now) {
                completed.push(session.channel_id.clone());
            }
        }

        Ok(completed)
    }

    pub fn pause_active_sessions(&mut self, now: Duration) {
        for turn in &mut self.turns {
            turn.timer.observe(PointsEligibility::default(), now);
        }
    }
    pub fn reset_completed_turn(&mut self, channel_id: &str) {
        if let Some(turn) = self
            .turns
            .iter_mut()
            .find(|turn| turn.channel_id == channel_id)
        {
            if turn.timer.complete {
                turn.timer = PointsTurnTimer::default();
            }
        }
    }

    pub fn elapsed_eligible(&self, channel_id: &str, now: Duration) -> Option<Duration> {
        self.turns
            .iter()
            .find(|turn| turn.channel_id == channel_id)
            .map(|turn| turn.timer.elapsed_eligible(now))
    }
}

fn rotate_after<'a>(
    mut candidates: Vec<&'a QueueCandidate>,
    last_channel_id: Option<&str>,
) -> Vec<&'a QueueCandidate> {
    if let Some(index) = last_channel_id.and_then(|last| {
        candidates
            .iter()
            .position(|candidate| candidate.channel_id == last)
    }) {
        let rotation = (index + 1) % candidates.len();
        candidates.rotate_left(rotation);
    }
    candidates
}

fn build_plan(
    stage: QueueStage,
    candidates: Vec<&QueueCandidate>,
    app_capacity: usize,
    manual_active_streams: usize,
) -> QueuePlan {
    let channel_ids = candidates
        .iter()
        .take(app_capacity)
        .map(|candidate| candidate.channel_id.clone())
        .collect::<Vec<_>>();
    let waiting_count = candidates.len().saturating_sub(channel_ids.len());

    QueuePlan {
        stage,
        over_capacity: manual_active_streams.saturating_add(channel_ids.len()) > 2,
        channel_ids,
        waiting_count,
        app_capacity,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PointsEligibility, PointsRotation, PointsRotationError, PointsSession, PointsTurnTimer,
        QueueCandidate, QueuePlanner, QueueStage, MAX_TRACKED_POINTS_SESSIONS,
        POINTS_TURN_DURATION,
    };
    use crate::model::{ChannelSelection, SchedulingMode};
    use std::time::Duration;

    fn candidate(
        channel_id: &str,
        is_live: bool,
        streak: bool,
        points: bool,
        streak_completed_for_live: bool,
    ) -> QueueCandidate {
        QueueCandidate {
            channel_id: channel_id.to_owned(),
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
        let plan = planner.plan(&channels, SchedulingMode::Earn, 1);
        assert_eq!(plan.stage, QueueStage::Streak);
        assert_eq!(plan.channel_ids, ["streak-a"]);
        assert_eq!(plan.waiting_count, 1);
        assert_eq!(plan.app_capacity, 1);
        assert!(!plan.over_capacity);
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
        let plan = planner.plan(&channels, SchedulingMode::Earn, 0);

        assert_eq!(plan.stage, QueueStage::Points);
        assert_eq!(plan.channel_ids, ["both-a", "both-b"]);
        assert_eq!(plan.waiting_count, 1);
    }

    #[test]
    fn points_turn_advances_round_robin_from_the_completed_channel() {
        let channels = [
            candidate("a", true, false, true, false),
            candidate("b", true, false, true, false),
            candidate("c", true, false, true, false),
        ];
        let mut planner = QueuePlanner::default();

        let first = planner.plan(&channels, SchedulingMode::Earn, 0);
        assert_eq!(first.channel_ids, ["a", "b"]);

        planner.record_points_turn_finished("a");
        let next = planner.plan(&channels, SchedulingMode::Earn, 0);
        assert_eq!(next.channel_ids, ["b", "c"]);
    }

    #[test]
    fn open_all_respects_capacity_and_reports_over_capacity() {
        let channels = [
            candidate("a", true, false, true, false),
            candidate("b", true, false, true, false),
            candidate("c", true, false, true, false),
            candidate("d", true, false, true, false),
            candidate("e", true, false, true, false),
        ];
        let plan = QueuePlanner::default().plan(&channels, SchedulingMode::OpenAll, 0);

        assert_eq!(plan.stage, QueueStage::Points);
        assert_eq!(plan.channel_ids, ["a", "b", "c", "d"]);
        assert_eq!(plan.waiting_count, 1);
        assert!(plan.over_capacity);
    }

    #[test]
    fn open_all_reserves_capacity_for_manually_open_streams() {
        let channels = [
            candidate("a", true, false, true, false),
            candidate("b", true, false, true, false),
            candidate("c", true, false, true, false),
            candidate("d", true, false, true, false),
        ];
        let plan = QueuePlanner::default().plan(&channels, SchedulingMode::OpenAll, 1);
        assert_eq!(plan.app_capacity, 3);
        assert_eq!(plan.channel_ids, ["a", "b", "c"]);
    }
    #[test]
    fn offline_channels_do_not_enter_the_queue() {
        let channels = [candidate("offline", false, true, true, false)];
        let plan = QueuePlanner::default().plan(&channels, SchedulingMode::Earn, 0);

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
}
