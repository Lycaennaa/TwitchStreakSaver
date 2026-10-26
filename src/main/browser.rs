use super::*;
#[derive(Clone, Copy)]
enum BrowserTabUpdate {
    Opened,
    Status {
        player: Option<PlayerState>,
        chat: Option<ChatState>,
    },
}
const STREAM_LIMIT_EXTENSION_ERROR: &str =
    "Reload the browser extension (version 0.3.6 or newer) to enforce the two-stream limit.";

const MANUAL_OPEN_INVENTORY_TIMEOUT: Duration = Duration::from_secs(10);
const MANUAL_OPEN_INVENTORY_ERROR: &str =
    "Browser extension is connected but has not reported Twitch-tab inventory. Reload or update the extension.";
const COMMAND_ACK_REQUIRED_EXTENSION_ERROR: &str =
    "Reload the browser extension (version 0.3.18 or newer) to enable reliable stream recovery.";
const OPEN_SELECTED_NO_CHANNELS_ERROR: &str = "Select followed Twitch channels to open.";
const OPEN_SELECTED_NO_STREAM_SLOTS_ERROR: &str =
    "No stream slots are available after counting existing Twitch tabs.";
const BRIDGE_COMMAND_ACK_TIMEOUT: Duration = Duration::from_secs(10);

fn browser_bridge_retry_due(retry_after: Option<Instant>, now: Instant) -> bool {
    retry_after.is_some_and(|retry_after| now >= retry_after)
}

#[cfg(test)]
mod retry_tests {
    use super::browser_bridge_retry_due;
    use std::time::{Duration, Instant};

    #[test]
    fn browser_bridge_retries_only_after_the_scheduled_deadline() {
        let now = Instant::now();
        let retry_after = now + Duration::from_secs(5);

        assert!(!browser_bridge_retry_due(Some(retry_after), now));
        assert!(browser_bridge_retry_due(Some(retry_after), retry_after));
        assert!(!browser_bridge_retry_due(None, now));
    }
}
impl Dashboard {
    pub(super) fn allocate_browser_command_id(&mut self) -> String {
        let request_id = format!(
            "{}-{}",
            self.browser_command_session_id, self.next_browser_command_id
        );
        self.next_browser_command_id = self.next_browser_command_id.wrapping_add(1).max(1);
        request_id
    }

    pub(super) fn poll_browser_bridge(&mut self) {
        let now = Instant::now();
        if self.browser_bridge.is_none()
            && browser_bridge_retry_due(self.browser_bridge_retry_after, now)
        {
            self.retry_browser_bridge();
        }
        let mut events = Vec::new();
        if let Some(bridge) = &self.browser_bridge {
            while let Ok(event) = bridge.try_recv() {
                events.push(event);
            }
        }
        for event in events {
            match event {
                BridgeEvent::Connected => {
                    self.requeue_unacknowledged_browser_commands();
                    self.pending_stream_switch = None;
                    self.browser_connected = true;
                    self.browser_command_ack_supported = false;
                    self.browser_launch_attempted = None;
                    self.external_stream_counts.clear();
                    self.external_unmatched_streams = 0;
                    self.external_inventory_received = false;
                    self.pending_manual_open_inventory_wait_since = None;
                    self.auto_open_retry_after.clear();
                    self.browser_error = None;
                    self.browser_notice = None;
                    self.send_bridge_command(BrowserCommand::BridgeReady);
                }
                BridgeEvent::Disconnected => {
                    self.requeue_unacknowledged_browser_commands();
                    self.pending_stream_switch = None;
                    self.browser_connected = false;
                    self.browser_command_ack_supported = false;
                    self.external_stream_counts.clear();
                    self.external_unmatched_streams = 0;
                    self.external_inventory_received = false;
                    self.pending_manual_open_inventory_wait_since = None;
                    self.browser_states.clear();
                    self.browser_notice = Some("Browser extension disconnected.".to_owned());
                    self.points_rotation
                        .pause_active_sessions(self.started_at.elapsed());
                    self.streak_rotation
                        .pause_active_sessions(self.started_at.elapsed());
                }
                BridgeEvent::Message(BrowserMessage::Known(message)) => {
                    self.handle_browser_message(message);
                }
                BridgeEvent::Message(BrowserMessage::Unknown(_)) => {}
                BridgeEvent::CommandFailed {
                    command,
                    recovery_generation,
                    message,
                } => {
                    self.handle_failed_bridge_command(command, recovery_generation, message);
                }

                BridgeEvent::Failed(message) => {
                    self.pending_stream_switch = None;
                    self.browser_error = Some(message);
                }
            }
        }
    }

