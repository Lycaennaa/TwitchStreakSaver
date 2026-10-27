use super::channel_selection_controls::{channel_selection_controls, selection_bulk_controls};
use super::*;

fn freshness_age(confirmed_at: Option<Instant>) -> String {
    let Some(confirmed_at) = confirmed_at else {
        return "not yet confirmed".to_owned();
    };
    let seconds = confirmed_at.elapsed().as_secs();
    if seconds < 60 {
        format!("{seconds}s ago")
    } else if seconds < 3_600 {
        format!("{}m ago", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}h {}m ago", seconds / 3_600, (seconds % 3_600) / 60)
    } else {
        format!("{}d {}h ago", seconds / 86_400, (seconds % 86_400) / 3_600)
    }
}

impl Dashboard {
    pub(super) fn channel_list(
        &mut self,
        ui: &mut egui::Ui,
        right_panel_bottom_y: f32,
    ) -> egui::Rect {
        let connected = matches!(
            &self.connection_status,
            TwitchConnectionStatus::Connected(_)
        );
        let browser_connected = self.browser_connected;
        let external_inventory_received = self.external_inventory_received;
        let has_sample_data = self
            .channels
            .iter()
            .any(|channel| channel.id.starts_with("sample-"));
        let has_no_channels = self.channels.is_empty();
        let filtered_indices =
            followed_channel_search_indices(&self.channels, &self.followed_search);
        let browser_error = self.browser_error.clone();
        let pubsub_connected = self.pubsub_connected;
        let pubsub_error = self.pubsub_error.clone();
        let twitch_baseline_confirmed_at = self.twitch_baseline_confirmed_at;
        let pubsub_state_confirmed_at = self.pubsub_state_confirmed_at;
        let pubsub_last_event_at = self.pubsub_last_event_at;
        let pubsub_missed_events_since = self.pubsub_missed_events_since;
        let streak_completed = &self.streak_completed;
        let points_balances = &self.channel_points_balances;
        let mut open_selected = false;
        let mut unmuted_changed = false;

        let mut list_rect = egui::Rect::NOTHING;
        let list_content_bottom_y =
            right_panel_bottom_y - egui::Frame::group(ui.style()).inner_margin.bottom as f32;
        self.show_queue_visibility(ui);
        {
            let channels = &mut self.channels;
            let search = &mut self.followed_search;
            let live_channel_ids = &self.live_channel_ids;
            let browser_states = &self.browser_states;
            let points_rotation = &self.points_rotation;
            let points_now = self.started_at.elapsed();
            let automation_delays = &mut self.automation_delays;
            let hide_sensitive_info = self.hide_sensitive_info;
            ui.group(|ui| {
                ui.horizontal(|ui| {
                    ui.strong("Followed channels");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if has_sample_data {
                            ui.label(egui::RichText::new("SAMPLE DATA").small().weak());
                        } else if connected {
                            ui.label(egui::RichText::new("LIVE STATUS · 1 MIN").small().weak());
                        }
                    });
                });
                ui.add_space(8.0);
                ui.add(
                    egui::TextEdit::singleline(search)
                        .password(hide_sensitive_info)
                        .hint_text("Search channels"),
                );
                if hide_sensitive_info {
                    ui.label("Bulk channel controls hidden during screenshot privacy.");
                } else {
                    let _ = selection_bulk_controls(ui, channels, &filtered_indices);
                }
                ui.horizontal_wrapped(|ui| {
                    open_selected = ui.button("Open selected channels").clicked();
                    ui.label(if browser_connected && external_inventory_received {
                        "Browser connected"
                    } else if browser_connected {
                        "Scanning Twitch tabs"
                    } else {
                        "Browser not connected"
                    });
                    ui.label(if pubsub_connected {
                        "Points & Streak events connected"
                    } else if connected {
                        "Points & Streak events reconnecting"
                    } else {
                        "Points & Streak events disconnected"
                    });
                });
                ui.horizontal_wrapped(|ui| {
                    if hide_sensitive_info {
                        ui.label("Live-status and PubSub timing hidden during screenshot privacy.");
                    } else {
                    ui.label(format!(
                        "Twitch live status last confirmed {}",
                        freshness_age(twitch_baseline_confirmed_at)
                    ));
                    ui.label(format!(
                        "PubSub state last confirmed {}",
                        freshness_age(pubsub_state_confirmed_at)
                    ));
                    if let Some(last_event_at) = pubsub_last_event_at {
                        ui.label(format!(
                            "Last PubSub event observed {}",
                            freshness_age(Some(last_event_at))
                        ));
                    } else {
                        ui.label("No PubSub events observed this session");
                    }
                    }
                });
                if !hide_sensitive_info {
                if let Some(gap_started_at) = pubsub_missed_events_since {
                    ui.colored_label(
                        egui::Color32::YELLOW,
                        format!(
                            "PubSub monitoring gap started {}; events may have been missed. Reconnecting does not restore event history.",
                            freshness_age(Some(gap_started_at))
                        ),
                    );
                }
                }
                ui.label(
                    "Separate player tabs are muted by default; select Unmuted to start with audio.",
                );

                if let Some(error) = &browser_error {
                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                }
                if let Some(error) = &pubsub_error {
                    ui.colored_label(egui::Color32::LIGHT_RED, format!("Points events: {error}"));
                }
                if !hide_sensitive_info {
                    if has_no_channels {
                    ui.label(if connected {
                        "No followed channels found on this Twitch account."
                    } else {
                        "Connect Twitch to load followed channels."
                    });
                } else if filtered_indices.is_empty() {
                    ui.label("No followed channels match the search.");
                }

                }
                // Match the bottom of the right-hand settings group.
                let list_height =
                    (list_content_bottom_y - ui.available_rect_before_wrap().min.y).max(0.0);
                let list_output = ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), list_height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        egui::ScrollArea::vertical()
                    .max_height(list_height)
                    .min_scrolled_height(0.0)
                .auto_shrink([true, false])
                .show(ui, |ui| {
                        if hide_sensitive_info {
                            ui.label("Followed-channel list hidden during screenshot privacy.");
                        } else {
                        for index in filtered_indices {
                            let channel = &mut channels[index];
                            ui.group(|ui| {
                                ui.horizontal(|ui| {
                                    ui.vertical(|ui| {
                                        ui.horizontal(|ui| {
                                            ui.strong(&channel.display_name);
                                            let live_status =
                                                if live_channel_ids.contains(&channel.id) {
                                                    "- live"
                                                } else if connected {
                                                    "- offline"
                                                } else if channel.id.starts_with("sample-") {
                                                    "- sample channel"
                                                } else {
                                                    "- not connected"
                                                };
                                            ui.label(live_status);
                                        });
                                        if let Some(points) = streak_completed.get(&channel.id) {
                                            ui.colored_label(
                                                egui::Color32::from_rgb(80, 210, 140),
                                                format!("WATCH_STREAK detected · {points} points"),
                                            );
                                        }
                                        if let Some(balance) = points_balances.get(&channel.id) {
                                            ui.label(format!(
                                                "Channel Points balance (latest event): {balance}"
                                            ));
                                        }
                                        if channel.selection.points {
                                            if let Some(elapsed) = points_rotation
                                                .elapsed_eligible(&channel.id, points_now)
                                            {
                                                ui.label(
                                                    egui::RichText::new(format!(
                                                        "Points turn: {}m {:02}s / {}m eligible",
                                                        elapsed.as_secs() / 60,
                                                        elapsed.as_secs() % 60,
                                                        STREAM_TURN_DURATION.as_secs() / 60,
                                                    ))
                                                    .small()
                                                    .weak(),
                                                );
                                            }
                                        }
                                        if let Some(state) = browser_states.get(&channel.id) {
                                            let player_state =
                                                match (state.player_tab_id, state.player_last_seen)
                                                {
                                                    (None, _) => "closed",
                                                    (Some(_), None) => "waiting",
                                                    (Some(_), Some(last_seen))
                                                        if last_seen.elapsed()
                                                            > BROWSER_STATE_STALE_AFTER =>
                                                    {
                                                        "stale"
                                                    }
                                                    _ => state
                                                        .player_state
                                                        .map(PlayerState::as_str)
                                                        .unwrap_or("waiting"),
                                                };
                                            let chat_state =
                                                match (state.chat_tab_id, state.chat_last_seen) {
                                                    (None, _) => "closed",
                                                    (Some(_), None) => "waiting",
                                                    (Some(_), Some(last_seen))
                                                        if last_seen.elapsed()
                                                            > BROWSER_STATE_STALE_AFTER =>
                                                    {
                                                        "stale"
                                                    }
                                                    _ => state
                                                        .chat_state
                                                        .map(ChatState::as_str)
                                                        .unwrap_or("waiting"),
                                                };
                                            ui.label(
                                                egui::RichText::new(format!(
                                                    "Player: {player_state} · Chat: {chat_state}"
                                                ))
                                                .small()
                                                .weak(),
                                            );
                                        } else if browser_connected {
                                            ui.label(
                                                egui::RichText::new("No app-managed tabs")
                                                    .small()
                                                    .weak(),
                                            );
                                        }
                                    });
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if channel_selection_controls(ui, channel).changed() {
                                                unmuted_changed = true;
                                            }
                                            channel_snooze_controls(
                                                ui,
                                                automation_delays,
                                                &channel.id,
                                                points_now,
                                            );
                                        },
                                    );
                                });
                            });
                            ui.add_space(4.0);
                        }
                        }
                            })
                            .inner_rect
                    },
                );
                list_rect = list_output.inner;
            });
        }
        if unmuted_changed {
            self.sync_extension_channels();
        }
        if open_selected {
            self.open_selected_twitch_channels();
        }
        list_rect
    }
}

fn channel_snooze_controls(
    ui: &mut egui::Ui,
    automation_delays: &mut automation::AutomationDelays,
    channel_id: &str,
    now: Duration,
) {
    if let Some(remaining) = automation_delays.channel_snooze_remaining(channel_id, now) {
        ui.label(format!(
            "Snoozed · {}",
            automation::format_remaining_duration(remaining)
        ));
        if ui.button("Unsnooze").clicked() {
            automation_delays.unsnooze_channel(channel_id);
        }
    } else {
        ui.menu_button("Snooze", |ui| {
            for (duration, label) in automation::TEMPORARY_AUTOMATION_DURATIONS {
                if ui.button(label).clicked() {
                    automation_delays.snooze_channel_for(channel_id, now, duration);
                    ui.close_menu();
                }
            }
        });
    }
}
