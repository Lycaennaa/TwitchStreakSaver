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

pub(super) struct QueueVisibilityEntry {
    pub(super) channel_id: String,
    pub(super) display_name: String,
    pub(super) player_ready: bool,
    pub(super) chat_ready: bool,
}

pub(super) struct QueueVisibility {
    pub(super) stage: QueueStage,
    pub(super) entries: Vec<QueueVisibilityEntry>,
    pub(super) blockers: Vec<String>,
    pub(super) offline_waiting: Vec<QueueVisibilityEntry>,
    pub(super) open_capacity: usize,
}
impl Dashboard {
    pub(super) fn queue_visibility(&self) -> QueueVisibility {
        let external_streams = active_external_stream_count(
            &self.external_stream_counts,
            self.external_unmatched_streams,
            &self.live_channel_ids,
        );
        let manually_open_streams = active_app_stream_count(
            &self.manual_open_channels,
            &self.channels,
            &self.live_channel_ids,
        );
        let candidates = queue_candidates(
            &self.channels,
            &self.manual_open_channels,
            &self.external_stream_counts,
            &self.live_channel_ids,
            &self.raid_redirected_channels,
            &self.streak_completed,
        );
        let offline_waiting = candidates
            .iter()
            .filter(|candidate| {
                !candidate.is_live && (candidate.selection.streak || candidate.selection.points)
            })
            .filter_map(|candidate| {
                let channel = self
                    .channels
                    .iter()
                    .find(|channel| channel.id == candidate.channel_id)?;
                Some(QueueVisibilityEntry {
                    channel_id: channel.id.clone(),
                    display_name: channel.display_name.clone(),
                    player_ready: false,
                    chat_ready: false,
                })
            })
            .collect();
        let plan = self.queue_planner.plan(candidates.iter().copied(), 0);
        let now = Instant::now();
        let entries: Vec<QueueVisibilityEntry> = plan
            .channel_ids
            .iter()
            .filter_map(|channel_id| {
                let channel = self
                    .channels
                    .iter()
                    .find(|channel| channel.id == *channel_id)?;
                let turn = stream_turn_session_for(
                    channel_id,
                    self.browser_states.get(channel_id),
                    now,
                    true,
                );
                Some(QueueVisibilityEntry {
                    channel_id: channel_id.clone(),
                    display_name: channel.display_name.clone(),
                    player_ready: turn.eligibility.player_playing,
                    chat_ready: turn.eligibility.chat_ready,
                })
            })
            .collect();
        let stream_count = manually_open_streams.saturating_add(external_streams);
        let open_capacity = APP_STREAM_LIMIT.saturating_sub(stream_count);
        let mut blockers = Vec::new();
        if !self.automation_enabled {
            blockers.push("Start automation to open managed queue channels.".to_owned());
        }
        if !matches!(
            &self.connection_status,
            TwitchConnectionStatus::Connected(_)
        ) {
            blockers.push(
                "Connect Twitch to refresh live channels and open managed channels.".to_owned(),
            );
        }
        if !self.browser_connected {
            blockers.push("Connect the configured browser to open managed channels.".to_owned());
        } else if !self.external_inventory_received {
            blockers.push(
                "Wait for the browser's Twitch-tab inventory before opening channels.".to_owned(),
            );
        }
        if !entries.is_empty()
            && self.browser_connected
            && self.external_inventory_received
            && !self.browser_command_ack_supported
        {
            blockers.push(
                "Reload or update the browser extension (version 0.3.18 or newer) to enable command acknowledgements required for opening channels."
                    .to_owned(),
            );
        }
        if open_capacity == 0 {
            blockers.push(format!(
                "Stream capacity is full ({stream_count}/{APP_STREAM_LIMIT}); close a managed or external stream to make room."
            ));
        }
        QueueVisibility {
            stage: plan.stage,
            entries,
            offline_waiting,
            blockers,
            open_capacity,
        }
    }