    pub(super) fn requeue_unacknowledged_browser_commands(&mut self) {
        let connection_generation = self.connection_generation;
        if self
            .pending_reload_command
            .as_ref()
            .is_some_and(|pending| pending.connection_generation != connection_generation)
        {
            self.pending_reload_command = None;
        }
        if let Some(pending) = self.pending_reload_command.as_mut() {
            pending.sent_at = None;
            self.reload_managed_player_tabs_pending = true;
        }
        let manual_command_is_current = self
            .pending_manual_open_command
            .as_ref()
            .is_some_and(|pending| pending.connection_generation == connection_generation);
        if manual_command_is_current {
            if let Some(pending) = self.pending_manual_open_command.as_mut() {
                pending.sent_at = None;
            }
            self.pending_manual_open = true;
        } else if self.pending_manual_open_command.is_some() {
            self.pending_manual_open_command = None;
        }
    }

    pub(super) fn handle_failed_bridge_command(
        &mut self,
        command: BrowserCommand,
        recovery_generation: Option<u64>,
        message: String,
    ) {
        self.browser_error = Some(message);
        self.browser_connected = false;
        self.external_inventory_received = false;
        self.pending_manual_open_inventory_wait_since = None;
        if recovery_generation != Some(self.connection_generation)
            || !matches!(
                &self.connection_status,
                TwitchConnectionStatus::Connected(_)
            )
        {
            return;
        }
        match command {
            BrowserCommand::ReloadManagedPlayerTabs { command_id } => {
                if let Some(pending) = self.pending_reload_command.as_mut() {
                    if pending.request_id == command_id
                        && pending.connection_generation == self.connection_generation
                    {
                        pending.sent_at = None;
                        self.reload_managed_player_tabs_pending = true;
                    }
                }
            }
            BrowserCommand::OpenSelected {
                command_id,
                manual_request: true,
                ..
            } => {
                if self
                    .pending_manual_open_command
                    .as_ref()
                    .is_some_and(|pending| {
                        pending.request_id == command_id
                            && pending.connection_generation == self.connection_generation
                    })
                {
                    if let Some(pending) = self.pending_manual_open_command.as_mut() {
                        pending.sent_at = None;
                    }
                    self.pending_manual_open = true;
                    self.browser_notice = Some(
                        "Open request will retry after the browser extension reconnects."
                            .to_owned(),
                    );
                }
            }
            _ => {}
        }
    }
    pub(super) fn retry_browser_bridge(&mut self) -> bool {
        if self.browser_bridge.is_some() {
            return true;
        }
        match BrowserBridge::start() {
            Ok(bridge) => {
                bridge.set_recovery_generation(self.connection_generation);
                self.browser_bridge = Some(bridge);
                self.browser_bridge_retry_after = None;
                self.browser_error = None;
                self.browser_notice = Some(
                    "App bridge is ready; waiting for the browser extension to pair.".to_owned(),
                );
                true
            }
            Err(error) => {
                self.browser_bridge_retry_after = Some(Instant::now() + BROWSER_BRIDGE_RETRY_DELAY);
                self.browser_error = Some(error.clone());
                self.browser_notice = Some(format!(
                    "Browser bridge could not start; it will retry automatically: {error}"
                ));
                false
            }
        }
    }

    fn acknowledge_browser_command(&mut self, request_id: String) {
        let connection_generation = self.connection_generation;
        if self.pending_reload_command.as_ref().is_some_and(|pending| {
            pending.request_id == request_id
                && pending.connection_generation == connection_generation
        }) {
            self.pending_reload_command = None;
            self.reload_managed_player_tabs_pending = false;
        }
        if self
            .pending_manual_open_command
            .as_ref()
            .is_some_and(|pending| {
                pending.request_id == request_id
                    && pending.connection_generation == connection_generation
            })
        {
            self.pending_manual_open_command = None;
            self.pending_manual_open = false;
            self.pending_manual_open_inventory_wait_since = None;
        }
    }

