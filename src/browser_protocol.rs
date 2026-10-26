use crate::model::{FollowedChannel, PreferredQuality};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BrowserCommand {
    BridgeReady,
    SetPreferredQuality {
        preferred_quality: PreferredQuality,
    },
    SyncChannels {
        channels: Vec<BrowserChannel>,
        close_on_raid_redirect: bool,
        adopt_existing_twitch_tabs: bool,
        viewer_login: Option<String>,
    },
    ClearRaidRedirectedChannels {
        channel_ids: Vec<String>,
    },
    OpenSelected {
        wake_browser: bool,
        channels: Vec<BrowserChannel>,
        command_id: String,
        manual_request: bool,
    },
    CloseAll,
    ReloadManagedPlayerTabs {
        command_id: String,
    },
    PopupActionResult {
        request_id: String,
        success: bool,
        message: String,
    },
    CloseChannels {
        channel_ids: Vec<String>,
    },
}

#[derive(Clone, Serialize)]
pub struct BrowserChannel {
    id: String,
    login: String,
    unmuted: bool,
}

impl From<&FollowedChannel> for BrowserChannel {
    fn from(channel: &FollowedChannel) -> Self {
        Self {
            id: channel.id.clone(),
            login: channel.login.clone(),
            unmuted: channel.unmuted,
        }
    }
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum BrowserMessage {
    Known(KnownBrowserMessage),
    Unknown(Value),
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BrowserPopupAction {
    PauseAutomation,
    OpenSelected,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum KnownBrowserMessage {
    Ready {
        #[serde(default)]
        command_acknowledgments: bool,
    },
    PopupAction {
        request_id: String,
        action: BrowserPopupAction,
    },
    CommandAcknowledged {
        command_id: String,
    },
    ExternalStreams {
        #[serde(deserialize_with = "deserialize_channel_counts")]
        channel_counts: HashMap<String, usize>,
        unmatched_count: usize,
        #[serde(default)]
        managed_channel_ids: Option<Vec<String>>,
        #[serde(default)]
        raid_redirected_channel_ids: Option<Vec<String>>,
    },
    TabOpened {
        channel_id: String,
        tab_id: i64,
        surface: BrowserSurface,
    },
    TabAdopted {
        channel_id: String,
        tab_id: i64,
        surface: BrowserSurface,
    },
    TabReleased {
        channel_id: String,
        tab_id: i64,
        surface: BrowserSurface,
    },
    ChannelStatus {
        channel_id: String,
        tab_id: i64,
        surface: BrowserSurface,
        #[serde(default)]
        player_state: Option<PlayerState>,
        #[serde(default)]
        chat_state: Option<ChatState>,
        #[serde(default)]
        chat_message_author_login: Option<String>,
    },
    TabClosed {
        channel_id: String,
        tab_id: i64,
        surface: BrowserSurface,
        #[serde(default)]
        close_reason: Option<BrowserTabCloseReason>,
    },
    Error {
        #[serde(default)]
        channel_id: Option<String>,
        #[serde(default)]
        message: Option<String>,
        #[serde(default)]
        close_reason: Option<BrowserTabCloseReason>,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrowserSurface {
    Player,
    Chat,
}
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrowserTabCloseReason {
    RaidRedirect,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(from = "String")]
pub enum PlayerState {
    Playing,
    Paused,
    Buffering,
    Ended,
    Missing,
    Unknown,
}

impl From<String> for PlayerState {
    fn from(value: String) -> Self {
        match value.as_str() {
            "playing" => Self::Playing,
            "paused" => Self::Paused,
            "buffering" => Self::Buffering,
            "ended" => Self::Ended,
            "missing" => Self::Missing,
            _ => Self::Unknown,
        }
    }
}

impl PlayerState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Playing => "playing",
            Self::Paused => "paused",
            Self::Buffering => "buffering",
            Self::Ended => "ended",
            Self::Missing => "missing",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(from = "String")]
pub enum ChatState {
    Ready,
    Loading,
    Missing,
    Unknown,
}

impl From<String> for ChatState {
    fn from(value: String) -> Self {
        match value.as_str() {
            "ready" => Self::Ready,
            "loading" => Self::Loading,
            "missing" => Self::Missing,
            _ => Self::Unknown,
        }
    }
}

impl ChatState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Loading => "loading",
            Self::Missing => "missing",
            Self::Unknown => "unknown",
        }
    }
}

fn deserialize_channel_counts<'de, D>(deserializer: D) -> Result<HashMap<String, usize>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = HashMap::<String, Value>::deserialize(deserializer)?;
    Ok(raw
        .into_iter()
        .filter_map(|(channel_id, value)| {
            value
                .as_u64()
                .and_then(|count| usize::try_from(count).ok())
                .map(|count| (channel_id, count))
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::{
        BrowserChannel, BrowserCommand, BrowserMessage, BrowserSurface, BrowserTabCloseReason,
        ChatState, KnownBrowserMessage, PlayerState,
    };
    use crate::model::{FollowedChannel, PreferredQuality};
    use serde_json::json;
    use std::collections::HashMap;

    #[test]
    fn open_command_matches_extension_wire_contract() {
        let mut channel = FollowedChannel::new("123", "Streamer");
        channel.login = "streamer".to_owned();
        channel.unmuted = true;
        let command = BrowserCommand::OpenSelected {
            wake_browser: true,
            channels: vec![BrowserChannel::from(&channel)],
            command_id: "session-7".to_owned(),
            manual_request: true,
        };

        assert_eq!(
            serde_json::to_value(command).unwrap(),
            json!({
                "type": "open_selected",
                "wake_browser": true,
                "channels": [{"id": "123", "login": "streamer", "unmuted": true}],
                "command_id": "session-7",
                "manual_request": true,
            }),
        );
    }

    #[test]
    fn command_acknowledgment_decodes_request_id() {
        let message: BrowserMessage = serde_json::from_value(json!({
            "type": "command_acknowledged",
            "command_id": "session-7",
        }))
        .unwrap();

        assert_eq!(
            message,
            BrowserMessage::Known(KnownBrowserMessage::CommandAcknowledged {
                command_id: "session-7".to_owned(),
            })
        );
    }
    #[test]
    fn legacy_ready_without_ack_capability_defaults_to_unsupported() {
        let message: BrowserMessage = serde_json::from_value(json!({
            "type": "ready",
        }))
        .unwrap();

        assert_eq!(
            message,
            BrowserMessage::Known(KnownBrowserMessage::Ready {
                command_acknowledgments: false,
            })
        );
    }
    #[test]
    fn channel_sync_includes_raid_redirect_setting() {
        let command = BrowserCommand::SyncChannels {
            channels: vec![],
            viewer_login: Some("viewer".to_owned()),
            close_on_raid_redirect: true,
            adopt_existing_twitch_tabs: true,
        };

        assert_eq!(
            serde_json::to_value(command).unwrap(),
            json!({
                "type": "sync_channels",
                "channels": [],
                "viewer_login": "viewer",
                "close_on_raid_redirect": true,
                "adopt_existing_twitch_tabs": true,
            }),
        );
    }
    #[test]
    fn preferred_quality_command_matches_extension_wire_contract() {
        let command = BrowserCommand::SetPreferredQuality {
            preferred_quality: PreferredQuality::P720,
        };

        assert_eq!(
            serde_json::to_value(command).unwrap(),
            json!({
                "type": "set_preferred_quality",
                "preferred_quality": "720p",
            }),
        );
    }

    #[test]
    fn raid_redirect_clear_command_matches_extension_wire_contract() {
        let command = BrowserCommand::ClearRaidRedirectedChannels {
            channel_ids: vec!["123".to_owned()],
        };

        assert_eq!(
            serde_json::to_value(command).unwrap(),
            json!({
                "type": "clear_raid_redirected_channels",
                "channel_ids": ["123"],
            }),
        );
    }
    #[test]
    fn tab_close_reason_decodes_raid_and_defaults_for_legacy_messages() {
        let raid = serde_json::from_value::<BrowserMessage>(json!({
            "type": "tab_closed",
            "channel_id": "123",
            "tab_id": 7,
            "surface": "player",
            "close_reason": "raid_redirect",
        }))
        .unwrap();
        assert_eq!(
            raid,
            BrowserMessage::Known(KnownBrowserMessage::TabClosed {
                channel_id: "123".to_owned(),
                tab_id: 7,
                surface: BrowserSurface::Player,
                close_reason: Some(BrowserTabCloseReason::RaidRedirect),
            }),
        );

        let legacy = serde_json::from_value::<BrowserMessage>(json!({
            "type": "tab_closed",
            "channel_id": "123",
            "tab_id": 7,
            "surface": "player",
        }))
        .unwrap();
        assert_eq!(
            legacy,
            BrowserMessage::Known(KnownBrowserMessage::TabClosed {
                channel_id: "123".to_owned(),
                tab_id: 7,
                surface: BrowserSurface::Player,
                close_reason: None,
            }),
        );
    }

    #[test]
    fn raid_close_error_decodes_and_legacy_error_defaults() {
        let raid_error = serde_json::from_value::<BrowserMessage>(json!({
            "type": "error",
            "channel_id": "123",
            "message": "Could not close managed tab",
            "close_reason": "raid_redirect",
        }))
        .unwrap();
        assert_eq!(
            raid_error,
            BrowserMessage::Known(KnownBrowserMessage::Error {
                channel_id: Some("123".to_owned()),
                message: Some("Could not close managed tab".to_owned()),
                close_reason: Some(BrowserTabCloseReason::RaidRedirect),
            }),
        );

        let legacy_error = serde_json::from_value::<BrowserMessage>(json!({
            "type": "error",
            "message": "legacy error",
        }))
        .unwrap();
        assert_eq!(
            legacy_error,
            BrowserMessage::Known(KnownBrowserMessage::Error {
                channel_id: None,
                message: Some("legacy error".to_owned()),
                close_reason: None,
            }),
        );
    }

    #[test]
    fn browser_messages_decode_typed_status_and_preserve_unknown_events() {
        let status = serde_json::from_value::<BrowserMessage>(json!({
            "type": "channel_status",
            "channel_id": "123",
            "tab_id": 7,
            "surface": "player",
            "player_state": "playing",
            "chat_state": "ready",
            "chat_message_author_login": "viewer",
        }))
        .unwrap();

        assert_eq!(
            status,
            BrowserMessage::Known(KnownBrowserMessage::ChannelStatus {
                channel_id: "123".to_owned(),
                tab_id: 7,
                surface: BrowserSurface::Player,
                player_state: Some(PlayerState::Playing),
                chat_state: Some(ChatState::Ready),
                chat_message_author_login: Some("viewer".to_owned()),
            }),
        );
        let unknown_status = serde_json::from_value::<BrowserMessage>(json!({
            "type": "channel_status",
            "channel_id": "123",
            "tab_id": 7,
            "surface": "player",
            "player_state": "seeking",
            "chat_state": "connecting",
        }))
        .unwrap();
        assert_eq!(
            unknown_status,
            BrowserMessage::Known(KnownBrowserMessage::ChannelStatus {
                channel_id: "123".to_owned(),
                tab_id: 7,
                surface: BrowserSurface::Player,
                player_state: Some(PlayerState::Unknown),
                chat_state: Some(ChatState::Unknown),
                chat_message_author_login: None,
            }),
        );
        assert_eq!(
            serde_json::from_value::<BrowserMessage>(json!({"type": "future_event"})).unwrap(),
            BrowserMessage::Unknown(json!({"type": "future_event"})),
        );
    }

    #[test]
    fn inventory_message_keeps_valid_counts_and_discards_malformed_values() {
        let message = serde_json::from_value::<BrowserMessage>(json!({
            "type": "external_streams",
            "channel_counts": {"live": 2, "invalid": "many", "zero": 0},
            "unmatched_count": 3,
            "managed_channel_ids": ["managed-channel"],
            "raid_redirected_channel_ids": ["raid-channel"],
        }))
        .unwrap();

        let BrowserMessage::Known(KnownBrowserMessage::ExternalStreams {
            channel_counts,
            unmatched_count,
            managed_channel_ids,
            raid_redirected_channel_ids,
        }) = message
        else {
            panic!("expected external stream inventory");
        };
        assert_eq!(
            channel_counts,
            HashMap::from([("live".to_owned(), 2), ("zero".to_owned(), 0)]),
        );
        assert_eq!(unmatched_count, 3);
        assert_eq!(
            managed_channel_ids,
            Some(vec!["managed-channel".to_owned()])
        );
        assert_eq!(
            raid_redirected_channel_ids,
            Some(vec!["raid-channel".to_owned()]),
        );
        let legacy = serde_json::from_value::<BrowserMessage>(json!({
            "type": "external_streams",
            "channel_counts": {},
            "unmatched_count": 0,
        }))
        .unwrap();
        assert!(matches!(
            legacy,
            BrowserMessage::Known(KnownBrowserMessage::ExternalStreams {
                managed_channel_ids: None,
                raid_redirected_channel_ids: None,
                ..
            })
        ));
    }
    #[test]
    fn adopted_and_released_tab_messages_deserialize() {
        let adopted = serde_json::from_value::<BrowserMessage>(json!({
            "type": "tab_adopted",
            "channel_id": "channel-1",
            "tab_id": 7,
            "surface": "player",
        }))
        .unwrap();
        assert_eq!(
            adopted,
            BrowserMessage::Known(KnownBrowserMessage::TabAdopted {
                channel_id: "channel-1".to_owned(),
                tab_id: 7,
                surface: BrowserSurface::Player,
            }),
        );

        let released = serde_json::from_value::<BrowserMessage>(json!({
            "type": "tab_released",
            "channel_id": "channel-1",
            "tab_id": 7,
            "surface": "player",
        }))
        .unwrap();
        assert_eq!(
            released,
            BrowserMessage::Known(KnownBrowserMessage::TabReleased {
                channel_id: "channel-1".to_owned(),
                tab_id: 7,
                surface: BrowserSurface::Player,
            }),
        );
    }
    #[test]
    fn popup_action_requests_deserialize_and_results_serialize() {
        let request = serde_json::from_value::<BrowserMessage>(json!({
            "type": "popup_action",
            "request_id": "popup-1",
            "action": "pause_automation",
        }))
        .unwrap();
        assert_eq!(
            request,
            BrowserMessage::Known(KnownBrowserMessage::PopupAction {
                request_id: "popup-1".to_owned(),
                action: super::BrowserPopupAction::PauseAutomation,
            }),
        );

        let result = serde_json::to_value(BrowserCommand::PopupActionResult {
            request_id: "popup-1".to_owned(),
            success: false,
            message: "No selected channels are available to open.".to_owned(),
        })
        .unwrap();
        assert_eq!(
            result,
            json!({
                "type": "popup_action_result",
                "request_id": "popup-1",
                "success": false,
                "message": "No selected channels are available to open.",
            }),
        );
    }
}
