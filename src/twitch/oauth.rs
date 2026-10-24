use super::api;
use super::*;
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
    Err(oauth_callback_timeout())
}

fn oauth_callback_timeout() -> TwitchError {
    TwitchError::Retryable("Twitch sign-in timed out; retrying connection.".to_owned())
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
            let _ =
                respond_with_policy(stream, "400 Bad Request", "text/plain", "Invalid request.");
            return Err(TwitchError::Failed(message));
        }
    };
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/redirect") => {
            respond_with_policy(stream, "200 OK", "text/html; charset=utf-8", OAUTH_HTML)?;
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
                respond_with_policy(
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
                    respond_with_policy(
                        stream,
                        "400 Bad Request",
                        "text/plain",
                        "Invalid OAuth response.",
                    )?;
                    return Ok(None);
                }
            };
            if callback.state.as_deref() != Some(expected_state) {
                respond_with_policy(
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
                    respond_with_policy(
                        stream,
                        "400 Bad Request",
                        "text/plain",
                        "Twitch sign-in was not accepted.",
                    )?;
                    return Err(TwitchError::Failed(message));
                }
            };
            respond_with_policy(
                stream,
                "200 OK",
                "text/plain; charset=utf-8",
                "Twitch sign-in received. You can return to the app.",
            )?;
            Ok(Some(token))
        }
        _ => {
            respond_with_policy(stream, "404 Not Found", "text/plain", "Not found.")?;
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

pub(super) fn token_from_callback(callback: OAuthCallback) -> Result<String, String> {
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

pub(super) fn valid_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= MAX_HTTP_BODY_BYTES
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

pub(super) fn random_state() -> Result<String, TwitchError> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|error| {
        TwitchError::Failed(format!(
            "Could not create secure Twitch sign-in state: {error}"
        ))
    })?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub(super) fn build_authorization_url(state: &str) -> Result<Url, TwitchError> {
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

pub(super) fn clear_keyring_token() -> Result<(), TwitchError> {
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
pub fn clear_saved_token() -> Result<(), String> {
    clear_keyring_token().map_err(TwitchError::message)
}
pub(super) fn obtain_token(
    client: &Client,
    cancel: &AtomicBool,
) -> Result<(String, TwitchUser), TwitchError> {
    if let Some(token) = load_keyring_token()? {
        if !valid_token(&token) {
            clear_keyring_token()?;
        } else {
            match api::validate_token(client, &token) {
                Ok(user) => return Ok((token, user)),
                Err(TwitchError::Reauthorize(_)) => clear_keyring_token()?,
                Err(error) => return Err(error),
            }
        }
    }
    if cancel.load(Ordering::Acquire) {
        return Err(TwitchError::Cancelled);
    }

    let token = authorize_in_browser(cancel)?;
    let user = api::validate_token(client, &token).map_err(|error| match error {
        TwitchError::Reauthorize(message) => TwitchError::Failed(message),
        error => error,
    })?;
    store_keyring_token(&token)?;
    Ok((token, user))
}

#[cfg(test)]
mod tests {
    use super::{oauth_callback_timeout, TwitchError};

    #[test]
    fn oauth_callback_timeout_is_retryable() {
        assert!(matches!(
            oauth_callback_timeout(),
            TwitchError::Retryable(_)
        ));
    }
}