    pub(super) fn handle_browser_message(&mut self, message: KnownBrowserMessage) {
        match message {
            KnownBrowserMessage::CommandAcknowledged { command_id } => {
                self.acknowledge_browser_command(command_id);
            }
            KnownBrowserMessage::PopupAction { request_id, action } => {
                let (success, message) = self
                    .apply_popup_action_with(action, |dashboard, command| {
                        dashboard.send_bridge_command(command)
                    });
                self.send_bridge_command(BrowserCommand::PopupActionResult {
                    request_id,
                    success,
                    message,
                });
            }
            KnownBrowserMessage::ExternalStreams {
                channel_counts,
                unmatched_count,
                managed_channel_ids,
                raid_redirected_channel_ids,
            } => {
                let known_channel_ids = self
                    .channels
                    .iter()
                    .map(|channel| channel.id.as_str())
                    .collect::<HashSet<_>>();
                self.external_stream_counts = channel_counts
                    .into_iter()
                    .filter(|(channel_id, count)| {
                        *count > 0 && known_channel_ids.contains(channel_id.as_str())
                    })
                    .collect();
                self.external_unmatched_streams = unmatched_count;
                if let Some(channel_ids) = raid_redirected_channel_ids {
                    self.raid_redirected_channels = channel_ids.into_iter().collect();
                }
                if let Some(managed_channel_ids) = managed_channel_ids {
                    if !self.external_inventory_received {
                        restore_managed_channel_ownership(
                            managed_channel_ids,
                            &mut self.auto_open_channels,
                            &mut self.manual_open_channels,
                        );
                        let auto_open_channels = &self.auto_open_channels;
                        self.auto_open_retry_after
                            .retain(|channel_id, _| auto_open_channels.contains(channel_id));
                    }
                    self.external_inventory_received = true;
                    self.pending_manual_open_inventory_wait_since = None;
                    if self.browser_error.as_deref() == Some(STREAM_LIMIT_EXTENSION_ERROR)
                        || self.browser_error.as_deref() == Some(MANUAL_OPEN_INVENTORY_ERROR)
                    {
                        self.browser_error = None;
                    }
                } else {
                    self.external_inventory_received = false;
                    self.browser_error = Some(STREAM_LIMIT_EXTENSION_ERROR.to_owned());
                }
                self.clear_offline_raid_redirects();
            }
            KnownBrowserMessage::Ready {
                command_acknowledgments,
            } => {
                self.browser_command_ack_supported = command_acknowledgments;
                if command_acknowledgments {
                    if self.browser_error.as_deref() == Some(COMMAND_ACK_REQUIRED_EXTENSION_ERROR) {
                        self.browser_error = None;
                    }
                } else {
                    self.external_inventory_received = false;
                    self.browser_error = Some(COMMAND_ACK_REQUIRED_EXTENSION_ERROR.to_owned());
                }
                if command_acknowledgments {
                    self.sync_extension_channels();
                    self.set_extension_preferred_quality();
                }
            }
            KnownBrowserMessage::TabOpened {
                channel_id,
                tab_id,
                surface,
            } => {
                let opening_streamer = if surface == BrowserSurface::Player
                    && tab_id > 0
                    && self
                        .browser_states
                        .get(&channel_id)
                        .and_then(|state| state.player_tab_id)
                        .is_none()
                    && (self.manual_open_channels.contains(&channel_id)
                        || self.auto_open_channels.contains(&channel_id)
                        || self
                            .pending_stream_switch
                            .as_ref()
                            .is_some_and(|pending| pending.is_awaiting_open(&channel_id)))
                {
                    self.channels
                        .iter()
                        .find(|channel| channel.id == channel_id)
                        .map(|channel| channel.display_name.clone())
                } else {
                    None
                };
                let accepted = self.handle_browser_tab(
                    channel_id.clone(),
                    tab_id,
                    surface,
                    BrowserTabUpdate::Opened,
                );
                let pending_open = !channel_id.is_empty()
                    && tab_id > 0
                    && self
                        .pending_stream_switch
                        .as_ref()
                        .is_some_and(|pending| pending.is_awaiting_open(&channel_id));
                if let Some(streamer) = opening_streamer {
                    record_activity(
                        &mut self.activity_log,
                        ActivityLogKind::StreamOpened,
                        streamer.clone(),
                        format!("Opening {streamer}."),
                    );
                }
                if surface == BrowserSurface::Player && (accepted || pending_open) {
                    if let Some(pending) = &mut self.pending_stream_switch {
                        pending.confirm_opened(&channel_id);
                    }
                    self.finish_pending_stream_switch();
                }
            }
            KnownBrowserMessage::TabAdopted {
                channel_id,
                tab_id,
                surface,
            } => {
                let is_player = surface == BrowserSurface::Player;
                let accepted = self.handle_browser_tab(
                    channel_id.clone(),
                    tab_id,
                    surface,
                    BrowserTabUpdate::Opened,
                );
                if accepted && is_player && !channel_id.is_empty() {
                    if !self.auto_open_channels.contains(&channel_id) {
                        self.manual_open_channels.insert(channel_id.clone());
                    }
                    if let Some(pending) = self.pending_stream_switch.as_mut() {
                        pending.confirm_opened(&channel_id);
                    }
                    self.finish_pending_stream_switch();
                }
            }
            KnownBrowserMessage::ChannelStatus {
                channel_id,
                tab_id,
                surface,
                player_state,
                chat_state,
                chat_message_author_login,
            } => {
                let chat_message_from_rust_user = match (
                    &self.connection_status,
                    chat_message_author_login.as_deref(),
                ) {
                    (TwitchConnectionStatus::Connected(login), Some(author_login)) => {
                        login.eq_ignore_ascii_case(author_login)
                    }
                    _ => false,
                };
                if chat_message_from_rust_user && self.automation_enabled {
                    self.points_rotation.exclude_channel(channel_id.clone());
                    self.streak_rotation.exclude_channel(channel_id.clone());
                }

                self.handle_browser_tab(
                    channel_id,
                    tab_id,
                    surface,
                    BrowserTabUpdate::Status {
                        player: player_state,
                        chat: chat_state,
                    },
                );
            }
            KnownBrowserMessage::TabClosed {
                channel_id,
                tab_id,
                surface,
                close_reason,
            } => {
                let player_tab_closed = surface == BrowserSurface::Player
                    && !channel_id.is_empty()
                    && tab_id > 0
                    && self
                        .pending_stream_switch
                        .as_ref()
                        .is_some_and(|pending| pending.is_awaiting_close(&channel_id))
                    && self
                        .browser_states
                        .get(&channel_id)
                        .map_or(true, |state| state.player_tab_id == Some(tab_id));
                let pending_tab_open = surface == BrowserSurface::Player
                    && !channel_id.is_empty()
                    && tab_id > 0
                    && self
                        .pending_stream_switch
                        .as_ref()
                        .is_some_and(|pending| pending.is_awaiting_open(&channel_id));
                let closed = handle_closed_browser_tab(
                    channel_id.clone(),
                    tab_id,
                    surface,
                    &mut self.browser_states,
                    &mut self.manual_open_channels,
                    &mut self.auto_open_channels,
                    &mut self.auto_open_retry_after,
                );
                let raid_redirected = closed
                    && surface == BrowserSurface::Player
                    && close_reason == Some(BrowserTabCloseReason::RaidRedirect);
                if raid_redirected {
                    self.raid_redirected_channels.insert(channel_id.clone());
                    if pending_tab_open {
                        if let Some(pending) = &mut self.pending_stream_switch {
                            pending.abandon_open(&channel_id);
                            pending.desired_channels.remove(&channel_id);
                        }
                    }
                    self.clear_offline_raid_redirects();
                }
                if player_tab_closed {
                    if let Some(pending) = &mut self.pending_stream_switch {
                        pending.confirm_closed(&channel_id);
                    }
                }
                if player_tab_closed || (raid_redirected && pending_tab_open) {
                    self.finish_pending_stream_switch();
                }
            }
            KnownBrowserMessage::TabReleased {
                channel_id,
                tab_id,
                surface,
            } => {
                let should_remove_state =
                    if let Some(state) = self.browser_states.get_mut(&channel_id) {
                        match surface {
                            BrowserSurface::Player if state.player_tab_id == Some(tab_id) => {
                                state.player_tab_id = None;
                                state.player_state = None;
                                state.player_last_seen = None;
                            }
                            BrowserSurface::Chat if state.chat_tab_id == Some(tab_id) => {
                                state.chat_tab_id = None;
                                state.chat_state = None;
                                state.chat_last_seen = None;
                            }
                            _ => {}
                        }
                        state.player_tab_id.is_none() && state.chat_tab_id.is_none()
                    } else {
                        false
                    };
                if should_remove_state {
                    self.browser_states.remove(&channel_id);
                }
                self.manual_open_channels.remove(&channel_id);
                self.auto_open_retry_after.remove(&channel_id);
                if self
                    .pending_stream_switch
                    .as_ref()
                    .is_some_and(|pending| pending.channels.contains_key(&channel_id))
                {
                    self.pending_stream_switch = None;
                }
            }
            KnownBrowserMessage::Error {
                channel_id,
                message,
                close_reason,
            } => {
                if close_reason == Some(BrowserTabCloseReason::RaidRedirect) {
                    let pending_state = channel_id.as_ref().and_then(|channel_id| {
                        self.pending_stream_switch
                            .as_ref()
                            .and_then(|pending| pending.channels.get(channel_id).copied())
                    });
                    let pending_tab_open =
                        matches!(pending_state, Some(PendingStreamChannelState::Opening));
                    let pending_tab_closing =
                        matches!(pending_state, Some(PendingStreamChannelState::Closing));
                    if let Some(channel_id) = channel_id {
                        self.raid_redirected_channels.insert(channel_id.clone());
                        if pending_tab_closing {
                            schedule_auto_open_retry(
                                &self.auto_open_channels,
                                &mut self.auto_open_retry_after,
                                &channel_id,
                                Instant::now(),
                            );
                        } else {
                            self.auto_open_retry_after.remove(&channel_id);
                            if pending_tab_open {
                                if let Some(pending) = &mut self.pending_stream_switch {
                                    pending.abandon_open(&channel_id);
                                    pending.desired_channels.remove(&channel_id);
                                }
                            }
                        }
                    }
                    if pending_tab_open {
                        self.finish_pending_stream_switch();
                    }
                    self.clear_offline_raid_redirects();
                } else if let Some(channel_id) = channel_id {
                    schedule_auto_open_retry(
                        &self.auto_open_channels,
                        &mut self.auto_open_retry_after,
                        &channel_id,
                        Instant::now(),
                    );
                } else if let Some(pending) = &self.pending_stream_switch {
                    let now = Instant::now();
                    for (channel_id, state) in &pending.channels {
                        if matches!(
                            *state,
                            PendingStreamChannelState::Closing | PendingStreamChannelState::Opening
                        ) {
                            schedule_auto_open_retry(
                                &self.auto_open_channels,
                                &mut self.auto_open_retry_after,
                                channel_id,
                                now,
                            );
                        }
                    }
                }
                if let Some(message) = message {
                    self.browser_error = Some(message.chars().take(240).collect());
                }
            }
        }
    }