    fn show_queue_visibility(&self, ui: &mut egui::Ui) {
        let queue = self.queue_visibility();
        ui.group(|ui| {
            ui.strong("Upcoming managed channels");
            if queue.entries.is_empty() {
                ui.label(match queue.stage {
                    QueueStage::Idle => "No selected live channels are waiting in the queue.",
                    QueueStage::Streak | QueueStage::Points => {
                        "The selected live queue is empty."
                    }
                });
            } else {
                ui.label(match queue.stage {
                    QueueStage::Streak => "Next up: Watch Streak",
                    QueueStage::Points => "Next up: Channel Points",
                    QueueStage::Idle => "Queue idle",
                });
                for entry in &queue.entries {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong(&entry.display_name)
                            .on_hover_text(&entry.channel_id);
                        ui.label(format!(
                            "Player {} · Chat {}",
                            if entry.player_ready { "ready" } else { "waiting" },
                            if entry.chat_ready { "ready" } else { "waiting" },
                        ));
                    });
                }
                ui.label("Player/chat readiness affects the 16-minute turn timer only; it does not prevent opening.");
            }
            ui.label(format!(
                "Available stream slots: {}/{}",
                queue.open_capacity, APP_STREAM_LIMIT
            ));
            for blocker in &queue.blockers {
                ui.colored_label(egui::Color32::YELLOW, blocker);
            }
            if !queue.offline_waiting.is_empty() {
                ui.label("Waiting to go live:");
                for entry in &queue.offline_waiting {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong(&entry.display_name);
                        ui.label("Waiting for channel to go live");
                    });
                }
            }
        });
    }

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

        self.show_queue_visibility(ui);
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
                ui.add(egui::TextEdit::singleline(search).hint_text("Search channels"));
                let _ = selection_bulk_controls(ui, channels);
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
                });
                if let Some(gap_started_at) = pubsub_missed_events_since {
                    ui.colored_label(
                        egui::Color32::YELLOW,
                        format!(
                            "PubSub monitoring gap started {}; events may have been missed. Reconnecting does not restore event history.",
                            freshness_age(Some(gap_started_at))
                        ),
                    );
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
                                        },
                                    );
                                });
                            });
                            ui.add_space(4.0);
                        }
                    });
            });
        }
        if unmuted_changed {
            self.sync_extension_channels();
        }
        if open_selected {
            self.open_selected_twitch_channels();
        }
    }
}

fn channel_selection_controls(ui: &mut egui::Ui, channel: &mut FollowedChannel) -> egui::Response {
    let unmuted = ui
        .checkbox(&mut channel.unmuted, "Unmuted")
        .on_hover_text("Start this player's tab with audio enabled.");
    ui.checkbox(&mut channel.selection.points, "Points");
    ui.checkbox(&mut channel.selection.streak, "Streak");
    unmuted
}

