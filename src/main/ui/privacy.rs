use super::super::activity_log::ActivityLogEntry;
#[cfg(test)]
use super::super::activity_log::ActivityLogKind;
#[cfg(test)]
use super::super::Dashboard;
use super::super::FollowedChannel;
use eframe::egui;

fn display_streamer_name(hide_sensitive_info: bool, name: &str) -> &str {
    if hide_sensitive_info {
        "Hidden streamer"
    } else {
        name
    }
}

pub(super) fn activity_log_display_values(
    entry: &ActivityLogEntry,
    hide_sensitive_info: bool,
) -> (&str, &str) {
    (
        display_streamer_name(hide_sensitive_info, &entry.streamer),
        if hide_sensitive_info {
            entry.kind.label()
        } else {
            entry.event.as_str()
        },
    )
}

pub(super) fn browser_profile_display_name(
    hide_sensitive_info: bool,
    index: usize,
    profile_name: &str,
) -> std::borrow::Cow<'_, str> {
    if hide_sensitive_info {
        std::borrow::Cow::Owned(format!("Browser profile {}", index + 1))
    } else {
        std::borrow::Cow::Borrowed(profile_name)
    }
}

pub(super) fn channel_selection_counts(
    channels: &[FollowedChannel],
    hide_sensitive_info: bool,
) -> Option<(usize, usize, usize)> {
    if hide_sensitive_info {
        return None;
    }

    Some((
        channels
            .iter()
            .filter(|channel| channel.selection.is_selected())
            .count(),
        channels
            .iter()
            .filter(|channel| channel.selection.streak)
            .count(),
        channels
            .iter()
            .filter(|channel| channel.selection.points)
            .count(),
    ))
}

pub(super) fn daily_activity_summary_contents(
    ui: &mut egui::Ui,
    activity_log: &std::collections::VecDeque<ActivityLogEntry>,
    hide_sensitive_info: bool,
) {
    if hide_sensitive_info {
        ui.label("Daily activity summary details hidden during screenshot privacy.");
        return;
    }

    let summaries = super::super::activity_log::daily_activity_summaries(activity_log);
    if summaries.is_empty() {
        ui.label("No retained activity to summarize.");
        return;
    }
    egui::ScrollArea::vertical()
        .max_height(100.0)
        .show(ui, |ui| {
            for summary in summaries {
                ui.label(format!(
                    "{} — {} streak completions, {} points-earned events, {} points-spent events, {} bonus notices, {} points turns, {} streak turns, {} stream switches, {} stream openings",
                    summary.date,
                    summary.streaks_completed,
                    summary.points_earned_events,
                    summary.points_spent_events,
                    summary.bonuses_available,
                    summary.points_turns_completed,
                    summary.streak_turns_ended,
                    summary.stream_switches,
                    summary.streams_opened,
                ));
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[test]
    fn screenshot_privacy_hides_streamer_and_freeform_activity_text() {
        let entry = ActivityLogEntry {
            time: "2026-10-27 12:00:00".to_owned(),
            streamer: "SensitiveStreamer".to_owned(),
            event: "Opening SensitiveStreamer.".to_owned(),
            kind: ActivityLogKind::StreamOpened,
        };

        assert_eq!(
            activity_log_display_values(&entry, true),
            ("Hidden streamer", entry.kind.label())
        );
        assert_eq!(
            activity_log_display_values(&entry, false),
            ("SensitiveStreamer", "Opening SensitiveStreamer.")
        );
    }

    #[test]
    fn activity_panel_renders_redacted_names_events_and_searches() {
        let activity_log = VecDeque::from([ActivityLogEntry {
            time: "2026-10-27 12:00:00".to_owned(),
            streamer: "SensitiveStreamer".to_owned(),
            event: "Opening SensitiveStreamer.".to_owned(),
            kind: ActivityLogKind::StreamOpened,
        }]);
        let mut event_filter = None;
        let mut search = "SensitiveStreamer".to_owned();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            ..Default::default()
        };
        let output = egui::Context::default().run(input, |context| {
            egui::CentralPanel::default().show(context, |ui| {
                Dashboard::activity_log_panel(
                    ui,
                    true,
                    &activity_log,
                    &mut event_filter,
                    &mut search,
                );
            });
        });
        let rendered = format!("{:?}", output.shapes);

        assert!(rendered.contains("Hidden streamer"));
        assert!(!rendered.contains("SensitiveStreamer"));
        assert!(!rendered.contains("Opening SensitiveStreamer."));
        assert!(!rendered.contains("2026-10-27"));
        assert!(!rendered.contains("Date/time"));
    }

    #[test]
    fn screenshot_privacy_hides_channel_totals() {
        let mut streak_channel = FollowedChannel::new("streak", "Streamer A");
        streak_channel.selection.streak = true;
        let mut points_channel = FollowedChannel::new("points", "Streamer B");
        points_channel.selection.points = true;
        let channels = [streak_channel, points_channel];

        assert_eq!(channel_selection_counts(&channels, false), Some((2, 1, 1)));
        assert_eq!(channel_selection_counts(&channels, true), None);
    }

    #[test]
    fn screenshot_privacy_aliases_browser_profile_names() {
        assert_eq!(
            browser_profile_display_name(true, 0, "Personal"),
            "Browser profile 1"
        );
        assert_eq!(
            browser_profile_display_name(false, 0, "Personal"),
            "Personal"
        );
    }

    fn render_daily_summaries(hide_sensitive_info: bool) -> String {
        let activity_log = VecDeque::from([ActivityLogEntry {
            time: "2026-10-27 12:00:00".to_owned(),
            streamer: "SensitiveStreamer".to_owned(),
            event: "Opening SensitiveStreamer.".to_owned(),
            kind: ActivityLogKind::StreamOpened,
        }]);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            ..Default::default()
        };
        let output = egui::Context::default().run(input, |context| {
            egui::CentralPanel::default().show(context, |ui| {
                daily_activity_summary_contents(ui, &activity_log, hide_sensitive_info);
            });
        });
        format!("{:?}", output.shapes)
    }

    #[test]
    fn screenshot_privacy_hides_daily_summary_dates_and_counts() {
        let hidden = render_daily_summaries(true);
        assert!(hidden.contains("Daily activity summary details hidden during screenshot privacy."));
        assert!(!hidden.contains("2026-10-27"));
        assert!(!hidden.contains("1 stream openings"));

        let visible = render_daily_summaries(false);
        assert!(visible.contains("2026-10-27"));
        assert!(visible.contains("1 stream openings"));
    }
}