    fn handle_browser_tab(
        &mut self,
        channel_id: String,
        tab_id: i64,
        surface: BrowserSurface,
        update: BrowserTabUpdate,
    ) -> bool {
        if channel_id.is_empty()
            || tab_id <= 0
            || !self.channels.iter().any(|channel| channel.id == channel_id)
        {
            return false;
        }
        if matches!(update, BrowserTabUpdate::Opened) {
            self.auto_open_retry_after.remove(&channel_id);
        }
        let state = self.browser_states.entry(channel_id).or_default();
        match surface {
            BrowserSurface::Player => {
                if state.player_tab_id != Some(tab_id) {
                    state.player_state = None;
                    state.player_last_seen = None;
                }
                state.player_tab_id = Some(tab_id);
                if let BrowserTabUpdate::Status { player, chat } = update {
                    if let Some(player_state) = player {
                        state.player_state = Some(player_state);
                        state.player_last_seen = Some(Instant::now());
                    }
                    if let Some(chat_state) = chat {
                        state.chat_tab_id = Some(tab_id);
                        state.chat_state = Some(chat_state);
                        state.chat_last_seen = Some(Instant::now());
                    }
                }
            }
            BrowserSurface::Chat => {
                if state.chat_tab_id != Some(tab_id) {
                    state.chat_state = None;
                    state.chat_last_seen = None;
                }
                state.chat_tab_id = Some(tab_id);
                if let BrowserTabUpdate::Status { chat, .. } = update {
                    if let Some(chat_state) = chat {
                        state.chat_state = Some(chat_state);
                        state.chat_last_seen = Some(Instant::now());
                    }
                }
            }
        }
        true
    }

