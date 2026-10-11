use crate::model::{ChannelSelection, SchedulingMode};
use std::time::Duration;
const MAX_APP_STREAMS: usize = SchedulingMode::OpenAll.app_stream_limit();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueueCandidate<'a> {
    pub channel_id: &'a str,
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

    pub fn plan<'a>(
        &self,
        candidates: impl Iterator<Item = QueueCandidate<'a>>,
        mode: SchedulingMode,
        manual_active_streams: usize,
    ) -> QueuePlan {
        let app_capacity = mode
            .app_stream_limit()
            .saturating_sub(manual_active_streams);
        let mut streak_candidates = CandidateRotation::default();
        let mut points_candidates = CandidateRotation::default();
        let mut has_streak_candidates = false;
        let mut has_points_candidates = false;

        for candidate in candidates {
            if is_eligible(QueueStage::Streak, &candidate) {
                has_streak_candidates = true;
                streak_candidates.observe(
                    candidate.channel_id,
                    self.last_streak_completed.as_deref(),
                    app_capacity,
                );
            }
            if is_eligible(QueueStage::Points, &candidate) {
                has_points_candidates = true;
                points_candidates.observe(
                    candidate.channel_id,
                    self.last_points_turn_finished.as_deref(),
                    app_capacity,
                );
            }
        }

        if has_streak_candidates {
            QueuePlan {
                stage: QueueStage::Streak,
                channel_ids: streak_candidates.into_channel_ids(app_capacity),
            }
        } else if has_points_candidates {
            QueuePlan {
                stage: QueueStage::Points,
                channel_ids: points_candidates.into_channel_ids(app_capacity),
            }
        } else {
            QueuePlan {
                stage: QueueStage::Idle,
                channel_ids: Vec::new(),
            }
        }
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

fn is_eligible(stage: QueueStage, candidate: &QueueCandidate<'_>) -> bool {
    candidate.is_live
        && match stage {
            QueueStage::Streak => {
                candidate.selection.streak && !candidate.streak_completed_for_live
            }
            QueueStage::Points => candidate.selection.points,
            QueueStage::Idle => false,
        }
}

#[derive(Default)]
struct CandidateRotation<'a> {
    before_anchor: [Option<&'a str>; MAX_APP_STREAMS],
    before_count: usize,
    after_anchor: [Option<&'a str>; MAX_APP_STREAMS],
    after_count: usize,
    anchor_found: bool,
}

impl<'a> CandidateRotation<'a> {
    fn observe(&mut self, channel_id: &'a str, anchor: Option<&str>, capacity: usize) {
        if self.anchor_found {
            if self.after_count < capacity {
                self.after_anchor[self.after_count] = Some(channel_id);
                self.after_count += 1;
            }
            return;
        }
        if self.before_count < capacity {
            self.before_anchor[self.before_count] = Some(channel_id);
            self.before_count += 1;
        }
        self.anchor_found = Some(channel_id) == anchor;
    }

    fn into_channel_ids(self, capacity: usize) -> Vec<String> {
        let Self {
            before_anchor,
            before_count,
            after_anchor,
            after_count,
            anchor_found,
        } = self;
        let mut channel_ids = Vec::with_capacity(capacity);
        for channel_id in after_anchor.into_iter().take(after_count).flatten() {
            channel_ids.push(channel_id.to_owned());
        }
        let before_count = if anchor_found {
            before_count.min(capacity.saturating_sub(after_count))
        } else {
            before_count
        };
        for channel_id in before_anchor.into_iter().take(before_count).flatten() {
            channel_ids.push(channel_id.to_owned());
        }
        channel_ids
    }
}

#[cfg(test)]
mod tests;
