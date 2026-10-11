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

pub fn clear_saved_token() -> Result<(), String> {
    clear_keyring_token().map_err(TwitchError::message)
}

pub fn run_connection(cancel: Arc<AtomicBool>, generation: u64, updates: Sender<TwitchUpdate>) {
    let client = match build_client() {
        Ok(client) => client,
        Err(error) => {
            send_failed(&updates, generation, error.message());
            return;
        }
    };
    let (token, user) = match obtain_token(&client, &cancel) {
        Ok(result) => result,
        Err(TwitchError::Cancelled) => return,
        Err(error) => {
            send_failed(&updates, generation, error.message());
            return;
        }
    };
    if cancel.load(Ordering::Acquire) {
        return;
    }

    let mut channels = match fetch_followed_channels(&client, &token, &user, &cancel) {
        Ok(channels) => channels,
        Err(TwitchError::Cancelled) => return,
        Err(error) => {
            send_failed(&updates, generation, error.message());
            return;
        }
    };
    let mut live_channel_ids = match fetch_live_channel_ids(&client, &token, &channels, &cancel) {
        Ok(live) => live,
        Err(TwitchError::Cancelled) => return,
        Err(error) => {
            send_failed(&updates, generation, error.message());
            return;
        }
    };
    if cancel.load(Ordering::Acquire) {
        return;
    }
    if updates
        .send(TwitchUpdate::Connected {
            generation,
            login: user.login.clone(),
            channels: channels.clone(),
            live_channel_ids: live_channel_ids.clone(),
        })
        .is_err()
    {
        return;
    }

    let pubsub_cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = Arc::clone(&pubsub_cancel);
    let pubsub_updates = updates.clone();
    let pubsub_token = token.clone();
    let pubsub_user_id = user.id.clone();
    if let Err(error) = thread::Builder::new()
        .name("twitch-pubsub-listener".to_owned())
        .spawn(move || {
            run_pubsub_listener(
                worker_cancel,
                generation,
                pubsub_user_id,
                pubsub_token,
                pubsub_updates,
            )
        })
    {
        let _ = updates.send(TwitchUpdate::PubSubStatus {
            generation,
            connected: false,
            detail: Some(format!(
                "Could not start Channel Points/Streak event listener: {error}"
            )),
        });
    }

    let mut last_live_refresh = Instant::now();
    let mut last_follow_refresh = Instant::now();
    while !cancel.load(Ordering::Acquire) {
        thread::sleep(Duration::from_millis(250));
        if cancel.load(Ordering::Acquire) {
            break;
        }

        if last_live_refresh.elapsed() >= LIVE_REFRESH_INTERVAL {
            match fetch_live_channel_ids(&client, &token, &channels, &cancel) {
                Ok(live) => {
                    live_channel_ids = live;
                    if updates
                        .send(TwitchUpdate::LiveStatus {
                            generation,
                            live_channel_ids: live_channel_ids.clone(),
                        })
                        .is_err()
                    {
                        break;
                    }
                }
                Err(TwitchError::Cancelled) => break,
                Err(error @ TwitchError::Reauthorize(_)) => {
                    fail_expired_session(&updates, generation, error);
                    break;
                }
                Err(error) => {
                    let _ = updates.send(TwitchUpdate::Warning {
                        generation,
                        message: error.message(),
                    });
                }
            }
            last_live_refresh = Instant::now();
        }

        if last_follow_refresh.elapsed() >= FOLLOW_REFRESH_INTERVAL {
            match fetch_followed_channels(&client, &token, &user, &cancel) {
                Ok(refreshed_channels) => {
                    match fetch_live_channel_ids(&client, &token, &refreshed_channels, &cancel) {
                        Ok(live) => {
                            channels = refreshed_channels;
                            live_channel_ids = live;
                            if updates
                                .send(TwitchUpdate::Refreshed {
                                    generation,
                                    channels: channels.clone(),
                                    live_channel_ids: live_channel_ids.clone(),
                                })
                                .is_err()
                            {
                                break;
                            }
                        }
                        Err(TwitchError::Cancelled) => break,
                        Err(error @ TwitchError::Reauthorize(_)) => {
                            fail_expired_session(&updates, generation, error);
                            break;
                        }
                        Err(error) => {
                            let _ = updates.send(TwitchUpdate::Warning {
                                generation,
                                message: error.message(),
                            });
                        }
                    }
                }
                Err(TwitchError::Cancelled) => break,
                Err(error @ TwitchError::Reauthorize(_)) => {
                    fail_expired_session(&updates, generation, error);
                    break;
                }
                Err(error) => {
                    let _ = updates.send(TwitchUpdate::Warning {
                        generation,
                        message: error.message(),
                    });
                }
            }
            last_follow_refresh = Instant::now();
            last_live_refresh = Instant::now();
        }
    }
    pubsub_cancel.store(true, Ordering::Release);
}
type PubSubSocket = WebSocket<MaybeTlsStream<TcpStream>>;

