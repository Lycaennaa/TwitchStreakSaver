use crate::model::FollowedChannel;
use crate::twitch_events::{parse_pubsub_frame, TwitchObservedEvent};
use reqwest::blocking::{Client, Response};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc::Sender, Arc};
use std::thread;
use std::time::{Duration, Instant};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{connect, Error as WebSocketError, Message, WebSocket};
use url::Url;
mod api;
pub mod connection;
pub mod oauth;
mod pubsub;

const CLIENT_ID: &str = "phiay4sq36lfv9zu7cbqwz2ndnesfd8";
const REDIRECT_URI: &str = "http://localhost:65432/redirect";
const OAUTH_URL: &str = "https://id.twitch.tv/oauth2/authorize";
const VALIDATE_URL: &str = "https://id.twitch.tv/oauth2/validate";
const FOLLOWED_URL: &str = "https://api.twitch.tv/helix/channels/followed";
const STREAMS_URL: &str = "https://api.twitch.tv/helix/streams";
const PUBSUB_URL: &str = "wss://pubsub-edge.twitch.tv/v1";
const PUBSUB_READ_TIMEOUT: Duration = Duration::from_secs(2);
const PUBSUB_PING_INTERVAL: Duration = Duration::from_secs(240);
const PUBSUB_RECONNECT_INITIAL: Duration = Duration::from_secs(2);
const PUBSUB_RECONNECT_MAX: Duration = Duration::from_secs(60);
const REQUIRED_SCOPE: &str = "user:read:follows";
const KEYRING_SERVICE: &str = "Twitch Streak Saver";
const KEYRING_ACCOUNT: &str = "twitch-access-token";
const OAUTH_TIMEOUT: Duration = Duration::from_secs(300);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const LIVE_REFRESH_INTERVAL: Duration = Duration::from_secs(60);
const FOLLOW_REFRESH_INTERVAL: Duration = Duration::from_secs(300);
const MAX_FOLLOW_PAGES: usize = 1000;
const MAX_HTTP_HEADER_BYTES: usize = 8192;
const MAX_HTTP_BODY_BYTES: usize = 4096;

pub enum TwitchUpdate {
    Connected {
        generation: u64,
        login: String,
        channels: Vec<FollowedChannel>,
        live_channel_ids: HashSet<String>,
    },
    Refreshed {
        generation: u64,
        channels: Vec<FollowedChannel>,
        live_channel_ids: HashSet<String>,
    },
    LiveStatus {
        generation: u64,
        live_channel_ids: HashSet<String>,
    },
    PubSubStatus {
        generation: u64,
        connected: bool,
        detail: Option<String>,
    },
    PointsEarned {
        generation: u64,
        channel_id: String,
        points_earned: u64,
        balance: u64,
        reason_code: String,
    },
    PointsSpent {
        generation: u64,
        channel_id: String,
        balance: u64,
    },
    ClaimAvailable {
        generation: u64,
        channel_id: String,
    },
    StreakCompleted {
        generation: u64,
        channel_id: String,
        points_earned: u64,
    },
    Warning {
        generation: u64,
        message: String,
    },
    Failed {
        generation: u64,
        message: String,
    },
}

#[derive(Debug)]
enum TwitchError {
    Cancelled,
    Reauthorize(String),
    Failed(String),
}

impl TwitchError {
    fn message(self) -> String {
        match self {
            Self::Cancelled => "Twitch connection cancelled".to_owned(),
            Self::Reauthorize(message) | Self::Failed(message) => message,
        }
    }
}

#[derive(Deserialize)]
struct ValidateResponse {
    client_id: String,
    login: String,
    user_id: String,
    #[serde(default)]
    scopes: Vec<String>,
}

struct TwitchUser {
    login: String,
    id: String,
}

#[derive(Deserialize)]
struct FollowedResponse {
    #[serde(default)]
    data: Vec<FollowedRecord>,
    pagination: Option<Pagination>,
}

#[derive(Deserialize)]
struct FollowedRecord {
    broadcaster_id: String,
    broadcaster_login: String,
    broadcaster_name: String,
}

#[derive(Deserialize)]
struct Pagination {
    cursor: Option<String>,
}

#[derive(Deserialize)]
struct StreamsResponse {
    #[serde(default)]
    data: Vec<LiveStream>,
}

#[derive(Deserialize)]
struct LiveStream {
    user_id: String,
}

#[derive(Deserialize)]
struct OAuthCallback {
    access_token: Option<String>,
    token_type: Option<String>,
    scope: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

struct HttpRequest {
    method: String,
    path: String,
    body: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::{
        api::user_from_validation,
        oauth::{build_authorization_url, token_from_callback},
        pubsub::{dispatch_pubsub_event, pubsub_listen_request},
        OAuthCallback, TwitchUpdate, ValidateResponse, CLIENT_ID, REDIRECT_URI, REQUIRED_SCOPE,
    };
    use crate::twitch_events::TwitchObservedEvent;
    use std::sync::mpsc;

