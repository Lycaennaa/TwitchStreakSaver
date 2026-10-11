use super::*;

impl Dashboard {
    pub(super) fn start_automation(&mut self) {
        self.automation_enabled = true;
    }

    pub(super) fn stop_automation(&mut self) {
        self.automation_enabled = false;
        self.manual_open_channels.clear();
        self.auto_open_channels.clear();
        self.auto_open_retry_after.clear();
        self.points_rotation = PointsRotation::default();
        self.send_bridge_command(BrowserCommand::CloseAll);
        self.browser_notice = Some("Automation stopped and app-managed tabs closed.".to_owned());
    }

    pub(super) fn reconcile_automatic_tabs(&mut self, now: Duration) {
        if !self.automation_enabled
            || !self.browser_connected
            || !self.external_inventory_received
            || !matches!(
                &self.connection_status,
                TwitchConnectionStatus::Connected(_)
            )
        {
            self.points_rotation.pause_active_sessions(now);
            return;
        }
        resume_due_auto_open_retries(
            &mut self.auto_open_channels,
            &mut self.auto_open_retry_after,
            Instant::now(),
        );

        let external_streams = active_external_stream_count(
            &self.external_stream_counts,
            self.external_unmatched_streams,
            &self.live_channel_ids,
        );
        let manually_open_streams = self
            .manual_open_channels
            .iter()
            .filter(|channel_id| self.live_channel_ids.contains(*channel_id))
            .count();
        let manual_active_streams = manually_open_streams.saturating_add(external_streams);
        let external_channel_ids = self
            .external_stream_counts
            .iter()
            .filter(|(channel_id, count)| {
                **count > 0 && self.live_channel_ids.contains(*channel_id)
            })
            .map(|(channel_id, _)| channel_id.clone())
            .collect::<HashSet<_>>();
        let channels = &self.channels;
        let manual_open_channels = &self.manual_open_channels;
        let live_channel_ids = &self.live_channel_ids;
        let streak_completed = &self.streak_completed;
        let external_channel_ids = &external_channel_ids;
        let candidates = || {
            channels
                .iter()
                .filter(|channel| {
                    !manual_open_channels.contains(&channel.id)
                        && !external_channel_ids.contains(&channel.id)
                })
                .map(|channel| QueueCandidate {
                    channel_id: &channel.id,
                    is_live: live_channel_ids.contains(&channel.id),
                    selection: channel.selection,
                    streak_completed_for_live: streak_completed.contains_key(&channel.id),
                })
        };
        let mut plan = self
            .queue_planner
            .plan(candidates(), self.mode, manual_active_streams);

        match plan.stage {
            QueueStage::Points if !plan.channel_ids.is_empty() => {
                let browser_now = Instant::now();
                let sessions = plan
                    .channel_ids
                    .iter()
                    .map(|channel_id| {
                        points_session_for(
                            channel_id,
                            self.browser_states.get(channel_id),
                            browser_now,
                            true,
                        )
                    })
                    .collect::<Vec<_>>();
                match self.points_rotation.observe_active_sessions(&sessions, now) {
                    Ok(completed) if !completed.is_empty() => {
                        for channel_id in completed {
                            self.queue_planner
                                .record_points_turn_finished(channel_id.clone());
                            self.points_rotation.reset_completed_turn(&channel_id);
                            if let Some(channel_name) = self
                                .channels
                                .iter()
                                .find(|channel| channel.id == channel_id)
                                .map(|channel| channel.display_name.clone())
                            {
                                enqueue_points_notice(
                                    &mut self.points_notices,
                                    PointsNotice::TurnCompleted { channel_name },
                                );
                            }
                        }
                        plan =
                            self.queue_planner
                                .plan(candidates(), self.mode, manual_active_streams);
                    }
                    Ok(_) => {}
                    Err(error) => {
                        self.browser_error = Some(format!("Points rotation paused: {error:?}."));
                        self.points_rotation.pause_active_sessions(now);
                        plan.channel_ids.clear();
                    }
                }
            }
            QueueStage::Points => {
                self.points_rotation.pause_active_sessions(now);
            }
            QueueStage::Streak => self.points_rotation.pause_active_sessions(now),
            QueueStage::Idle => {
                let _ = self.points_rotation.observe_active_sessions(&[], now);
            }
        }

        let desired = if matches!(plan.stage, QueueStage::Streak | QueueStage::Points) {
            plan.channel_ids.into_iter().collect::<HashSet<_>>()
        } else {
            HashSet::new()
        };
        let close_ids = self
            .auto_open_channels
            .difference(&desired)
            .filter(|channel_id| !self.manual_open_channels.contains(*channel_id))
            .cloned()
            .collect::<Vec<_>>();
        let open_channels = desired
            .difference(&self.auto_open_channels)
            .filter(|channel_id| !self.auto_open_retry_after.contains_key(*channel_id))
            .filter_map(|channel_id| {
                self.channels
                    .iter()
                    .find(|channel| channel.id == *channel_id)
                    .map(BrowserChannel::from)
            })
            .collect::<Vec<_>>();
        self.auto_open_retry_after
            .retain(|channel_id, _| desired.contains(channel_id));

        if !close_ids.is_empty() {
            self.send_bridge_command(BrowserCommand::CloseChannels {
                channel_ids: close_ids,
            });
        }
        if !open_channels.is_empty() {
            self.send_bridge_command(BrowserCommand::OpenSelected {
                layout: self.tab_layout,
                wake_browser: self.wake_browser_on_open,
                channels: open_channels,
            });
        }
        self.auto_open_channels = desired;
    }

    pub(super) fn install_chrome_native_host(&mut self) {
        match twitch_streak_saver::browser_bridge::install_native_host(&self.extension_id) {
            Ok(path) => {
                self.browser_error = None;
                self.browser_notice = Some(format!(
                    "Native host installed for Chrome and Brave. Reload the extension. Manifests: {path}"
                ));
            }
            Err(error) => self.browser_error = Some(error),
        }
    }
}