    pub(super) fn finish_pending_stream_switch(&mut self) {
        let complete = self
            .pending_stream_switch
            .as_ref()
            .is_some_and(PendingStreamSwitch::is_complete);
        if complete {
            if let Some(pending) = self.pending_stream_switch.take() {
                let successful = pending.is_successful();
                let PendingStreamSwitch {
                    streamer,
                    event,
                    desired_channels,
                    channels,
                } = pending;
                self.auto_open_channels = desired_channels;
                self.auto_open_channels.retain(|channel_id| {
                    self.channels
                        .iter()
                        .any(|channel| &channel.id == channel_id)
                });
                for channel_id in channels.keys() {
                    self.auto_open_retry_after.remove(channel_id);
                }
                if successful {
                    record_activity(
                        &mut self.activity_log,
                        ActivityLogKind::StreamSwitch,
                        streamer,
                        event,
                    );
                }
            }
        }
    }
    pub(super) fn service_network_recovery(&mut self) {
        let stale_request = self
            .pending_reload_command
            .as_ref()
            .is_some_and(|pending| pending.connection_generation != self.connection_generation);
        if stale_request {
            self.pending_reload_command = None;
        }
        if self.pending_reload_command.as_ref().is_some_and(|pending| {
            pending
                .sent_at
                .is_some_and(|sent_at| sent_at.elapsed() < BRIDGE_COMMAND_ACK_TIMEOUT)
        }) {
            return;
        }
        if !self.reload_managed_player_tabs_pending
            || !self.browser_command_ack_supported
            || self.twitch_network_unavailable
            || !matches!(
                &self.connection_status,
                TwitchConnectionStatus::Connected(_)
            )
            || self.connection_error.is_some()
            || !self.browser_connected
            || self.browser_error.is_some()
            || !self.external_inventory_received
        {
            return;
        }
        let request_id = self
            .pending_reload_command
            .as_ref()
            .map(|pending| pending.request_id.clone())
            .unwrap_or_else(|| self.allocate_browser_command_id());
        let connection_generation = self.connection_generation;
        let sent = self.send_bridge_command(BrowserCommand::ReloadManagedPlayerTabs {
            command_id: request_id.clone(),
        });
        self.pending_reload_command = Some(PendingBridgeCommand {
            request_id,
            connection_generation,
            sent_at: sent.then(Instant::now),
            command: None,
        });
    }

    pub(super) fn service_pending_manual_open(&mut self) {
        self.service_pending_manual_open_with(|dashboard, command| {
            dashboard.send_bridge_command(command)
        });
    }