fn run_pubsub_listener(
    cancel: Arc<AtomicBool>,
    generation: u64,
    user_id: String,
    token: String,
    updates: Sender<TwitchUpdate>,
) {
    let mut retry_delay = PUBSUB_RECONNECT_INITIAL;
    while !cancel.load(Ordering::Acquire) {
        if let Err(detail) = run_pubsub_session(&cancel, generation, &user_id, &token, &updates) {
            if cancel.load(Ordering::Acquire) {
                break;
            }
            let _ = updates.send(TwitchUpdate::PubSubStatus {
                generation,
                connected: false,
                detail: Some(detail),
            });
        }
        if wait_for_pubsub_retry(&cancel, retry_delay) {
            break;
        }
        retry_delay = retry_delay.saturating_mul(2).min(PUBSUB_RECONNECT_MAX);
    }
}

fn pubsub_listen_request(user_id: &str, token: &str, nonce: &str) -> Value {
    json!({
        "type": "LISTEN",
        "nonce": nonce,
        "data": {
            "topics": [format!("community-points-user-v1.{user_id}")],
            "auth_token": token,
        }
    })
}

fn run_pubsub_session(
    cancel: &AtomicBool,
    generation: u64,
    user_id: &str,
    token: &str,
    updates: &Sender<TwitchUpdate>,
) -> Result<(), String> {
    let (mut socket, _) = connect(PUBSUB_URL).map_err(|error| {
        format!("Could not connect to Twitch Channel Points/Streak events: {error}")
    })?;
    set_pubsub_socket_timeouts(&mut socket)?;

    let topic = format!("community-points-user-v1.{user_id}");
    let nonce = random_state().map_err(TwitchError::message)?;
    let listen = pubsub_listen_request(user_id, token, &nonce);
    socket
        .send(Message::Text(listen.to_string()))
        .map_err(|error| {
            format!("Could not subscribe to Twitch Channel Points/Streak events: {error}")
        })?;

    let mut subscribed = false;
    let mut last_ping = Instant::now();
    let subscribe_deadline = Instant::now() + Duration::from_secs(20);
    while !cancel.load(Ordering::Acquire) {
        if !subscribed && Instant::now() >= subscribe_deadline {
            return Err(
                "Timed out waiting for Twitch Channel Points/Streak event subscription.".to_owned(),
            );
        }
        if last_ping.elapsed() >= PUBSUB_PING_INTERVAL {
            socket
                .send(Message::Text(json!({"type": "PING"}).to_string()))
                .map_err(|error| {
                    format!("Could not ping Twitch Channel Points/Streak event connection: {error}")
                })?;
            last_ping = Instant::now();
        }

        let message = match socket.read() {
            Ok(message) => message,
            Err(WebSocketError::Io(error))
                if matches!(
                    error.kind(),
                    ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                ) =>
            {
                continue;
            }
            Err(error) => {
                return Err(format!(
                    "Twitch Channel Points/Streak event connection failed: {error}"
                ))
            }
        };
        match message {
            Message::Text(text) => handle_pubsub_text(
                &mut socket,
                &text,
                &topic,
                &nonce,
                &mut subscribed,
                generation,
                updates,
            )?,
            Message::Ping(payload) => socket.send(Message::Pong(payload)).map_err(|error| {
                format!("Could not answer Twitch Channel Points/Streak event ping: {error}")
            })?,
            Message::Close(_) => {
                return Err("Twitch closed the Channel Points/Streak event connection.".to_owned())
            }
            _ => {}
        }
    }
    Ok(())
}

