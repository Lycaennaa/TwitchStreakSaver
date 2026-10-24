use super::*;
pub(super) fn build_client() -> Result<Client, TwitchError> {
    Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .user_agent("Twitch Streak Saver/0.1")
        .build()
        .map_err(|error| {
            TwitchError::Failed(format!("Could not initialize Twitch networking: {error}"))
        })
}

pub(super) fn validate_token(client: &Client, token: &str) -> Result<TwitchUser, TwitchError> {
    let response = client
        .get(VALIDATE_URL)
        .header("Authorization", format!("OAuth {token}"))
        .send()
        .map_err(|error| request_error(error, "Could not validate Twitch authorization"))?;
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

pub(super) fn user_from_validation(
    validation: ValidateResponse,
) -> Result<TwitchUser, TwitchError> {
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
        let message = format!("Twitch could not {operation} (HTTP {status}).");
        return Err(
            if status.is_server_error()
                || status == reqwest::StatusCode::TOO_MANY_REQUESTS
                || status == reqwest::StatusCode::REQUEST_TIMEOUT
            {
                TwitchError::Retryable(message)
            } else {
                TwitchError::Failed(message)
            },
        );
    }
    response.json().map_err(|error| {
        request_error(
            error,
            &format!("Twitch returned invalid data while trying to {operation}"),
        )
    })
}
fn request_error(error: reqwest::Error, operation: &str) -> TwitchError {
    let message = format!("{operation}: {error}");
    if error.is_connect() || error.is_timeout() || error.is_body() {
        TwitchError::Retryable(message)
    } else {
        TwitchError::Failed(message)
    }
}

pub(super) fn fetch_followed_channels(
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
        let response = request
            .send()
            .map_err(|error| request_error(error, "Could not load followed Twitch channels"))?;
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

pub(super) fn fetch_live_channel_ids(
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
            .map_err(|error| request_error(error, "Could not load Twitch live status"))?;
        let streams: StreamsResponse = parse_response(response, "load live status")?;
        live_ids.extend(streams.data.into_iter().map(|stream| stream.user_id));
    }
    Ok(live_ids)
}