    pub(super) fn service_pending_manual_open_with(
        &mut self,
        send: impl FnMut(&mut Self, BrowserCommand) -> bool,
    ) {
        if let Some(pending) = self.pending_manual_open_command.clone() {
            let current_generation = pending.connection_generation == self.connection_generation;
            if current_generation
                && pending
                    .sent_at
                    .is_some_and(|sent_at| sent_at.elapsed() < BRIDGE_COMMAND_ACK_TIMEOUT)
            {
                return;
            }
            if current_generation {
                if let Some(pending) = self.pending_manual_open_command.as_mut() {
                    pending.sent_at = None;
                }
                self.pending_manual_open = true;
            } else {
                self.pending_manual_open_command = None;
            }
        }
        if !self.pending_manual_open {
            return;
        }
        let twitch_ready = !self.twitch_network_unavailable
            && self.connection_error.is_none()
            && matches!(
                &self.connection_status,
                TwitchConnectionStatus::Connected(_)
            );
        if !self.browser_command_ack_supported
            || !self.browser_connected
            || self.browser_error.is_some()
        {
            self.pending_manual_open_inventory_wait_since = None;
            return;
        }
        if !self.external_inventory_received {
            if twitch_ready {
                let waiting_since = self
                    .pending_manual_open_inventory_wait_since
                    .get_or_insert_with(Instant::now);
                if waiting_since.elapsed() >= MANUAL_OPEN_INVENTORY_TIMEOUT
                    && self.browser_error.is_none()
                {
                    self.browser_error = Some(MANUAL_OPEN_INVENTORY_ERROR.to_owned());
                }
            } else {
                self.pending_manual_open_inventory_wait_since = None;
            }
            return;
        }
        self.pending_manual_open_inventory_wait_since = None;
        if !twitch_ready {
            return;
        }
        self.pending_manual_open = false;
        self.open_selected_twitch_channels_with(send);
    }

    pub(super) fn send_bridge_command(&mut self, command: BrowserCommand) -> bool {
        if !self.browser_command_ack_supported
            && matches!(
                &command,
                BrowserCommand::OpenSelected { .. }
                    | BrowserCommand::ReloadManagedPlayerTabs { .. }
            )
        {
            self.browser_error = Some(COMMAND_ACK_REQUIRED_EXTENSION_ERROR.to_owned());
            return false;
        }
        if !self.browser_connected {
            if self.browser_error.as_deref() != Some(COMMAND_ACK_REQUIRED_EXTENSION_ERROR) {
                self.browser_error = Some("Browser extension is not connected.".to_owned());
            }
            return false;
        }
        if let Some(bridge) = &self.browser_bridge {
            let result = match command {
                BrowserCommand::OpenSelected {
                    wake_browser,
                    channels,
                    command_id,
                    manual_request,
                } => bridge.send_recoverable(
                    BrowserCommand::OpenSelected {
                        wake_browser,
                        channels,
                        command_id,
                        manual_request,
                    },
                    self.connection_generation,
                ),
                BrowserCommand::ReloadManagedPlayerTabs { command_id } => bridge.send_recoverable(
                    BrowserCommand::ReloadManagedPlayerTabs { command_id },
                    self.connection_generation,
                ),
                command => bridge.send(command),
            };
            match result {
                Ok(()) => {
                    if self.browser_error.as_deref() != Some(COMMAND_ACK_REQUIRED_EXTENSION_ERROR) {
                        self.browser_error = None;
                    }
                    true
                }
                Err(error) => {
                    if self.browser_error.as_deref() != Some(COMMAND_ACK_REQUIRED_EXTENSION_ERROR) {
                        self.browser_error = Some(error);
                    }
                    false
                }
            }
        } else {
            if self.browser_error.as_deref() != Some(COMMAND_ACK_REQUIRED_EXTENSION_ERROR) {
                self.browser_error = Some("Browser bridge listener is unavailable.".to_owned());
            }
            false
        }
    }

    pub(super) fn sync_extension_channels(&mut self) {
        if !self.browser_connected {
            return;
        }
        self.external_inventory_received = false;
        let channels = self
            .channels
            .iter()
            .filter(|channel| !channel.login.is_empty())
            .map(BrowserChannel::from)
            .collect();
        self.send_bridge_command(BrowserCommand::SyncChannels {
            channels,
            close_on_raid_redirect: self.close_on_raid_redirect,
            adopt_existing_twitch_tabs: self.adopt_existing_twitch_tabs,
            viewer_login: match &self.connection_status {
                TwitchConnectionStatus::Connected(login) => Some(login.clone()),
                _ => None,
            },
        });
    }

    pub(super) fn set_extension_preferred_quality(&mut self) {
        self.send_bridge_command(BrowserCommand::SetPreferredQuality {
            preferred_quality: self.preferred_quality,
        });
    }

