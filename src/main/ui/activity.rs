use super::*;
pub(super) fn activity_log_matches(
    entry: &ActivityLogEntry,
    event_filter: Option<ActivityLogKind>,
    search: &str,
) -> bool {
    let query = search.trim();
    event_filter.map_or(true, |kind| entry.kind == kind)
        && (query.is_empty()
            || fuzzy_contains(&entry.time, query)
            || fuzzy_contains(&entry.streamer, query)
            || fuzzy_contains(&entry.event, query)
            || fuzzy_contains(entry.kind.label(), query))
}
impl Dashboard {
    pub(super) fn activity_log_panel(
        ui: &mut egui::Ui,
        hide_sensitive_info: bool,
        activity_log: &std::collections::VecDeque<ActivityLogEntry>,
        event_filter: &mut Option<ActivityLogKind>,
        search: &mut String,
    ) -> egui::Response {
        ui.group(|ui| {
            ui.horizontal_wrapped(|ui| {
                section_heading(ui, "Activity log");
                ui.add(
                    egui::TextEdit::singleline(search)
                        .password(hide_sensitive_info)
                        .hint_text("Search event, streamer, or time")
                        .desired_width(300.0),
                );
                egui::ComboBox::from_id_salt("activity_log_event_filter")
                    .selected_text((*event_filter).map_or("All events", ActivityLogKind::label))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(event_filter, None, "All events");
                        for kind in ActivityLogKind::ALL {
                            ui.selectable_value(event_filter, Some(kind), kind.label());
                        }
                    });
            });
            ui.collapsing("Daily summaries", |ui| {
                daily_activity_summary_contents(ui, activity_log, hide_sensitive_info);
            });
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if activity_log.is_empty() {
                        ui.label("Point, Streak, and stream-switch events will appear here.");
                        return;
                    }
                    let mut has_matching_entries = false;
                    egui::Grid::new("activity_log_entries")
                        .num_columns(if hide_sensitive_info { 2 } else { 3 })
                        .striped(true)
                        .show(ui, |ui| {
                            if !hide_sensitive_info {
                                ui.strong("Date/time");
                            }
                            ui.strong("Streamer");
                            ui.strong("Event");
                            ui.end_row();
                            for entry in activity_log
                                .iter()
                                .filter(|entry| activity_log_matches(entry, *event_filter, search))
                            {
                                has_matching_entries = true;
                                let (streamer_text, event_text) =
                                    activity_log_display_values(entry, hide_sensitive_info);
                                if !hide_sensitive_info {
                                    ui.label(egui::RichText::new(&entry.time).monospace());
                                }
                                ui.label(streamer_text);
                                ui.label(event_text);
                                ui.end_row();
                            }
                        });
                    if !has_matching_entries {
                        ui.label("No activity log entries match these filters.");
                    }
                });
        })
        .response
    }
}
