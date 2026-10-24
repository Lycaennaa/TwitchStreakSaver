use super::*;

impl Dashboard {
    pub(super) fn start_twitch_connection(&mut self) {
        if let Some(cancel) = self.twitch_cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        let now = Instant::now();
        if self.pubsub_connected {
            self.note_pubsub_disconnection(now);
        } else {
            self.note_pubsub_gap(now);
        }
        self.connection_generation = self.connection_generation.wrapping_add(1);
        if let Some(bridge) = &self.browser_bridge {
            bridge.set_recovery_generation(self.connection_generation);
        }
        self.pending_reload_command = None;
        self.pending_manual_open_command = None;
        let generation = self.connection_generation;
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let updates = self.twitch_sender.clone();
        self.connection_status = TwitchConnectionStatus::Connecting;
        self.connection_error = None;
        self.twitch_network_errors.clear();
        self.twitch_network_unavailable = false;
        self.live_channel_ids.clear();
        self.twitch_cancel = Some(cancel);

        if let Err(error) = thread::Builder::new()
            .name("twitch-connection".to_owned())
            .spawn(move || {
                twitch_api::connection::run_connection(worker_cancel, generation, updates)
            })
        {
            self.twitch_cancel = None;
            self.connection_status = TwitchConnectionStatus::Disconnected;
            self.connection_error = Some(format!("Could not start Twitch connection: {error}"));
        }
    }
    pub(super) fn disconnect_twitch(&mut self) {
        if let Some(cancel) = self.twitch_cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        self.note_pubsub_disconnection(Instant::now());
        self.connection_generation = self.connection_generation.wrapping_add(1);
        if let Some(bridge) = &self.browser_bridge {
            bridge.set_recovery_generation(self.connection_generation);
        }
        self.connection_status = TwitchConnectionStatus::Disconnected;
        self.twitch_network_unavailable = false;
        self.twitch_network_errors.clear();
        self.reload_managed_player_tabs_pending = false;
        self.pending_manual_open = false;
        self.pending_manual_open_inventory_wait_since = None;
        self.pending_reload_command = None;
        self.pending_manual_open_command = None;
        self.pubsub_connected = false;
        self.pubsub_error = None;
        self.live_channel_ids.clear();
        self.streak_completed.clear();
        self.notified_streak_channels.clear();
        self.queue_planner = QueuePlanner::default();
        self.points_rotation = StreamRotation::default();
        self.streak_rotation = StreamRotation::default();
        let close_ids = self
            .auto_open_channels
            .difference(&self.manual_open_channels)
            .cloned()
            .collect::<Vec<_>>();
        self.auto_open_channels.clear();
        self.pending_stream_switch = None;
        self.auto_open_retry_after.clear();
        if !close_ids.is_empty() {
            self.send_bridge_command(BrowserCommand::CloseChannels {
                channel_ids: close_ids,
            });
        }
        self.connection_error = twitch_api::oauth::clear_saved_token().err();
    }

    pub(super) fn cancel_twitch_connection(&mut self) {
        if let Some(cancel) = self.twitch_cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        self.connection_generation = self.connection_generation.wrapping_add(1);
        if let Some(bridge) = &self.browser_bridge {
            bridge.set_recovery_generation(self.connection_generation);
        }
        self.connection_status = TwitchConnectionStatus::Disconnected;
        self.twitch_network_unavailable = false;
        self.reload_managed_player_tabs_pending = false;
        self.pending_manual_open = false;
        self.pending_manual_open_inventory_wait_since = None;
        self.pending_reload_command = None;
        self.pending_manual_open_command = None;
        self.connection_error = None;
    }

    fn replace_followed_channels(&mut self, incoming: Vec<FollowedChannel>) {
        for channel in &self.channels {
            if self.auto_open_channels.contains(&channel.id)
                && !incoming
                    .iter()
                    .any(|incoming_channel| incoming_channel.id == channel.id)
            {
                self.removed_auto_streamer_names
                    .insert(channel.id.clone(), channel.display_name.clone());
            }
        }
        self.channels =
            twitch_streak_saver::model::merge_followed_channels(&self.channels, incoming);
        let followed_ids = self
            .channels
            .iter()
            .map(|channel| channel.id.as_str())
            .collect::<HashSet<_>>();
        self.channel_points_balances
            .retain(|channel_id, _| followed_ids.contains(channel_id.as_str()));
        self.available_claim_channels
            .retain(|channel_id| followed_ids.contains(channel_id.as_str()));
    }

    pub(super) fn note_pubsub_gap(&mut self, now: Instant) {
        self.pubsub_missed_events_since.get_or_insert(now);
        self.pubsub_connected = false;
    }

    fn note_pubsub_disconnection(&mut self, now: Instant) {
        self.note_pubsub_gap(now);
        self.pubsub_state_confirmed_at = Some(now);
    }

