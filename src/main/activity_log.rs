use chrono::Local;
use std::collections::{HashSet, VecDeque};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ActivityLogKind {
    WatchStreak,
    PointsEarned,
    PointsSpent,
    BonusAvailable,
    PointsTurnCompleted,
    StreakTurnEnded,
    StreamSwitch,
}

impl ActivityLogKind {
    pub(super) const ALL: [Self; 7] = [
        Self::WatchStreak,
        Self::PointsEarned,
        Self::PointsSpent,
        Self::BonusAvailable,
        Self::PointsTurnCompleted,
        Self::StreakTurnEnded,
        Self::StreamSwitch,
    ];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::WatchStreak => "Watch Streak completed",
            Self::PointsEarned => "Points earned",
            Self::PointsSpent => "Points spent",
            Self::BonusAvailable => "Bonus available",
            Self::PointsTurnCompleted => "Points turn completed",
            Self::StreakTurnEnded => "Streak turn ended",
            Self::StreamSwitch => "Stream switch",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ActivityLogEntry {
    pub(super) time: String,
    pub(super) streamer: String,
    pub(super) event: String,
    pub(super) kind: ActivityLogKind,
}

const MAX_ACTIVITY_LOG_ENTRIES: usize = 200;

pub(super) fn push_activity_log(log: &mut VecDeque<ActivityLogEntry>, entry: ActivityLogEntry) {
    if log.len() >= MAX_ACTIVITY_LOG_ENTRIES {
        log.pop_back();
    }
    log.push_front(entry);
}

pub(super) fn record_activity(
    log: &mut VecDeque<ActivityLogEntry>,
    kind: ActivityLogKind,
    streamer: String,
    event: String,
) {
    push_activity_log(
        log,
        ActivityLogEntry {
            time: Local::now().format("%H:%M:%S").to_string(),
            streamer,
            event,
            kind,
        },
    );
}

pub(super) fn record_claim_available_once(
    available_claim_channels: &mut HashSet<String>,
    log: &mut VecDeque<ActivityLogEntry>,
    channel_id: String,
    streamer: String,
) {
    if available_claim_channels.insert(channel_id) {
        record_activity(
            log,
            ActivityLogKind::BonusAvailable,
            streamer,
            "Bonus available to claim manually on Twitch.".to_owned(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_log_keeps_timestamp_streamer_and_latest_events() {
        let mut log = VecDeque::new();
        let entry = ActivityLogEntry {
            time: "21:15:09".to_owned(),
            streamer: "First streamer".to_owned(),
            event: "Watch Streak completed; reward: 400 points.".to_owned(),
            kind: ActivityLogKind::WatchStreak,
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
                    kind: ActivityLogKind::PointsEarned,
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
        assert_eq!(log.front().unwrap().kind, ActivityLogKind::BonusAvailable);

        record_claim_available_once(
            &mut available,
            &mut log,
            "channel-2".to_owned(),
            "Streamer Two".to_owned(),
        );
        assert_eq!(log.len(), 2);
        assert_eq!(log.front().unwrap().streamer, "Streamer Two");
    }
}
