use super::*;

impl Dashboard {
    pub(super) fn start_twitch_connection(&mut self) {
        let account_id = self.selected_account_id.clone();
        self.start_twitch_connection_for(account_id, false);
    }

    pub(super) fn add_twitch_account(&mut self) {
        self.start_twitch_connection_for(None, true);
    }

    pub(super) fn select_twitch_account(&mut self, account_id: String) {
        if self.selected_account_id.as_deref() == Some(account_id.as_str()) {
            if matches!(
                &self.connection_status,
                TwitchConnectionStatus::Disconnected
            ) {
                self.start_twitch_connection();
            }
            return;
        }
        self.start_twitch_connection_for(Some(account_id), false);
    }

    fn start_twitch_connection_for(
        &mut self,
        account_id: Option<String>,
        force_authorization: bool,
    ) {
        let switching_account =
            force_authorization || account_id.as_deref() != self.selected_account_id.as_deref();
        if switching_account
            && (!matches!(
                &self.connection_status,
                TwitchConnectionStatus::Disconnected
            ) || self.twitch_cancel.is_some())
        {
            self.disconnect_twitch();
        }
        if let Some(account_id) = account_id.as_deref() {
            self.select_followed_channel_context(account_id);
            self.selected_account_id = Some(account_id.to_owned());
        }
        if let Some(cancel) = self.twitch_cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        let now = Instant::now();
        if self.points_events_connected {
            self.note_points_events_disconnection(now);
        } else {
            self.note_points_events_gap(now);
        }
        self.connection_generation = self.connection_generation.wrapping_add(1);
        if let Some(bridge) = &self.browser_bridge {
            bridge.set_recovery_generation(self.connection_generation);
        }
        self.reset_twitch_account_runtime();
        self.connection_status = TwitchConnectionStatus::Connecting;
        self.connection_error = None;
        let generation = self.connection_generation;
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let updates = self.twitch_sender.clone();
        self.twitch_cancel = Some(cancel);

        if let Err(error) = thread::Builder::new()
            .name("twitch-connection".to_owned())
            .spawn(move || {
                twitch_api::connection::run_connection(
                    worker_cancel,
                    generation,
                    updates,
                    account_id,
                    force_authorization,
                )
            })
        {
            self.twitch_cancel = None;
            self.connection_status = TwitchConnectionStatus::Disconnected;
            self.connection_error = Some(format!("Could not start Twitch connection: {error}"));
        }
    }

    pub(super) fn select_followed_channel_context(&mut self, account_id: &str) {
        if self.followed_channels_account_id.as_deref() == Some(account_id) {
            return;
        }
        let previous_account_id = self.followed_channels_account_id.clone();
        if let Some(previous_account_id) = &previous_account_id {
            self.followed_channels_by_account
                .insert(previous_account_id.clone(), self.channels.clone());
        }
        self.channels = match self.followed_channels_by_account.get(account_id) {
            Some(channels) => channels.clone(),
            None if previous_account_id.is_none() => self.channels.clone(),
            None => Vec::new(),
        };
        self.followed_channels_account_id = Some(account_id.to_owned());
    }

    fn reset_twitch_account_runtime(&mut self) {
        self.twitch_network_unavailable = false;
        self.twitch_network_errors.clear();
        self.reload_managed_player_tabs_pending = false;
        self.pending_manual_open = false;
        self.pending_manual_open_inventory_wait_since = None;
        self.pending_reload_command = None;
        self.pending_manual_open_command = None;
        self.points_events_connected = false;
        self.points_events_error = None;
        self.twitch_user_id = None;
        self.twitch_baseline_confirmed_at = None;
        self.points_events_state_confirmed_at = None;
        self.points_events_last_event_at = None;
        self.points_events_missed_events_since = None;
        self.live_channel_ids.clear();
        self.streak_completed.clear();
        self.notified_streak_channels.clear();
        self.channel_points_balances.clear();
        self.available_claim_channels.clear();
        self.queue_planner = QueuePlanner::default();
        self.points_rotation = StreamRotation::default();
        self.streak_rotation = StreamRotation::default();
        self.pending_stream_switch = None;
        self.auto_open_retry_after.clear();
    }

    pub(super) fn forget_twitch_account(&mut self, account_id: &str) {
        let was_selected = self.selected_account_id.as_deref() == Some(account_id);
        if was_selected {
            self.disconnect_twitch();
        }
        if let Err(error) = twitch_api::credential_store::remove_saved_account_token(account_id) {
            self.connection_error = Some(error);
            return;
        }
        self.forgotten_twitch_account_ids
            .insert(account_id.to_owned());
        self.twitch_accounts
            .retain(|account| account.id != account_id);
        self.followed_channels_by_account.remove(account_id);
        if self.followed_channels_account_id.as_deref() == Some(account_id) {
            self.followed_channels_account_id = None;
            self.channels.clear();
        }
        if was_selected {
            self.selected_account_id = self
                .twitch_accounts
                .first()
                .map(|account| account.id.clone());
            if self.selected_account_id.is_some() {
                self.start_twitch_connection();
            }
        }
    }

    pub(super) fn disconnect_twitch(&mut self) {
        if let Some(cancel) = self.twitch_cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        self.note_points_events_disconnection(Instant::now());
        self.connection_generation = self.connection_generation.wrapping_add(1);
        if let Some(bridge) = &self.browser_bridge {
            bridge.set_recovery_generation(self.connection_generation);
        }
        self.connection_status = TwitchConnectionStatus::Disconnected;
        let close_ids = self
            .auto_open_channels
            .difference(&self.manual_open_channels)
            .cloned()
            .collect::<Vec<_>>();
        self.auto_open_channels.clear();
        self.reset_twitch_account_runtime();
        if !close_ids.is_empty() {
            self.send_bridge_command(BrowserCommand::CloseChannels {
                channel_ids: close_ids,
            });
        }
        self.connection_error = None;
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
        self.note_points_events_disconnection(Instant::now());
        self.reset_twitch_account_runtime();
        self.connection_error = None;
    }
}
