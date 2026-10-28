use super::*;
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
    pub(super) open_capacity: usize,
}
impl Dashboard {
    pub(super) fn queue_visibility(&self) -> QueueVisibility {
        let automation_now = self.started_at.elapsed();
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
            &self.automation_delays,
            automation_now,
        );
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
        if self.automation_delays.is_paused(automation_now) {
            blockers.push("Automation temporarily paused.".to_owned());
        }
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
            blockers,
            open_capacity,
        }
    }
    pub(super) fn show_queue_visibility(&self, ui: &mut egui::Ui) {
        let _ = self.show_queue_visibility_card(ui);
    }
    pub(super) fn show_queue_visibility_card(&self, ui: &mut egui::Ui) -> egui::Rect {
        let queue = self.queue_visibility();
        ui.group(|ui| {
            ui.set_min_width(ui.available_width());
            super::ui::section_heading(ui, "Upcoming managed channels");
            if self.hide_sensitive_info {
                ui.label("Upcoming managed channels hidden during screenshot privacy.");
            } else if queue.entries.is_empty() {
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
            if !self.hide_sensitive_info {
            ui.label(format!(
                "Available stream slots: {}/{}",
                queue.open_capacity, APP_STREAM_LIMIT
            ));
            for blocker in &queue.blockers {
                ui.colored_label(egui::Color32::YELLOW, blocker);
            }
            }
        })
        .response
        .rect
    }
}