fn handle_pubsub_text(
    socket: &mut PubSubSocket,
    text: &str,
    expected_topic: &str,
    expected_nonce: &str,
    subscribed: &mut bool,
    generation: u64,
    updates: &Sender<TwitchUpdate>,
) -> Result<(), String> {
    let frame: Value = serde_json::from_str(text)
        .map_err(|error| format!("Twitch sent invalid Channel Points event data: {error}"))?;
    match frame
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "PING" => socket
            .send(Message::Text(json!({"type": "PONG"}).to_string()))
            .map_err(|error| {
                format!("Could not answer Twitch Channel Points/Streak event ping: {error}")
            })?,
        "PONG" => {}
        "RECONNECT" => {
            return Err("Twitch requested a Channel Points/Streak event reconnect.".to_owned())
        }
        "RESPONSE" => {
            if frame.get("nonce").and_then(Value::as_str) == Some(expected_nonce) {
                let error = frame
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !error.is_empty() {
                    return Err(format!(
                        "Twitch rejected the Channel Points/Streak event subscription: {error}"
                    ));
                }
                if !*subscribed {
                    *subscribed = true;
                    let _ = updates.send(TwitchUpdate::PubSubStatus {
                        generation,
                        connected: true,
                        detail: None,
                    });
                }
            }
        }
        "MESSAGE" => {
            let topic = frame
                .get("data")
                .and_then(|data| data.get("topic"))
                .and_then(Value::as_str);
            if topic == Some(expected_topic) {
                match parse_pubsub_frame(text) {
                    Ok(Some(event)) => dispatch_pubsub_event(generation, event, updates),
                    Ok(None) => {}
                    Err(error) => {
                        let _ = updates.send(TwitchUpdate::PubSubStatus {
                            generation,
                            connected: true,
                            detail: Some(format!("Could not parse Channel Points event: {error}")),
                        });
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn dispatch_pubsub_event(
    generation: u64,
    event: TwitchObservedEvent,
    updates: &Sender<TwitchUpdate>,
) {
    match event {
        TwitchObservedEvent::PointsEarned {
            channel_id,
            points_earned,
            balance,
            reason_code,
        } => {
            if reason_code == "WATCH_STREAK" {
                let _ = updates.send(TwitchUpdate::StreakCompleted {
                    generation,
                    channel_id: channel_id.clone(),
                    points_earned,
                });
            }
            let _ = updates.send(TwitchUpdate::PointsEarned {
                generation,
                channel_id,
                points_earned,
                balance,
                reason_code,
            });
        }
        TwitchObservedEvent::PointsSpent {
            channel_id,
            balance,
        } => {
            let _ = updates.send(TwitchUpdate::PointsSpent {
                generation,
                channel_id,
                balance,
            });
        }
        TwitchObservedEvent::ClaimAvailable { channel_id } => {
            let _ = updates.send(TwitchUpdate::ClaimAvailable {
                generation,
                channel_id,
            });
        }
    }
}

fn set_pubsub_socket_timeouts(socket: &mut PubSubSocket) -> Result<(), String> {
    let stream = match socket.get_mut() {
        MaybeTlsStream::Plain(stream) => stream,
        MaybeTlsStream::Rustls(stream) => stream.get_mut(),
        _ => return Err("Unsupported Twitch PubSub TLS stream.".to_owned()),
    };
    stream
        .set_read_timeout(Some(PUBSUB_READ_TIMEOUT))
        .and_then(|()| stream.set_write_timeout(Some(REQUEST_TIMEOUT)))
        .map_err(|error| {
            format!("Could not configure Twitch Channel Points/Streak event connection: {error}")
        })
}

fn wait_for_pubsub_retry(cancel: &AtomicBool, delay: Duration) -> bool {
    let deadline = Instant::now() + delay;
    while !cancel.load(Ordering::Acquire) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(100));
    }
    cancel.load(Ordering::Acquire)
}

fn send_failed(updates: &Sender<TwitchUpdate>, generation: u64, message: String) {
    let _ = updates.send(TwitchUpdate::Failed {
        generation,
        message,
    });
}

fn fail_expired_session(updates: &Sender<TwitchUpdate>, generation: u64, error: TwitchError) {
    let message = match clear_keyring_token() {
        Ok(()) => error.message(),
        Err(clear_error) => format!(
            "{}; could not remove saved token: {}",
            error.message(),
            clear_error.message()
        ),
    };
    send_failed(updates, generation, message);
}

fn build_client() -> Result<Client, TwitchError> {
    Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent("Twitch Streak Saver/0.1")
        .build()
        .map_err(|error| {
            TwitchError::Failed(format!("Could not initialize Twitch networking: {error}"))
        })
}

fn obtain_token(client: &Client, cancel: &AtomicBool) -> Result<(String, TwitchUser), TwitchError> {
    if let Some(token) = load_keyring_token()? {
        match validate_token(client, &token) {
            Ok(user) => return Ok((token, user)),
            Err(TwitchError::Reauthorize(_)) => clear_keyring_token()?,
            Err(error) => return Err(error),
        }
    }
    if cancel.load(Ordering::Acquire) {
        return Err(TwitchError::Cancelled);
    }

    let token = authorize_in_browser(cancel)?;
    let user = validate_token(client, &token).map_err(|error| match error {
        TwitchError::Reauthorize(message) => TwitchError::Failed(message),
        error => error,
    })?;
    store_keyring_token(&token)?;
    Ok((token, user))
}

fn validate_token(client: &Client, token: &str) -> Result<TwitchUser, TwitchError> {
    if !valid_token(token) {
        return Err(TwitchError::Reauthorize(
            "Saved Twitch authorization is invalid. Sign in again.".to_owned(),
        ));
    }
    let response = client
        .get(VALIDATE_URL)
        .header("Authorization", format!("OAuth {token}"))
        .send()
        .map_err(|error| {
            TwitchError::Failed(format!("Could not validate Twitch authorization: {error}"))
        })?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED
        || response.status() == reqwest::StatusCode::FORBIDDEN
    {
        return Err(TwitchError::Reauthorize(
            "Twitch authorization expired or was rejected. Sign in again.".to_owned(),
        ));
    }
    let validation: ValidateResponse = parse_response(response, "validate Twitch authorization")?;
    user_from_validation(validation)
}

fn user_from_validation(validation: ValidateResponse) -> Result<TwitchUser, TwitchError> {
    if validation.client_id != CLIENT_ID {
        return Err(TwitchError::Reauthorize(
            "Twitch token belongs to a different client. Sign in again.".to_owned(),
        ));
    }
    if !validation
        .scopes
        .iter()
        .any(|scope| scope == REQUIRED_SCOPE)
    {
        return Err(TwitchError::Reauthorize(
            "Twitch authorization lacks user:read:follows. Sign in again and grant the requested permission.".to_owned(),
        ));
    }
    if validation.login.is_empty() || validation.user_id.is_empty() {
        return Err(TwitchError::Failed(
            "Twitch returned an incomplete account identity.".to_owned(),
        ));
    }
    Ok(TwitchUser {
        login: validation.login,
        id: validation.user_id,
    })
}

fn parse_response<T: for<'de> Deserialize<'de>>(
    response: Response,
    operation: &str,
) -> Result<T, TwitchError> {
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(TwitchError::Reauthorize(format!(
            "Twitch rejected authorization while trying to {operation}. Sign in again."
        )));
    }
    if !status.is_success() {
        return Err(TwitchError::Failed(format!(
            "Twitch could not {operation} (HTTP {status})."
        )));
    }
    response.json().map_err(|error| {
        TwitchError::Failed(format!(
            "Twitch returned invalid data while trying to {operation}: {error}"
        ))
    })
}

fn fetch_followed_channels(
    client: &Client,
    token: &str,
    user: &TwitchUser,
    cancel: &AtomicBool,
) -> Result<Vec<FollowedChannel>, TwitchError> {
    let mut channels = Vec::new();
    let mut channel_ids = HashSet::new();
    let mut cursor: Option<String> = None;
    let mut seen_cursors = HashSet::new();

    for _ in 0..MAX_FOLLOW_PAGES {
        if cancel.load(Ordering::Acquire) {
            return Err(TwitchError::Cancelled);
        }
        let mut request = client
            .get(FOLLOWED_URL)
            .header("Client-ID", CLIENT_ID)
            .bearer_auth(token)
            .query(&[("user_id", user.id.as_str()), ("first", "100")]);
        if let Some(after) = cursor.as_deref() {
            request = request.query(&[("after", after)]);
        }
        let response = request.send().map_err(|error| {
            TwitchError::Failed(format!("Could not load followed Twitch channels: {error}"))
        })?;
        let page: FollowedResponse = parse_response(response, "load followed channels")?;
        for record in page.data {
            if channel_ids.insert(record.broadcaster_id.clone()) {
                let mut channel =
                    FollowedChannel::new(record.broadcaster_id, record.broadcaster_name);
                channel.login = record.broadcaster_login;
                channels.push(channel);
            }
        }
        cursor = page.pagination.and_then(|pagination| pagination.cursor);
        match cursor.as_ref() {
            Some(next) if seen_cursors.insert(next.clone()) => {}
            Some(_) => {
                return Err(TwitchError::Failed(
                    "Twitch returned a repeated followed-channel page cursor.".to_owned(),
                ));
            }
            None => return Ok(channels),
        }
    }
    Err(TwitchError::Failed(
        "Twitch followed-channel list exceeded the pagination safety limit.".to_owned(),
    ))
}

fn fetch_live_channel_ids(
    client: &Client,
    token: &str,
    channels: &[FollowedChannel],
    cancel: &AtomicBool,
) -> Result<HashSet<String>, TwitchError> {
    let mut live_ids = HashSet::new();
    for batch in channels.chunks(100) {
        if cancel.load(Ordering::Acquire) {
            return Err(TwitchError::Cancelled);
        }
        let user_ids: Vec<(&str, &str)> = batch
            .iter()
            .map(|channel| ("user_id", channel.id.as_str()))
            .collect();
        let response = client
            .get(STREAMS_URL)
            .header("Client-ID", CLIENT_ID)
            .bearer_auth(token)
            .query(&user_ids)
            .send()
            .map_err(|error| {
                TwitchError::Failed(format!("Could not load Twitch live status: {error}"))
            })?;
        let streams: StreamsResponse = parse_response(response, "load live status")?;
        live_ids.extend(streams.data.into_iter().map(|stream| stream.user_id));
    }
    Ok(live_ids)
}

fn authorize_in_browser(cancel: &AtomicBool) -> Result<String, TwitchError> {
    let state = random_state()?;
    let listener_v4 = TcpListener::bind("127.0.0.1:65432").map_err(|error| {
        TwitchError::Failed(format!(
            "Could not start Twitch sign-in callback on localhost:65432: {error}"
        ))
    })?;
    listener_v4.set_nonblocking(true).map_err(|error| {
        TwitchError::Failed(format!("Could not configure Twitch callback: {error}"))
    })?;
    let listener_v6 = TcpListener::bind("[::1]:65432").ok();
    if let Some(listener) = &listener_v6 {
        listener.set_nonblocking(true).map_err(|error| {
            TwitchError::Failed(format!("Could not configure Twitch callback: {error}"))
        })?;
    }

    let url = build_authorization_url(&state)?;
    webbrowser::open(url.as_str()).map_err(|error| {
        TwitchError::Failed(format!(
            "Could not open Twitch sign-in in a browser: {error}"
        ))
    })?;

    let deadline = Instant::now() + OAUTH_TIMEOUT;
    while Instant::now() < deadline {
        if cancel.load(Ordering::Acquire) {
            return Err(TwitchError::Cancelled);
        }
        if let Some(token) = accept_callback(&listener_v4, &state)? {
            return Ok(token);
        }
        if let Some(listener) = &listener_v6 {
            if let Some(token) = accept_callback(listener, &state)? {
                return Ok(token);
            }
        }
        thread::sleep(Duration::from_millis(25));
    }
    Err(TwitchError::Failed(
        "Twitch sign-in timed out. Connect again to retry.".to_owned(),
    ))
}

fn accept_callback(
    listener: &TcpListener,
    expected_state: &str,
) -> Result<Option<String>, TwitchError> {
    match listener.accept() {
        Ok((mut stream, _)) => handle_callback_request(&mut stream, expected_state),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
        Err(error) => Err(TwitchError::Failed(format!(
            "Twitch sign-in callback failed: {error}"
        ))),
    }
}

fn handle_callback_request(
    stream: &mut TcpStream,
    expected_state: &str,
) -> Result<Option<String>, TwitchError> {
    let request = match read_http_request(stream) {
        Ok(request) => request,
        Err(message) => {
            let _ = respond(stream, "400 Bad Request", "text/plain", "Invalid request.");
            return Err(TwitchError::Failed(message));
        }
    };
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/redirect") => {
            respond(stream, "200 OK", "text/html; charset=utf-8", OAUTH_HTML)?;
            Ok(None)
        }
        ("GET", "/oauth.js") => {
            respond_with_policy(
                stream,
                "200 OK",
                "application/javascript; charset=utf-8",
                OAUTH_JS,
            )?;
            Ok(None)
        }
        ("POST", "/token") => {
            if request.body.len() > MAX_HTTP_BODY_BYTES {
                respond(
                    stream,
                    "413 Payload Too Large",
                    "text/plain",
                    "Response too large.",
                )?;
                return Ok(None);
            }
            let callback: OAuthCallback = match serde_json::from_slice(&request.body) {
                Ok(callback) => callback,
                Err(_) => {
                    respond(
                        stream,
                        "400 Bad Request",
                        "text/plain",
                        "Invalid OAuth response.",
                    )?;
                    return Ok(None);
                }
            };
            if callback.state.as_deref() != Some(expected_state) {
                respond(
                    stream,
                    "400 Bad Request",
                    "text/plain",
                    "OAuth state did not match.",
                )?;
                return Ok(None);
            }
            let token = match token_from_callback(callback) {
                Ok(token) => token,
                Err(message) => {
                    respond(
                        stream,
                        "400 Bad Request",
                        "text/plain",
                        "Twitch sign-in was not accepted.",
                    )?;
                    return Err(TwitchError::Failed(message));
                }
            };
            respond(
                stream,
                "200 OK",
                "text/plain; charset=utf-8",
                "Twitch sign-in received. You can return to the app.",
            )?;
            Ok(Some(token))
        }
        _ => {
            respond(stream, "404 Not Found", "text/plain", "Not found.")?;
            Ok(None)
        }
    }
}

