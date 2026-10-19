use chrono::Local;
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ActivityLogKind {
    WatchStreak,
    PointsEarned,
    PointsSpent,
    BonusAvailable,
    PointsTurnCompleted,
    StreakTurnEnded,
    StreamSwitch,
    StreamOpened,
}

impl ActivityLogKind {
    pub(super) const ALL: [Self; 8] = [
        Self::WatchStreak,
        Self::PointsEarned,
        Self::PointsSpent,
        Self::BonusAvailable,
        Self::PointsTurnCompleted,
        Self::StreakTurnEnded,
        Self::StreamSwitch,
        Self::StreamOpened,
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
            Self::StreamOpened => "Stream opened",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) struct ActivityLogEntry {
    pub(super) time: String,
    pub(super) streamer: String,
    pub(super) event: String,
    pub(super) kind: ActivityLogKind,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct DailyActivitySummary {
    pub(super) date: String,
    pub(super) streaks_completed: usize,
    pub(super) points_earned_events: usize,
    pub(super) points_spent_events: usize,
    pub(super) bonuses_available: usize,
    pub(super) points_turns_completed: usize,
    pub(super) streak_turns_ended: usize,
    pub(super) stream_switches: usize,
    pub(super) streams_opened: usize,
}

pub(super) fn daily_activity_summaries(
    log: &VecDeque<ActivityLogEntry>,
) -> Vec<DailyActivitySummary> {
    let mut summary_indices = HashMap::<&str, usize>::new();
    let mut summaries = Vec::<DailyActivitySummary>::new();
    for entry in log {
        let date = entry
            .time
            .split_once(' ')
            .map_or(entry.time.as_str(), |(date, _)| date);
        let index = if let Some(index) = summary_indices.get(date) {
            *index
        } else {
            let index = summaries.len();
            summaries.push(DailyActivitySummary {
                date: date.to_owned(),
                ..DailyActivitySummary::default()
            });
            summary_indices.insert(date, index);
            index
        };
        let summary = &mut summaries[index];
        match &entry.kind {
            ActivityLogKind::WatchStreak => summary.streaks_completed += 1,
            ActivityLogKind::PointsEarned => summary.points_earned_events += 1,
            ActivityLogKind::PointsSpent => summary.points_spent_events += 1,
            ActivityLogKind::BonusAvailable => summary.bonuses_available += 1,
            ActivityLogKind::PointsTurnCompleted => summary.points_turns_completed += 1,
            ActivityLogKind::StreakTurnEnded => summary.streak_turns_ended += 1,
            ActivityLogKind::StreamSwitch => summary.stream_switches += 1,
            ActivityLogKind::StreamOpened => summary.streams_opened += 1,
        }
    }
    summaries
}

pub(super) const MAX_ACTIVITY_LOG_ENTRIES: usize = 500;

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
            time: Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
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
            time: "2026-10-19 21:15:09".to_owned(),
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
                    time: format!("2026-10-18 00:00:{:02}", index % 60),
                    streamer: format!("streamer-{index}"),
                    event: format!("event-{index}"),
                    kind: ActivityLogKind::PointsEarned,
                },
            );
        }

        assert_eq!(log.len(), MAX_ACTIVITY_LOG_ENTRIES);
        assert_eq!(log.front().unwrap().event, "event-500");
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

#[test]
fn daily_activity_summaries_group_events_by_date() {
    let log = VecDeque::from([
        ActivityLogEntry {
            time: "2026-10-19 22:00:00".to_owned(),
            streamer: "Streamer One".to_owned(),
            event: "Opening Streamer One.".to_owned(),
            kind: ActivityLogKind::StreamOpened,
        },
        ActivityLogEntry {
            time: "2026-10-19 21:00:00".to_owned(),
            streamer: "Streamer Two".to_owned(),
            event: "Watch Streak completed.".to_owned(),
            kind: ActivityLogKind::WatchStreak,
        },
        ActivityLogEntry {
            time: "2026-10-18 20:00:00".to_owned(),
            streamer: "Streamer Three".to_owned(),
            event: "Points spent.".to_owned(),
            kind: ActivityLogKind::PointsSpent,
        },
    ]);

    let summaries = daily_activity_summaries(&log);
    assert_eq!(summaries.len(), 2);
    assert_eq!(summaries[0].date, "2026-10-19");
    assert_eq!(summaries[0].streams_opened, 1);
    assert_eq!(summaries[0].streaks_completed, 1);
    assert_eq!(summaries[1].date, "2026-10-18");
    assert_eq!(summaries[1].points_spent_events, 1);
}

#[test]
fn activity_log_entries_round_trip_for_local_persistence() {
    let log = VecDeque::from([ActivityLogEntry {
        time: "2026-10-19 21:15:09".to_owned(),
        streamer: "Streamer One".to_owned(),
        event: "Opening Streamer One.".to_owned(),
        kind: ActivityLogKind::StreamOpened,
    }]);

    let serialized = serde_json::to_string(&log).unwrap();
    let restored: VecDeque<ActivityLogEntry> = serde_json::from_str(&serialized).unwrap();
    assert_eq!(restored, log);
}