    fn note_twitch_network_unavailable(&mut self, source: TwitchNetworkSource, message: String) {
        self.twitch_network_errors.insert(source, message);
        self.update_twitch_network_state();
    }

    fn note_twitch_network_online(&mut self, source: TwitchNetworkSource) {
        self.twitch_network_errors.remove(&source);
        self.update_twitch_network_state();
    }

    fn note_twitch_network_fully_online(&mut self) {
        self.twitch_network_errors.clear();
        self.update_twitch_network_state();
    }

    fn update_twitch_network_state(&mut self) {
        let was_unavailable = self.twitch_network_unavailable;
        self.twitch_network_unavailable = !self.twitch_network_errors.is_empty();
        if self.twitch_network_unavailable {
            self.connection_error = [
                TwitchNetworkSource::Connection,
                TwitchNetworkSource::FollowedChannels,
                TwitchNetworkSource::LiveStatus,
            ]
            .into_iter()
            .find_map(|source| self.twitch_network_errors.get(&source).cloned());
        } else {
            if was_unavailable {
                self.reload_managed_player_tabs_pending = true;
            }
            self.connection_error = None;
        }
    }

    pub(super) fn poll_twitch_updates(&mut self) {
        while let Ok(update) = self.twitch_updates.try_recv() {
            match update {
                TwitchUpdate::Connected {
                    generation,
                    login,
                    channels,
                    live_channel_ids,
                } if generation == self.connection_generation => {
                    self.note_twitch_network_fully_online();
                    self.twitch_baseline_confirmed_at = Some(Instant::now());
                    self.replace_followed_channels(channels);
                    self.streak_completed
                        .retain(|channel_id, _| live_channel_ids.contains(channel_id));
                    self.notified_streak_channels
                        .retain(|channel_id| live_channel_ids.contains(channel_id));
                    self.live_channel_ids = live_channel_ids;
                    self.connection_status = TwitchConnectionStatus::Connected(login);
                    self.connection_error = None;
                    self.clear_offline_raid_redirects();
                    self.sync_extension_channels();
                }
                TwitchUpdate::Refreshed {
                    generation,
                    channels,
                    live_channel_ids,
                } if generation == self.connection_generation => {
                    self.note_twitch_network_fully_online();
                    self.twitch_baseline_confirmed_at = Some(Instant::now());
                    self.replace_followed_channels(channels);
                    self.streak_completed
                        .retain(|channel_id, _| live_channel_ids.contains(channel_id));
                    self.notified_streak_channels
                        .retain(|channel_id| live_channel_ids.contains(channel_id));
                    self.live_channel_ids = live_channel_ids;
                    self.connection_error = None;
                    self.clear_offline_raid_redirects();
                    self.sync_extension_channels();
                }
                TwitchUpdate::FollowedChannels {
                    generation,
                    channels,
                    live_status_failure,
                } if generation == self.connection_generation => {
                    self.note_twitch_network_online(TwitchNetworkSource::FollowedChannels);
                    self.replace_followed_channels(channels);
                    self.sync_extension_channels();
                    match live_status_failure {
                        TwitchUpdateFailure::Retryable(message) => {
                            self.note_twitch_network_unavailable(
                                TwitchNetworkSource::LiveStatus,
                                message,
                            );
                        }
                        TwitchUpdateFailure::Warning(message) => {
                            self.connection_error = Some(message);
                        }
                    }
                }

                TwitchUpdate::LiveStatus {
                    generation,
                    live_channel_ids,
                } if generation == self.connection_generation => {
                    self.note_twitch_network_online(TwitchNetworkSource::LiveStatus);
                    self.twitch_baseline_confirmed_at = Some(Instant::now());
                    self.streak_completed
                        .retain(|channel_id, _| live_channel_ids.contains(channel_id));
                    self.notified_streak_channels
                        .retain(|channel_id| live_channel_ids.contains(channel_id));
                    self.live_channel_ids = live_channel_ids;
                    self.clear_offline_raid_redirects();
                }
                TwitchUpdate::PubSubStatus {
                    generation,
                    connected,
                    detail,
                } if generation == self.connection_generation => {
                    let now = Instant::now();
                    if connected {
                        self.pubsub_state_confirmed_at = Some(now);
                    } else {
                        self.note_pubsub_disconnection(now);
                    }
                    if connected && !self.pubsub_connected {
                        self.available_claim_channels.clear();
                    }
                    self.pubsub_connected = connected;
                    self.pubsub_error = detail;
                }
                TwitchUpdate::NetworkUnavailable {
                    generation,
                    source,
                    message,
                } if generation == self.connection_generation => {
                    self.note_twitch_network_unavailable(source, message);
                }
                TwitchUpdate::StreakCompleted {
                    generation,
                    channel_id,
                    points_earned,
                } if generation == self.connection_generation => {
                    self.pubsub_last_event_at = Some(Instant::now());
                    if let Some(channel_name) = self
                        .channels
                        .iter()
                        .find(|channel| channel.id == channel_id)
                        .map(|channel| channel.display_name.clone())
                    {
                        if self
                            .streak_completed
                            .insert(channel_id.clone(), points_earned)
                            .is_none()
                        {
                            self.queue_planner
                                .record_streak_completed(channel_id.clone());
                        }
                        if self.notified_streak_channels.insert(channel_id) {
                            record_activity(
                                &mut self.activity_log,
                                ActivityLogKind::WatchStreak,
                                channel_name,
                                format!("Watch Streak completed; reward: {points_earned} points."),
                            );
                        }
                    }
                }
                TwitchUpdate::PointsEarned {
                    generation,
                    channel_id,
                    points_earned,
                    balance,
                    reason_code,
                } if generation == self.connection_generation => {
                    self.pubsub_last_event_at = Some(Instant::now());
                    let Some(channel_name) = self
                        .channels
                        .iter()
                        .find(|channel| channel.id == channel_id)
                        .map(|channel| channel.display_name.clone())
                    else {
                        continue;
                    };
                    let balance_changed = self
                        .channel_points_balances
                        .insert(channel_id.clone(), balance)
                        != Some(balance);
                    if reason_code == "CLAIM" {
                        self.available_claim_channels.remove(&channel_id);
                    }
                    if balance_changed && reason_code != "WATCH_STREAK" {
                        record_activity(&mut self.activity_log,
                            ActivityLogKind::PointsEarned,
                            channel_name,
                            format!(
                                "Earned {points_earned} points ({reason_code}); new balance: {balance}."
                            ),
                        );
                    }
                }
                TwitchUpdate::PointsSpent {
                    generation,
                    channel_id,
                    balance,
                } if generation == self.connection_generation => {
                    self.pubsub_last_event_at = Some(Instant::now());
                    let Some(channel_name) = self
                        .channels
                        .iter()
                        .find(|channel| channel.id == channel_id)
                        .map(|channel| channel.display_name.clone())
                    else {
                        continue;
                    };
                    if self.channel_points_balances.insert(channel_id, balance) != Some(balance) {
                        record_activity(
                            &mut self.activity_log,
                            ActivityLogKind::PointsSpent,
                            channel_name,
                            format!("Points spent; new balance: {balance}."),
                        );
                    }
                }
                TwitchUpdate::ClaimAvailable {
                    generation,
                    channel_id,
                } if generation == self.connection_generation => {
                    self.pubsub_last_event_at = Some(Instant::now());
                    let Some(channel_name) = self
                        .channels
                        .iter()
                        .find(|channel| channel.id == channel_id)
                        .map(|channel| channel.display_name.clone())
                    else {
                        continue;
                    };
                    record_claim_available_once(
                        &mut self.available_claim_channels,
                        &mut self.activity_log,
                        channel_id,
                        channel_name,
                    );
                }
                TwitchUpdate::Warning {
                    generation,
                    message,
                } if generation == self.connection_generation => {
                    self.connection_error = Some(message);
                }
                TwitchUpdate::Failed {
                    generation,
                    message,
                } if generation == self.connection_generation => {
                    self.connection_generation = self.connection_generation.wrapping_add(1);
                    if let Some(bridge) = &self.browser_bridge {
                        bridge.set_recovery_generation(self.connection_generation);
                    }
                    self.twitch_network_errors.clear();
                    self.twitch_network_unavailable = false;
                    self.reload_managed_player_tabs_pending = false;
                    self.pending_manual_open = false;
                    self.pending_manual_open_inventory_wait_since = None;
                    self.pending_reload_command = None;
                    self.pending_manual_open_command = None;
                    self.note_pubsub_disconnection(Instant::now());
                    self.twitch_cancel = None;
                    self.connection_status = TwitchConnectionStatus::Disconnected;
                    self.live_channel_ids.clear();
                    self.pubsub_connected = false;
                    self.pubsub_error = None;
                    self.streak_completed.clear();
                    self.notified_streak_channels.clear();
                    self.queue_planner = QueuePlanner::default();
                    self.points_rotation = StreamRotation::default();
                    self.streak_rotation = StreamRotation::default();
                    let close_ids = self
                        .auto_open_channels
                        .difference(&self.manual_open_channels)
                        .cloned()
                        .collect::<Vec<_>>();
                    self.auto_open_channels.clear();
                    self.pending_stream_switch = None;
                    self.auto_open_retry_after.clear();
                    if !close_ids.is_empty() {
                        self.send_bridge_command(BrowserCommand::CloseChannels {
                            channel_ids: close_ids,
                        });
                    }
                    self.connection_error = Some(message);
                }
                _ => {}
            }
        }
    }
}
