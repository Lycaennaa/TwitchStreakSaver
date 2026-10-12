use crate::model::{ChannelSelection, SchedulingMode};
use std::collections::{HashMap, HashSet};
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
    last_streak_turn_finished: Option<String>,
    last_points_turn_finished: Option<String>,
    deferred_streak_channels: HashMap<String, Duration>,
}

impl QueuePlanner {
    pub fn record_streak_completed(&mut self, channel_id: impl Into<String>) {
        let channel_id = channel_id.into();
        self.last_streak_turn_finished = Some(channel_id.clone());
        self.deferred_streak_channels.remove(&channel_id);
    }

    pub fn record_streak_turn_timed_out(&mut self, channel_id: impl Into<String>, now: Duration) {
        let channel_id = channel_id.into();
        self.last_streak_turn_finished = Some(channel_id.clone());
        self.last_points_turn_finished = Some(channel_id.clone());
        self.deferred_streak_channels
            .insert(channel_id, now.saturating_add(STREAM_TURN_DURATION));
    }

    pub fn record_points_turn_finished(&mut self, channel_id: impl Into<String>) {
        self.last_points_turn_finished = Some(channel_id.into());
        self.deferred_streak_channels.clear();
    }

    pub fn retain_live_channels(&mut self, live_channel_ids: &HashSet<String>) {
        self.deferred_streak_channels
            .retain(|channel_id, _| live_channel_ids.contains(channel_id));
    }

    pub fn clear_streak_turn_deferrals(&mut self) {
        self.deferred_streak_channels.clear();
    }
    pub fn expire_streak_turn_deferrals(&mut self, now: Duration) {
        self.deferred_streak_channels
            .retain(|_, expires_at| *expires_at > now);
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
        let mut deferred_streak_candidates = CandidateRotation::default();
        let mut points_candidates = CandidateRotation::default();
        let mut has_streak_candidates = false;
        let mut has_deferred_streak_candidates = false;
        let mut has_points_candidates = false;

        for candidate in candidates {
            if is_eligible(QueueStage::Streak, &candidate) {
                if self
                    .deferred_streak_channels
                    .contains_key(candidate.channel_id)
                {
                    has_deferred_streak_candidates = true;
                    deferred_streak_candidates.observe(
                        candidate.channel_id,
                        self.last_streak_turn_finished.as_deref(),
                        app_capacity,
                    );
                } else {
                    has_streak_candidates = true;
                    streak_candidates.observe(
                        candidate.channel_id,
                        self.last_streak_turn_finished.as_deref(),
                        app_capacity,
                    );
                }
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
        } else if has_deferred_streak_candidates {
            QueuePlan {
                stage: QueueStage::Streak,
                channel_ids: deferred_streak_candidates.into_channel_ids(app_capacity),
            }
        } else {
            QueuePlan {
                stage: QueueStage::Idle,
                channel_ids: Vec::new(),
            }
        }
    }
}
pub const STREAM_TURN_DURATION: Duration = Duration::from_secs(16 * 60);
pub const MAX_TRACKED_STREAM_SESSIONS: usize = 4;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TurnEligibility {
    pub player_playing: bool,
    pub chat_ready: bool,
    pub capacity_available: bool,
}

impl TurnEligibility {
    pub const fn is_eligible(self) -> bool {
        self.player_playing && self.chat_ready && self.capacity_available
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamSession {
    pub channel_id: String,
    pub eligibility: TurnEligibility,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamRotationError {
    TooManySessions { limit: usize, actual: usize },
    DuplicateChannel,
}

#[derive(Default)]
pub struct StreamRotation {
    turns: Vec<TrackedStreamTurn>,
}

#[derive(Default)]
struct TrackedStreamTurn {
    channel_id: String,
    timer: StreamTurnTimer,
}

#[derive(Default)]
pub struct StreamTurnTimer {
    eligible_duration: Duration,
    eligible_since: Option<Duration>,
    complete: bool,
}

impl StreamTurnTimer {
    pub fn observe(&mut self, eligibility: TurnEligibility, now: Duration) -> bool {
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

        if self.elapsed_eligible(now) >= STREAM_TURN_DURATION {
            self.eligible_duration = STREAM_TURN_DURATION;
            self.eligible_since = None;
            self.complete = true;
            true
        } else {
            false
        }
    }

    fn pause(&mut self, now: Duration) {
        if let Some(eligible_since) = self.eligible_since.take() {
            self.eligible_duration = self
                .eligible_duration
                .saturating_add(now.saturating_sub(eligible_since));
        }
    }

    pub fn elapsed_eligible(&self, now: Duration) -> Duration {
        self.eligible_duration
            .saturating_add(
                self.eligible_since
                    .map_or(Duration::ZERO, |start| now.saturating_sub(start)),
            )
            .min(STREAM_TURN_DURATION)
    }
}

impl StreamRotation {
    pub fn observe_active_sessions(
        &mut self,
        sessions: &[StreamSession],
        now: Duration,
    ) -> Result<Vec<String>, StreamRotationError> {
        if sessions.len() > MAX_TRACKED_STREAM_SESSIONS {
            return Err(StreamRotationError::TooManySessions {
                limit: MAX_TRACKED_STREAM_SESSIONS,
                actual: sessions.len(),
            });
        }
        for (index, session) in sessions.iter().enumerate() {
            if sessions[..index]
                .iter()
                .any(|earlier| earlier.channel_id == session.channel_id)
            {
                return Err(StreamRotationError::DuplicateChannel);
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
                self.turns.push(TrackedStreamTurn {
                    channel_id: session.channel_id.clone(),
                    timer: StreamTurnTimer::default(),
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
            turn.timer.pause(now);
        }
    }
    pub fn reset_completed_turn(&mut self, channel_id: &str) {
        if let Some(turn) = self
            .turns
            .iter_mut()
            .find(|turn| turn.channel_id == channel_id)
        {
            if turn.timer.complete {
                turn.timer = StreamTurnTimer::default();
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
