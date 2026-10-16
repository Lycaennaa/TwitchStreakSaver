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
                    self.pending_stream_switch = None;
                    self.browser_connected = true;
                    self.external_stream_counts.clear();
                    self.external_unmatched_streams = 0;
                    self.external_inventory_received = false;
                    self.auto_open_retry_after.clear();
                    self.browser_error = None;
                    self.browser_notice = None;
                    self.send_bridge_command(BrowserCommand::BridgeReady);
                }
                BridgeEvent::Disconnected => {
                    self.pending_stream_switch = None;
                    self.browser_connected = false;
                    self.external_stream_counts.clear();
                    self.external_unmatched_streams = 0;
                    self.external_inventory_received = false;
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
                BridgeEvent::Failed(message) => {
                    self.pending_stream_switch = None;
                    self.browser_error = Some(message);
                }
            }
        }
    }

    pub(super) fn retry_browser_bridge(&mut self) -> bool {
        if self.browser_bridge.is_some() {
            return true;
        }
        match BrowserBridge::start() {
            Ok(bridge) => {
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

    pub(super) fn handle_browser_message(&mut self, message: KnownBrowserMessage) {
        match message {
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
                    if self.browser_error.as_deref() == Some(STREAM_LIMIT_EXTENSION_ERROR) {
                        self.browser_error = None;
                    }
                } else {
                    self.external_inventory_received = false;
                    self.browser_error = Some(STREAM_LIMIT_EXTENSION_ERROR.to_owned());
                }
                self.clear_offline_raid_redirects();
            }
            KnownBrowserMessage::Ready => {
                self.sync_extension_channels();
                self.set_extension_preferred_quality();
            }
            KnownBrowserMessage::TabOpened {
                channel_id,
                tab_id,
                surface,
            } => {
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
                if surface == BrowserSurface::Player && (accepted || pending_open) {
                    if let Some(pending) = &mut self.pending_stream_switch {
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
            } => {
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
                for channel_id in channels.keys() {
                    self.auto_open_retry_after.remove(channel_id);
                }
                if successful {
                    record_activity(&mut self.activity_log, streamer, event);
                }
            }
        }
    }

    pub(super) fn send_bridge_command(&mut self, command: BrowserCommand) {
        if !self.browser_connected {
            self.browser_error = Some("Browser extension is not connected.".to_owned());
            return;
        }
        if let Some(bridge) = &self.browser_bridge {
            if let Err(error) = bridge.send(command) {
                self.browser_error = Some(error);
            }
        } else {
            self.browser_error = Some("Browser bridge listener is unavailable.".to_owned());
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

    pub(super) fn open_selected_twitch_channels(&mut self) {
        if !self.browser_connected {
            self.browser_error = Some("Browser extension is not connected.".to_owned());
            return;
        }
        if !self.external_inventory_received {
            self.browser_notice =
                Some("Scanning existing Twitch tabs; try again shortly.".to_owned());
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
                "Select followed Twitch channels to open.".to_owned()
            } else {
                "No stream slots are available after counting existing Twitch tabs.".to_owned()
            });
            return;
        }

        let channels: Vec<BrowserChannel> = selected_indices
            .iter()
            .map(|index| BrowserChannel::from(&self.channels[*index]))
            .collect();
        self.manual_open_channels.extend(
            selected_indices
                .iter()
                .map(|index| self.channels[*index].id.clone()),
        );
        self.send_bridge_command(BrowserCommand::OpenSelected {
            wake_browser: self.wake_browser_on_open,
            channels,
        });
        self.browser_notice = Some(
            "Each selected channel opens as one combined Twitch page in its own active window. Windows stay unfocused unless browser focus is enabled. App-opened player audio is muted."
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