fn read_http_request(stream: &mut TcpStream) -> Result<HttpRequest, String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| format!("Could not read Twitch callback: {error}"))?;
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 2048];
    let header_end = loop {
        if let Some(index) = find_bytes(&bytes, b"\r\n\r\n") {
            break index + 4;
        }
        if bytes.len() >= MAX_HTTP_HEADER_BYTES {
            return Err("Twitch callback request headers exceeded the size limit.".to_owned());
        }
        let count = stream
            .read(&mut buffer)
            .map_err(|error| format!("Could not read Twitch callback request: {error}"))?;
        if count == 0 {
            return Err("Twitch callback closed before sending a request.".to_owned());
        }
        bytes.extend_from_slice(&buffer[..count]);
    };
    if header_end > MAX_HTTP_HEADER_BYTES {
        return Err("Twitch callback request headers exceeded the size limit.".to_owned());
    }

    let header_text = std::str::from_utf8(&bytes[..header_end])
        .map_err(|_| "Twitch callback sent invalid HTTP headers.".to_owned())?;
    let mut lines = header_text.split("\r\n");
    let mut request_line = lines
        .next()
        .ok_or_else(|| "Twitch callback omitted the HTTP request line.".to_owned())?
        .split_whitespace();
    let method = request_line
        .next()
        .ok_or_else(|| "Twitch callback omitted the HTTP method.".to_owned())?
        .to_owned();
    let target = request_line
        .next()
        .ok_or_else(|| "Twitch callback omitted the request path.".to_owned())?;
    let path = target.split('?').next().unwrap_or_default().to_owned();
    let content_length = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .map(|(_, value)| value.trim().parse::<usize>())
        .transpose()
        .map_err(|_| "Twitch callback sent an invalid content length.".to_owned())?
        .unwrap_or(0);
    if content_length > MAX_HTTP_BODY_BYTES {
        return Err("Twitch callback request body exceeded the size limit.".to_owned());
    }

    while bytes.len() - header_end < content_length {
        let count = stream
            .read(&mut buffer)
            .map_err(|error| format!("Could not read Twitch callback body: {error}"))?;
        if count == 0 {
            return Err("Twitch callback closed before sending its full body.".to_owned());
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    Ok(HttpRequest {
        method,
        path,
        body: bytes[header_end..header_end + content_length].to_vec(),
    })
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn respond(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &str,
) -> Result<(), TwitchError> {
    respond_with_policy(stream, status, content_type, body)
}

fn respond_with_policy(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &str,
) -> Result<(), TwitchError> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: default-src 'none'; script-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'none'\r\n\r\n{body}",
        body.len()
    )
    .map_err(|error| TwitchError::Failed(format!("Could not reply to Twitch callback: {error}")))
}

