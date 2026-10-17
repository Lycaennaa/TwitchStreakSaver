use super::*;

impl Dashboard {
    pub(super) fn start_automation(&mut self) {
        self.automation_enabled = true;
    }

    pub(super) fn stop_automation(&mut self) {
        self.automation_enabled = false;
        self.manual_open_channels.clear();
        self.auto_open_channels.clear();
        self.pending_stream_switch = None;
        self.auto_open_retry_after.clear();
        self.points_rotation = StreamRotation::default();
        self.streak_rotation = StreamRotation::default();
        self.queue_planner.clear_streak_turn_deferrals();
        self.send_bridge_command(BrowserCommand::CloseAll);
        self.browser_notice = Some("Automation stopped and app-managed tabs closed.".to_owned());
    }

    pub(super) fn reconcile_automatic_tabs(&mut self, now: Duration) {
        self.removed_auto_streamer_names.retain(|channel_id, _| {
            self.auto_open_channels.contains(channel_id)
                && !self
                    .channels
                    .iter()
                    .any(|channel| channel.id == *channel_id)
        });
        if !self.automation_enabled
            || !self.browser_connected
            || !self.external_inventory_received
            || !matches!(
                &self.connection_status,
                TwitchConnectionStatus::Connected(_)
            )
        {
            self.points_rotation.pause_active_sessions(now);
            self.streak_rotation.pause_active_sessions(now);
            return;
        }
        let defer_due_retries = self.pending_stream_switch.is_some();
        let due_auto_open_retries = resume_due_auto_open_retries(
            &mut self.auto_open_channels,
            &mut self.auto_open_retry_after,
            Instant::now(),
            defer_due_retries,
        );
        self.queue_planner.expire_streak_turn_deferrals(now);
        self.queue_planner
            .retain_live_channels(&self.live_channel_ids);

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
        let raid_redirected_channels = &self.raid_redirected_channels;
        let streak_completed = &self.streak_completed;
        let external_channel_ids = &external_channel_ids;
        let candidates = || {
            channels
                .iter()
                .filter(|channel| {
                    !manual_open_channels.contains(&channel.id)
                        && !external_channel_ids.contains(&channel.id)
                        && !raid_redirected_channels.contains(&channel.id)
                })
                .map(|channel| QueueCandidate {
                    channel_id: &channel.id,
                    is_live: live_channel_ids.contains(&channel.id),
                    selection: channel.selection,
                    streak_completed_for_live: streak_completed.contains_key(&channel.id),
                })
        };
        let mut plan = self.queue_planner.plan(candidates(), manual_active_streams);

        match plan.stage {
            QueueStage::Points if !plan.channel_ids.is_empty() => {
                let _ = self.streak_rotation.observe_active_sessions(&[], now);
                let browser_now = Instant::now();
                let sessions = plan
                    .channel_ids
                    .iter()
                    .map(|channel_id| {
                        stream_turn_session_for(
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
                                record_activity(
                                    &mut self.activity_log,
                                    ActivityLogKind::PointsTurnCompleted,
                                    channel_name,
                                    "Points turn completed; queue advanced after 16 eligible minutes."
                                        .to_owned(),
                                );
                            }
                        }
                        plan = self.queue_planner.plan(candidates(), manual_active_streams);
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
                let _ = self.streak_rotation.observe_active_sessions(&[], now);
            }
            QueueStage::Streak if !plan.channel_ids.is_empty() => {
                self.points_rotation.pause_active_sessions(now);
                let browser_now = Instant::now();
                let sessions = plan
                    .channel_ids
                    .iter()
                    .map(|channel_id| {
                        stream_turn_session_for(
                            channel_id,
                            self.browser_states.get(channel_id),
                            browser_now,
                            true,
                        )
                    })
                    .collect::<Vec<_>>();
                match self.streak_rotation.observe_active_sessions(&sessions, now) {
                    Ok(completed) if !completed.is_empty() => {
                        for channel_id in completed {
                            self.queue_planner
                                .record_streak_turn_timed_out(channel_id.clone(), now);
                            self.streak_rotation.reset_completed_turn(&channel_id);
                            if let Some(channel_name) = self
                                .channels
                                .iter()
                                .find(|channel| channel.id == channel_id)
                                .map(|channel| channel.display_name.clone())
                            {
                                record_activity(
                                    &mut self.activity_log,
                                    ActivityLogKind::StreakTurnEnded,
                                    channel_name,
                                    "Streak turn ended after 16 eligible minutes without a WATCH_STREAK event."
                                        .to_owned(),
                                );
                            }
                        }
                        plan = self.queue_planner.plan(candidates(), manual_active_streams);
                    }
                    Ok(_) => {}
                    Err(error) => {
                        self.browser_error = Some(format!("Streak rotation paused: {error:?}."));
                        self.streak_rotation.pause_active_sessions(now);
                        plan.channel_ids.clear();
                    }
                }
            }
            QueueStage::Streak => {
                self.points_rotation.pause_active_sessions(now);
                self.streak_rotation.pause_active_sessions(now);
            }
            QueueStage::Idle => {
                let _ = self.points_rotation.observe_active_sessions(&[], now);
                let _ = self.streak_rotation.observe_active_sessions(&[], now);
            }
        }

        let desired = if matches!(plan.stage, QueueStage::Streak | QueueStage::Points) {
            plan.channel_ids.into_iter().collect::<HashSet<_>>()
        } else {
            HashSet::new()
        };

        if self.pending_stream_switch.is_some() {
            let mut retry_close_ids = Vec::new();
            let mut retry_open_channels = Vec::new();
            let mut retry_channel_ids = Vec::new();
            let mut abandoned_destination_ids = Vec::new();
            for channel_id in due_auto_open_retries {
                let state = self
                    .pending_stream_switch
                    .as_ref()
                    .and_then(|pending| pending.channels.get(&channel_id))
                    .copied();
                if matches!(state, Some(PendingStreamChannelState::Opening))
                    && self.raid_redirected_channels.contains(&channel_id)
                {
                    abandoned_destination_ids.push(channel_id);
                    continue;
                }
                match state {
                    Some(PendingStreamChannelState::Closing) => {
                        retry_close_ids.push(channel_id.clone());
                        retry_channel_ids.push(channel_id);
                    }
                    Some(PendingStreamChannelState::Opening) => {
                        if let Some(channel) = self
                            .channels
                            .iter()
                            .find(|channel| channel.id == channel_id)
                        {
                            retry_open_channels.push(BrowserChannel::from(channel));
                            retry_channel_ids.push(channel_id);
                        } else {
                            abandoned_destination_ids.push(channel_id);
                        }
                    }
                    _ => {}
                }
            }
            if !abandoned_destination_ids.is_empty() {
                if let Some(pending) = &mut self.pending_stream_switch {
                    for channel_id in &abandoned_destination_ids {
                        pending.abandon_open(channel_id);
                    }
                    pending
                        .desired_channels
                        .retain(|channel_id| desired.contains(channel_id));
                }
                for channel_id in &abandoned_destination_ids {
                    self.auto_open_channels.remove(channel_id);
                    self.auto_open_retry_after.remove(channel_id);
                }
                self.finish_pending_stream_switch();
            }
            for channel_id in &retry_channel_ids {
                self.auto_open_channels.remove(channel_id);
                self.auto_open_retry_after.remove(channel_id);
            }
            if !retry_close_ids.is_empty() {
                self.send_bridge_command(BrowserCommand::CloseChannels {
                    channel_ids: retry_close_ids,
                });
            }
            if !retry_open_channels.is_empty() {
                self.send_bridge_command(BrowserCommand::OpenSelected {
                    wake_browser: self.wake_browser_on_open,
                    channels: retry_open_channels,
                });
            }
            self.auto_open_channels.extend(retry_channel_ids);
            return;
        }
        let close_ids = self
            .auto_open_channels
            .difference(&desired)
            .filter(|channel_id| !self.manual_open_channels.contains(*channel_id))
            .cloned()
            .collect::<Vec<_>>();
        let mut open_streamer_names = Vec::new();
        let mut open_streamer_ids = Vec::new();
        let open_channels = desired
            .difference(&self.auto_open_channels)
            .filter(|channel_id| !self.auto_open_retry_after.contains_key(*channel_id))
            .filter_map(|channel_id| {
                self.channels
                    .iter()
                    .find(|channel| channel.id == *channel_id)
                    .map(|channel| {
                        open_streamer_ids.push(channel_id.clone());
                        open_streamer_names.push(channel.display_name.clone());
                        BrowserChannel::from(channel)
                    })
            })
            .collect::<Vec<_>>();
        self.auto_open_retry_after
            .retain(|channel_id, _| desired.contains(channel_id));

        if !close_ids.is_empty() && !open_channels.is_empty() {
            let mut previous_streamers = close_ids
                .iter()
                .map(|channel_id| {
                    self.channels
                        .iter()
                        .find(|channel| channel.id == *channel_id)
                        .map(|channel| channel.display_name.clone())
                        .or_else(|| self.removed_auto_streamer_names.get(channel_id).cloned())
                        .unwrap_or_else(|| channel_id.clone())
                })
                .collect::<Vec<_>>();
            previous_streamers.sort();
            open_streamer_names.sort();
            let previous_streamers = previous_streamers.join(", ");
            let event = format!(
                "Switched from {previous_streamers} to {}.",
                open_streamer_names.join(", ")
            );
            let mut channels = close_ids
                .iter()
                .cloned()
                .map(|channel_id| (channel_id, PendingStreamChannelState::Closing))
                .collect::<HashMap<_, _>>();
            channels.extend(
                open_streamer_ids
                    .into_iter()
                    .map(|channel_id| (channel_id, PendingStreamChannelState::Opening)),
            );
            let pending = PendingStreamSwitch {
                streamer: previous_streamers,
                event,
                desired_channels: desired,
                channels,
            };
            self.auto_open_channels
                .extend(pending.desired_channels.iter().cloned());
            self.pending_stream_switch = Some(pending);
        } else {
            self.auto_open_channels = desired;
        }
        if !close_ids.is_empty() {
            self.send_bridge_command(BrowserCommand::CloseChannels {
                channel_ids: close_ids,
            });
        }
        if !open_channels.is_empty() {
            self.send_bridge_command(BrowserCommand::OpenSelected {
                wake_browser: self.wake_browser_on_open,
                channels: open_channels,
            });
        }
    }
}
