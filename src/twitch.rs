use crate::model::FollowedChannel;
use reqwest::blocking::{Client, Response};
use serde::Deserialize;
use std::collections::HashSet;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc::Sender, Arc};
use std::thread;
use std::time::{Duration, Instant};
use url::Url;
mod api;
pub mod connection;
pub mod credential_store;
pub mod oauth;

const CLIENT_ID: &str = "phiay4sq36lfv9zu7cbqwz2ndnesfd8";
const REDIRECT_URI: &str = "http://localhost:65432/redirect";
const OAUTH_URL: &str = "https://id.twitch.tv/oauth2/authorize";
const VALIDATE_URL: &str = "https://id.twitch.tv/oauth2/validate";
const FOLLOWED_URL: &str = "https://api.twitch.tv/helix/channels/followed";
const STREAMS_URL: &str = "https://api.twitch.tv/helix/streams";
const REQUIRED_SCOPE: &str = "user:read:follows";
const OAUTH_TIMEOUT: Duration = Duration::from_secs(300);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const LIVE_REFRESH_INTERVAL: Duration = Duration::from_secs(60);
const FOLLOW_REFRESH_INTERVAL: Duration = Duration::from_secs(300);
const NETWORK_RETRY_INITIAL: Duration = Duration::from_secs(2);
const NETWORK_RETRY_MAX: Duration = Duration::from_secs(60);
const MAX_FOLLOW_PAGES: usize = 1000;
const MAX_HTTP_HEADER_BYTES: usize = 8192;
const MAX_HTTP_BODY_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TwitchNetworkSource {
    Connection,
    FollowedChannels,
    LiveStatus,
}

#[derive(Debug)]
pub enum TwitchUpdateFailure {
    Retryable(String),
    Warning(String),
}

pub enum TwitchUpdate {
    AccountSaved {
        generation: u64,
        login: String,
        user_id: String,
    },
    Connected {
        generation: u64,
        login: String,
        user_id: String,
        channels: Vec<FollowedChannel>,
        live_channel_ids: HashSet<String>,
    },
    Refreshed {
        generation: u64,
        channels: Vec<FollowedChannel>,
        live_channel_ids: HashSet<String>,
    },
    FollowedChannels {
        generation: u64,
        channels: Vec<FollowedChannel>,
        live_status_failure: TwitchUpdateFailure,
    },

    LiveStatus {
        generation: u64,
        live_channel_ids: HashSet<String>,
    },
    PointsEventsStatus {
        generation: u64,
        connected: bool,
        detail: Option<String>,
    },
    NetworkUnavailable {
        generation: u64,
        source: TwitchNetworkSource,
        message: String,
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
    Retryable(String),
    Failed(String),
}
impl TwitchError {
    fn message(self) -> String {
        match self {
            Self::Cancelled => "Twitch connection cancelled".to_owned(),
            Self::Reauthorize(message) | Self::Retryable(message) | Self::Failed(message) => {
                message
            }
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
        OAuthCallback, ValidateResponse, CLIENT_ID, REDIRECT_URI, REQUIRED_SCOPE,
    };

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
}