fn token_from_callback(callback: OAuthCallback) -> Result<String, String> {
    if let Some(error) = callback.error {
        let description = callback.error_description.unwrap_or(error.clone());
        return Err(format!("Twitch authorization failed: {description}"));
    }
    if !callback
        .token_type
        .as_deref()
        .is_some_and(|token_type| token_type.eq_ignore_ascii_case("bearer"))
    {
        return Err("Twitch returned an unsupported token type.".to_owned());
    }
    let token = callback
        .access_token
        .ok_or_else(|| "Twitch did not return an access token.".to_owned())?;
    if !valid_token(&token) {
        return Err("Twitch returned an invalid access token.".to_owned());
    }
    let has_scope = callback
        .scope
        .unwrap_or_default()
        .split(|character: char| character == ' ' || character == '+')
        .any(|scope| scope == REQUIRED_SCOPE);
    if !has_scope {
        return Err("Twitch sign-in did not grant user:read:follows.".to_owned());
    }
    Ok(token)
}

fn valid_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= MAX_HTTP_BODY_BYTES
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

fn random_state() -> Result<String, TwitchError> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|error| {
        TwitchError::Failed(format!(
            "Could not create secure Twitch sign-in state: {error}"
        ))
    })?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn build_authorization_url(state: &str) -> Result<Url, TwitchError> {
    let mut url = Url::parse(OAUTH_URL)
        .map_err(|error| TwitchError::Failed(format!("Invalid Twitch sign-in URL: {error}")))?;
    url.query_pairs_mut()
        .append_pair("response_type", "token")
        .append_pair("client_id", CLIENT_ID)
        .append_pair("redirect_uri", REDIRECT_URI)
        .append_pair("scope", REQUIRED_SCOPE)
        .append_pair("force_verify", "true")
        .append_pair("state", state);
    Ok(url)
}

