use crate::model::FollowedChannel;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::HashMap;

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BrowserCommand {
    BridgeReady,
    SyncChannels {
        channels: Vec<BrowserChannel>,
        close_on_raid_redirect: bool,
    },
    ClearRaidRedirectedChannels {
        channel_ids: Vec<String>,
    },
    OpenSelected {
        wake_browser: bool,
        channels: Vec<BrowserChannel>,
    },
    CloseAll,
    CloseChannels {
        channel_ids: Vec<String>,
    },
}

#[derive(Serialize)]
pub struct BrowserChannel {
    id: String,
    login: String,
}

impl From<&FollowedChannel> for BrowserChannel {
    fn from(channel: &FollowedChannel) -> Self {
        Self {
            id: channel.id.clone(),
            login: channel.login.clone(),
        }
    }
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum BrowserMessage {
    Known(KnownBrowserMessage),
    Unknown(Value),
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum KnownBrowserMessage {
    Ready,
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
    ChannelStatus {
        channel_id: String,
        tab_id: i64,
        surface: BrowserSurface,
        #[serde(default)]
        player_state: Option<PlayerState>,
        #[serde(default)]
        chat_state: Option<ChatState>,
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
    use crate::model::FollowedChannel;
    use serde_json::json;
    use std::collections::HashMap;

    #[test]
    fn open_command_matches_extension_wire_contract() {
        let mut channel = FollowedChannel::new("123", "Streamer");
        channel.login = "streamer".to_owned();
        let command = BrowserCommand::OpenSelected {
            wake_browser: true,
            channels: vec![BrowserChannel::from(&channel)],
        };

        assert_eq!(
            serde_json::to_value(command).unwrap(),
            json!({
                "type": "open_selected",
                "wake_browser": true,
                "channels": [{"id": "123", "login": "streamer"}],
            }),
        );
    }
    #[test]
    fn channel_sync_includes_raid_redirect_setting() {
        let command = BrowserCommand::SyncChannels {
            channels: vec![],
            close_on_raid_redirect: true,
        };

        assert_eq!(
            serde_json::to_value(command).unwrap(),
            json!({
                "type": "sync_channels",
                "channels": [],
                "close_on_raid_redirect": true,
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
}