    pub(super) fn clear_offline_raid_redirects(&mut self) {
        if !self.browser_connected
            || !matches!(
                &self.connection_status,
                TwitchConnectionStatus::Connected(_)
            )
        {
            return;
        }
        let channel_ids = self
            .raid_redirected_channels
            .iter()
            .filter(|channel_id| !self.live_channel_ids.contains(*channel_id))
            .cloned()
            .collect::<Vec<_>>();
        if !channel_ids.is_empty() {
            self.send_bridge_command(BrowserCommand::ClearRaidRedirectedChannels { channel_ids });
        }
    }

    pub(super) fn apply_popup_action_with(
        &mut self,
        action: BrowserPopupAction,
        mut send: impl FnMut(&mut Self, BrowserCommand) -> bool,
    ) -> (bool, String) {
        match action {
            BrowserPopupAction::PauseAutomation => {
                if !self.automation_enabled {
                    return (false, "Automatic opening is stopped.".to_owned());
                }
                let (duration, label) = automation::TEMPORARY_AUTOMATION_DURATIONS[0];
                self.automation_delays
                    .pause_for(self.started_at.elapsed(), duration);
                (true, format!("Automatic opening paused for {label}."))
            }
            BrowserPopupAction::OpenSelected => {
                if self.browser_error.as_deref() == Some(COMMAND_ACK_REQUIRED_EXTENSION_ERROR) {
                    return (false, COMMAND_ACK_REQUIRED_EXTENSION_ERROR.to_owned());
                }
                if self.browser_connected && !self.browser_command_ack_supported {
                    return (false, COMMAND_ACK_REQUIRED_EXTENSION_ERROR.to_owned());
                }
                if matches!(
                    self.browser_error.as_deref(),
                    Some(OPEN_SELECTED_NO_CHANNELS_ERROR)
                        | Some(OPEN_SELECTED_NO_STREAM_SLOTS_ERROR)
                ) {
                    self.browser_error = None;
                }
                self.pending_manual_open_command = None;
                self.open_selected_twitch_channels_with(&mut send);
                if let Some(message) = self.browser_error.as_deref() {
                    (false, message.to_owned())
                } else if self.pending_manual_open_command.is_some() {
                    (
                        true,
                        "Open-selected request sent to the browser.".to_owned(),
                    )
                } else if self.pending_manual_open {
                    (
                        false,
                        "Open-selected request is queued until the app, browser, and Twitch are ready."
                            .to_owned(),
                    )
                } else if let Some(message) = self.browser_error.as_deref() {
                    (false, message.to_owned())
                } else {
                    (
                        false,
                        "No selected channels are available to open.".to_owned(),
                    )
                }
            }
        }
    }

    pub(super) fn open_selected_twitch_channels(&mut self) {
        self.pending_manual_open_command = None;
        self.open_selected_twitch_channels_with(|dashboard, command| {
            dashboard.send_bridge_command(command)
        });
    }