fn selection_bulk_controls(
    ui: &mut egui::Ui,
    channels: &mut [FollowedChannel],
) -> (egui::Response, egui::Response) {
    let enabled = !channels.is_empty();
    let all_points_selected =
        !channels.is_empty() && channels.iter().all(|channel| channel.selection.points);
    let all_streaks_selected =
        !channels.is_empty() && channels.iter().all(|channel| channel.selection.streak);
    let (points, streaks) = ui
        .horizontal_wrapped(|ui| {
            let points_label = if all_points_selected {
                "Unselect all Points"
            } else {
                "Select all Points"
            };
            let points = ui.add_enabled(enabled, egui::Button::new(points_label));
            let select_points = points.clicked();
            let points = points.on_hover_text(if all_points_selected {
                "Clear Points selection for all followed channels, regardless of the current search."
            } else {
                "Select Points for all followed channels, regardless of the current search."
            });
            if select_points {
                for channel in channels.iter_mut() {
                    channel.selection.points = !all_points_selected;
                }
            }

            let streaks_label = if all_streaks_selected {
                "Unselect all Streaks"
            } else {
                "Select all Streaks"
            };
            let streaks = ui.add_enabled(enabled, egui::Button::new(streaks_label));
            let select_streaks = streaks.clicked();
            let streaks = streaks.on_hover_text(if all_streaks_selected {
                "Clear Streak selection for all followed channels, regardless of the current search."
            } else {
                "Select Streaks for all followed channels, regardless of the current search."
            });
            if select_streaks {
                for channel in channels.iter_mut() {
                    channel.selection.streak = !all_streaks_selected;
                }
            }

            (points, streaks)
        })
        .inner;
    (points, streaks)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn show_selection_controls(
        context: &egui::Context,
        channels: &mut [FollowedChannel],
        events: Vec<egui::Event>,
    ) -> [egui::Rect; 2] {
        let mut button_rects = None;
        let _ = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600.0, 300.0),
                )),
                events,
                ..Default::default()
            },
            |context| {
                egui::CentralPanel::default().show(context, |ui| {
                    let (points, streaks) = selection_bulk_controls(ui, channels);
                    button_rects = Some([points.rect, streaks.rect]);
                });
            },
        );
        button_rects.expect("selection controls were not rendered")
    }

    fn click_button(context: &egui::Context, channels: &mut [FollowedChannel], index: usize) {
        let rect = show_selection_controls(context, channels, Vec::new())[index];
        let position = rect.center();
        for pressed in [true, false] {
            let _ = show_selection_controls(
                context,
                channels,
                vec![
                    egui::Event::PointerMoved(position),
                    egui::Event::PointerButton {
                        pos: position,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
    }

    fn show_channel_selection_controls(
        context: &egui::Context,
        channel: &mut FollowedChannel,
        events: Vec<egui::Event>,
    ) -> egui::Rect {
        let mut unmuted_rect = None;
        let _ = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600.0, 300.0),
                )),
                events,
                ..Default::default()
            },
            |context| {
                egui::CentralPanel::default().show(context, |ui| {
                    unmuted_rect = Some(channel_selection_controls(ui, channel).rect);
                });
            },
        );
        unmuted_rect.expect("channel selection controls were not rendered")
    }

    fn click_unmuted(context: &egui::Context, channel: &mut FollowedChannel) {
        let position = show_channel_selection_controls(context, channel, Vec::new()).center();
        for pressed in [true, false] {
            let _ = show_channel_selection_controls(
                context,
                channel,
                vec![
                    egui::Event::PointerMoved(position),
                    egui::Event::PointerButton {
                        pos: position,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
    }

    #[test]
    fn unmuted_checkbox_only_changes_audio_preference() {
        let mut channel = FollowedChannel::new("channel", "Channel");
        let context = egui::Context::default();

        click_unmuted(&context, &mut channel);

        assert!(channel.unmuted);
        assert!(!channel.selection.streak);
        assert!(!channel.selection.points);

        click_unmuted(&context, &mut channel);

        assert!(!channel.unmuted);
        assert!(!channel.selection.streak);
        assert!(!channel.selection.points);
    }

    #[test]
    fn bulk_buttons_toggle_all_followed_channels_independently() {
        let mut first = FollowedChannel::new("first", "First");
        first.selection.streak = true;
        let hidden = FollowedChannel::new("hidden", "Hidden");
        let mut third = FollowedChannel::new("third", "Third");
        third.selection.points = true;
        let mut channels = vec![first, hidden, third];
        let context = egui::Context::default();

        click_button(&context, &mut channels, 0);

        assert!(channels.iter().all(|channel| channel.selection.points));
        assert!(channels[0].selection.streak);
        assert!(!channels[1].selection.streak);
        assert!(!channels[2].selection.streak);

        click_button(&context, &mut channels, 1);

        assert!(channels.iter().all(|channel| channel.selection.points));
        assert!(channels.iter().all(|channel| channel.selection.streak));

        click_button(&context, &mut channels, 0);

        assert!(channels.iter().all(|channel| !channel.selection.points));
        assert!(channels.iter().all(|channel| channel.selection.streak));

        click_button(&context, &mut channels, 1);

        assert!(channels.iter().all(|channel| !channel.selection.points));
        assert!(channels.iter().all(|channel| !channel.selection.streak));
    }
}