const OAUTH_HTML: &str = "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"referrer\" content=\"no-referrer\"><title>Twitch sign-in</title></head><body><p>Completing Twitch sign-in…</p><script src=\"/oauth.js\" defer></script></body></html>";

const OAUTH_JS: &str = r#"const params = new URLSearchParams(location.hash.slice(1) || location.search.slice(1));
const response = Object.fromEntries(params.entries());
history.replaceState(null, "", location.pathname);
fetch("/token", {method: "POST", headers: {"Content-Type": "application/json"}, body: JSON.stringify(response)}).then(async result => {
  document.body.textContent = result.ok ? "Twitch sign-in received. You can return to the app." : "Twitch sign-in failed. Return to the app to retry.";
}).catch(() => {
  document.body.textContent = "Could not return Twitch sign-in to the app.";
});"#;

fn load_keyring_token() -> Result<Option<String>, TwitchError> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT).map_err(|error| {
            TwitchError::Failed(format!("Could not open secure token storage: {error}"))
        })?;
        return match entry.get_password() {
            Ok(token) => Ok(Some(token)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(TwitchError::Failed(format!(
                "Could not read secure token storage: {error}"
            ))),
        };
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Err(TwitchError::Failed(
            "Secure Twitch token storage is supported on macOS and Windows.".to_owned(),
        ))
    }
}

fn store_keyring_token(token: &str) -> Result<(), TwitchError> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT).map_err(|error| {
            TwitchError::Failed(format!("Could not open secure token storage: {error}"))
        })?;
        return entry.set_password(token).map_err(|error| {
            TwitchError::Failed(format!("Could not save Twitch token securely: {error}"))
        });
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = token;
        Err(TwitchError::Failed(
            "Secure Twitch token storage is supported on macOS and Windows.".to_owned(),
        ))
    }
}

fn clear_keyring_token() -> Result<(), TwitchError> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT).map_err(|error| {
            TwitchError::Failed(format!("Could not open secure token storage: {error}"))
        })?;
        return match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(TwitchError::Failed(format!(
                "Could not remove saved Twitch token: {error}"
            ))),
        };
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Err(TwitchError::Failed(
            "Secure Twitch token storage is supported on macOS and Windows.".to_owned(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_authorization_url, dispatch_pubsub_event, pubsub_listen_request, token_from_callback,
        user_from_validation, OAuthCallback, TwitchUpdate, ValidateResponse, CLIENT_ID,
        REDIRECT_URI, REQUIRED_SCOPE,
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