    pub(super) fn open_selected_twitch_channels_with(
        &mut self,
        mut send: impl FnMut(&mut Self, BrowserCommand) -> bool,
    ) {
        if !matches!(
            &self.connection_status,
            TwitchConnectionStatus::Connected(_)
        ) || self.twitch_network_unavailable
            || self.connection_error.is_some()
        {
            self.pending_manual_open = true;
            self.browser_notice = Some(
                "Open request queued until Twitch reconnects; it will use channels still selected when connectivity returns."
                    .to_owned(),
            );
            return;
        }
        if !self.browser_connected {
            self.pending_manual_open = true;
            self.browser_notice =
                Some("Open request queued until the browser extension reconnects.".to_owned());
            return;
        }
        if !self.external_inventory_received {
            self.pending_manual_open = true;
            self.browser_notice =
                Some("Open request queued until Twitch-tab inventory is ready.".to_owned());
            return;
        }
        self.pending_manual_open = false;
        let saved_manual_command = self
            .pending_manual_open_command
            .as_ref()
            .filter(|pending| pending.connection_generation == self.connection_generation)
            .and_then(|pending| pending.command.clone());
        if let Some(command) = saved_manual_command {
            let sent = send(self, command);
            if let Some(pending) = self.pending_manual_open_command.as_mut() {
                pending.sent_at = sent.then(Instant::now);
            }
            self.pending_manual_open = true;
            self.browser_notice = Some(
                "Each selected channel opens as one combined Twitch page in its own active window. Windows stay unfocused unless browser focus is enabled. App-opened player tabs start muted by default; channels marked Unmuted start with audio enabled."
                    .to_owned(),
            );
            return;
        }

        let limit = APP_STREAM_LIMIT;
        let selected_count = self
            .channels
            .iter()
            .filter(|channel| channel.selection.is_selected() && !channel.login.is_empty())
            .count();
        let mut app_open_channels = self.manual_open_channels.clone();
        app_open_channels.extend(self.auto_open_channels.iter().cloned());
        let app_open_streams =
            active_app_stream_count(&app_open_channels, &self.channels, &self.live_channel_ids);
        let external_streams = active_external_stream_count(
            &self.external_stream_counts,
            self.external_unmatched_streams,
            &self.live_channel_ids,
        );
        let available_slots =
            limit.saturating_sub(app_open_streams.saturating_add(external_streams));
        let external_channel_ids = self
            .external_stream_counts
            .iter()
            .filter(|(channel_id, count)| {
                **count > 0 && self.live_channel_ids.contains(*channel_id)
            })
            .map(|(channel_id, _)| channel_id.clone())
            .collect::<HashSet<_>>();
        let mut additional_channels = 0;
        let selected_indices = selected_channel_indices(&self.channels, self.channels.len())
            .into_iter()
            .filter(|index| {
                let channel_id = &self.channels[*index].id;
                if app_open_channels.contains(channel_id) {
                    true
                } else if external_channel_ids.contains(channel_id) {
                    false
                } else if additional_channels < available_slots {
                    additional_channels += 1;
                    true
                } else {
                    false
                }
            })
            .collect::<Vec<_>>();
        if selected_indices.is_empty() {
            self.browser_error = Some(if selected_count == 0 {
                OPEN_SELECTED_NO_CHANNELS_ERROR.to_owned()
            } else {
                OPEN_SELECTED_NO_STREAM_SLOTS_ERROR.to_owned()
            });
            return;
        }

        let channels: Vec<BrowserChannel> = selected_indices
            .iter()
            .map(|index| BrowserChannel::from(&self.channels[*index]))
            .collect();
        let request_id = self.allocate_browser_command_id();
        let connection_generation = self.connection_generation;
        let command = BrowserCommand::OpenSelected {
            wake_browser: self.wake_browser_on_open,
            channels,
            command_id: request_id.clone(),
            manual_request: true,
        };
        if !send(self, command.clone()) {
            self.pending_manual_open = true;
            return;
        }
        self.manual_open_channels.extend(
            selected_indices
                .iter()
                .map(|index| self.channels[*index].id.clone()),
        );
        self.pending_manual_open = true;
        self.pending_manual_open_command = Some(PendingBridgeCommand {
            request_id,
            connection_generation,
            sent_at: Some(Instant::now()),
            command: Some(command),
        });
        self.browser_notice = Some(
            "Each selected channel opens as one combined Twitch page in its own active window. Windows stay unfocused unless browser focus is enabled. App-opened player tabs start muted by default; channels marked Unmuted start with audio enabled."
                .to_owned(),
        );
    }
}
fn handle_closed_browser_tab(
    channel_id: String,
    tab_id: i64,
    surface: BrowserSurface,
    browser_states: &mut HashMap<String, BrowserChannelState>,
    manual_open_channels: &mut HashSet<String>,
    auto_open_channels: &mut HashSet<String>,
    auto_open_retry_after: &mut HashMap<String, Instant>,
) -> bool {
    if channel_id.is_empty() || tab_id <= 0 {
        return false;
    }
    let remove_state = if let Some(state) = browser_states.get_mut(&channel_id) {
        match surface {
            BrowserSurface::Player if state.player_tab_id == Some(tab_id) => {
                state.player_tab_id = None;
                state.player_state = None;
                state.player_last_seen = None;
            }
            BrowserSurface::Chat if state.chat_tab_id == Some(tab_id) => {
                state.chat_tab_id = None;
                state.chat_state = None;
                state.chat_last_seen = None;
            }
            _ => return false,
        }
        state.player_tab_id.is_none() && state.chat_tab_id.is_none()
    } else {
        false
    };
    if remove_state {
        browser_states.remove(&channel_id);
    }
    if remove_state || surface == BrowserSurface::Player {
        manual_open_channels.remove(&channel_id);
    }
    auto_open_channels.remove(&channel_id);
    auto_open_retry_after.remove(&channel_id);
    true
}

#[cfg(test)]
mod tests {
    use super::{handle_closed_browser_tab, BrowserSurface};
    use std::collections::{HashMap, HashSet};

    #[test]
    fn unknown_managed_player_close_releases_manual_reservation() {
        let channel_id = "unfollowed".to_owned();
        let mut browser_states = HashMap::new();
        let mut manual_open_channels = HashSet::from([channel_id.clone()]);
        let mut auto_open_channels = HashSet::new();
        let mut auto_open_retry_after = HashMap::new();

        handle_closed_browser_tab(
            channel_id.clone(),
            7,
            BrowserSurface::Chat,
            &mut browser_states,
            &mut manual_open_channels,
            &mut auto_open_channels,
            &mut auto_open_retry_after,
        );
        assert_eq!(manual_open_channels, HashSet::from([channel_id.clone()]));

        handle_closed_browser_tab(
            channel_id,
            7,
            BrowserSurface::Player,
            &mut browser_states,
            &mut manual_open_channels,
            &mut auto_open_channels,
            &mut auto_open_retry_after,
        );
        assert!(manual_open_channels.is_empty());
    }
}
