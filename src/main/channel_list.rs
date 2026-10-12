use super::*;

impl Dashboard {
    pub(super) fn channel_list(&mut self, ui: &mut egui::Ui) {
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
        let browser_notice = self.browser_notice.clone();
        let pubsub_connected = self.pubsub_connected;
        let pubsub_error = self.pubsub_error.clone();
        let streak_completed = &self.streak_completed;
        let points_balances = &self.channel_points_balances;
        let mut open_selected = false;

        {
            let channels = &mut self.channels;
            let search = &mut self.followed_search;
            let live_channel_ids = &self.live_channel_ids;
            let browser_states = &self.browser_states;
            let points_rotation = &self.points_rotation;
            let points_now = self.started_at.elapsed();
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
                ui.add(egui::TextEdit::singleline(search).hint_text("Fuzzy search channels"));
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
                ui.label("Separate player tabs are muted by default.");

                if let Some(error) = &browser_error {
                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                }
                if let Some(error) = &pubsub_error {
                    ui.colored_label(egui::Color32::LIGHT_RED, format!("Points events: {error}"));
                }
                if let Some(notice) = &browser_notice {
                    ui.label(egui::RichText::new(notice).small().weak());
                }
                if has_no_channels {
                    ui.label(if connected {
                        "No followed channels found on this Twitch account."
                    } else {
                        "Connect Twitch to load followed channels."
                    });
                } else if filtered_indices.is_empty() {
                    ui.label("No followed channels match the search.");
                }

                egui::ScrollArea::vertical()
                    .max_height(480.0)
                    .show(ui, |ui| {
                        for index in filtered_indices {
                            let channel = &mut channels[index];
                            ui.group(|ui| {
                                ui.horizontal(|ui| {
                                    ui.vertical(|ui| {
                                        ui.strong(&channel.display_name);
                                        let live_status = if live_channel_ids.contains(&channel.id)
                                        {
                                            "LIVE"
                                        } else if connected {
                                            "Offline"
                                        } else if channel.id.starts_with("sample-") {
                                            "Sample channel"
                                        } else {
                                            "Not connected"
                                        };
                                        ui.label(egui::RichText::new(live_status).small().weak());
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
                                                        POINTS_TURN_DURATION.as_secs() / 60,
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
                                        } else {
                                            ui.label(
                                                egui::RichText::new(if browser_connected {
                                                    "No app-managed tabs"
                                                } else {
                                                    "Browser status unavailable"
                                                })
                                                .small()
                                                .weak(),
                                            );
                                        }
                                    });
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            ui.checkbox(&mut channel.selection.points, "Points");
                                            ui.checkbox(&mut channel.selection.streak, "Streak");
                                        },
                                    );
                                });
                            });
                            ui.add_space(4.0);
                        }
                    });
            });
        }
        if open_selected {
            self.open_selected_twitch_channels();
        }
    }
}