    #[test]
    fn pubsub_subscription_listens_only_to_authenticated_user_topic() {
        let request = pubsub_listen_request("viewer-123", "secret-token", "nonce-1");

        assert_eq!(request["type"], "LISTEN");
        assert_eq!(request["nonce"], "nonce-1");
        assert_eq!(
            request["data"]["topics"][0],
            "community-points-user-v1.viewer-123"
        );
        assert_eq!(request["data"]["auth_token"], "secret-token");
    }
    #[test]
    fn authorization_url_matches_streamlink_client_and_registered_callback() {
        let url = build_authorization_url("test-state").unwrap();
        let query: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();

        assert_eq!(url.host_str(), Some("id.twitch.tv"));
        assert_eq!(query.get("client_id").map(String::as_str), Some(CLIENT_ID));
        assert_eq!(
            query.get("redirect_uri").map(String::as_str),
            Some(REDIRECT_URI)
        );
        assert_eq!(
            query.get("response_type").map(String::as_str),
            Some("token")
        );
        assert_eq!(query.get("scope").map(String::as_str), Some(REQUIRED_SCOPE));
        assert_eq!(query.get("state").map(String::as_str), Some("test-state"));
    }

    #[test]
    fn oauth_callback_requires_bearer_token_and_follow_scope() {
        let callback = OAuthCallback {
            access_token: Some("safe.token-123".to_owned()),
            token_type: Some("bearer".to_owned()),
            scope: Some(REQUIRED_SCOPE.to_owned()),
            state: Some("state".to_owned()),
            error: None,
            error_description: None,
        };
        assert_eq!(token_from_callback(callback).unwrap(), "safe.token-123");

        let missing_scope = OAuthCallback {
            access_token: Some("safe.token-123".to_owned()),
            token_type: Some("bearer".to_owned()),
            scope: Some("chat:read".to_owned()),
            state: Some("state".to_owned()),
            error: None,
            error_description: None,
        };
        assert!(token_from_callback(missing_scope).is_err());
    }

    #[test]
    fn token_validation_requires_streamlink_client_and_follow_scope() {
        let identity = ValidateResponse {
            client_id: CLIENT_ID.to_owned(),
            login: "viewer".to_owned(),
            user_id: "123".to_owned(),
            scopes: vec![REQUIRED_SCOPE.to_owned()],
        };
        assert_eq!(user_from_validation(identity).unwrap().login, "viewer");

        let wrong_client = ValidateResponse {
            client_id: "other-client".to_owned(),
            login: "viewer".to_owned(),
            user_id: "123".to_owned(),
            scopes: vec![REQUIRED_SCOPE.to_owned()],
        };
        assert!(user_from_validation(wrong_client).is_err());
    }
    #[test]
    fn pubsub_watch_streak_updates_balance_and_streak_completion() {
        let (updates_sender, updates_receiver) = mpsc::channel();
        dispatch_pubsub_event(
            9,
            TwitchObservedEvent::PointsEarned {
                channel_id: "channel-1".to_owned(),
                points_earned: 400,
                balance: 1400,
                reason_code: "WATCH_STREAK".to_owned(),
            },
            &updates_sender,
        );

        assert!(matches!(
            updates_receiver.try_recv().unwrap(),
            TwitchUpdate::StreakCompleted {
                generation: 9,
                channel_id,
                points_earned: 400,
            } if channel_id == "channel-1"
        ));
        assert!(matches!(
            updates_receiver.try_recv().unwrap(),
            TwitchUpdate::PointsEarned {
                generation: 9,
                channel_id,
                points_earned: 400,
                balance: 1400,
                reason_code,
            } if channel_id == "channel-1" && reason_code == "WATCH_STREAK"
        ));
    }

    #[test]
    fn pubsub_dispatches_spending_and_bonus_events_without_mutations() {
        let (updates_sender, updates_receiver) = mpsc::channel();
        dispatch_pubsub_event(
            3,
            TwitchObservedEvent::PointsSpent {
                channel_id: "channel-2".to_owned(),
                balance: 250,
            },
            &updates_sender,
        );
        dispatch_pubsub_event(
            3,
            TwitchObservedEvent::ClaimAvailable {
                channel_id: "channel-2".to_owned(),
            },
            &updates_sender,
        );

        assert!(matches!(
            updates_receiver.try_recv().unwrap(),
            TwitchUpdate::PointsSpent {
                generation: 3,
                channel_id,
                balance: 250,
            } if channel_id == "channel-2"
        ));
        assert!(matches!(
            updates_receiver.try_recv().unwrap(),
            TwitchUpdate::ClaimAvailable {
                generation: 3,
                channel_id,
            } if channel_id == "channel-2"
        ));
    }
}
