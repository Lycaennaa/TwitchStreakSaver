use super::oauth;
use super::*;
type PubSubSocket = WebSocket<MaybeTlsStream<TcpStream>>;

pub(super) fn run_pubsub_listener(
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

pub(super) fn pubsub_listen_request(user_id: &str, token: &str, nonce: &str) -> Value {
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
    let nonce = oauth::random_state().map_err(TwitchError::message)?;
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

pub(super) fn dispatch_pubsub_event(
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
