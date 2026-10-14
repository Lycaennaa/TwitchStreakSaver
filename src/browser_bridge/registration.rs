use super::{BridgeEvent, EXTENSION_REGISTRATION_ADDRESS, TRUSTED_EXTENSION_ID};
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

const MAX_REGISTRATION_REQUEST_SIZE: usize = 4096;
const REGISTRATION_READ_TIMEOUT: Duration = Duration::from_secs(2);

pub(super) fn serve_extension_registrations(
    listener: TcpListener,
    events: Sender<BridgeEvent>,
    stop: Arc<AtomicBool>,
) {
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, address)) if address.ip().is_loopback() => {
                handle_extension_registration(stream);
            }
            Ok((_stream, _)) => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => {
                let _ = events.send(BridgeEvent::Failed(format!(
                    "Extension registration listener failed: {error}"
                )));
                break;
            }
        }
    }
}

fn handle_extension_registration(mut stream: TcpStream) {
    if stream
        .set_read_timeout(Some(REGISTRATION_READ_TIMEOUT))
        .is_err()
        || stream
            .set_write_timeout(Some(REGISTRATION_READ_TIMEOUT))
            .is_err()
    {
        return;
    }
    let request = match read_registration_headers(&mut stream) {
        Ok(request) => request,
        Err(_) => {
            let _ = write_registration_response(&mut stream, "400 Bad Request", None, false);
            return;
        }
    };
    let request = match parse_registration_request(&request) {
        Ok(request) => request,
        Err(_) => {
            let _ = write_registration_response(&mut stream, "403 Forbidden", None, false);
            return;
        }
    };
    if request.method == RegistrationMethod::Options {
        let _ = write_registration_response(
            &mut stream,
            "204 No Content",
            Some(TRUSTED_EXTENSION_ID),
            request.private_network,
        );
        return;
    }
    let _ = write_registration_response(
        &mut stream,
        "204 No Content",
        Some(TRUSTED_EXTENSION_ID),
        false,
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RegistrationMethod {
    Get,
    Options,
}

struct RegistrationRequest {
    method: RegistrationMethod,
    private_network: bool,
}

fn parse_registration_request(bytes: &[u8]) -> Result<RegistrationRequest, String> {
    if bytes.len() > MAX_REGISTRATION_REQUEST_SIZE || !bytes.ends_with(b"\r\n\r\n") {
        return Err("Invalid extension registration request.".to_owned());
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| "Invalid extension registration request.".to_owned())?;
    let headers = text
        .strip_suffix("\r\n\r\n")
        .ok_or_else(|| "Invalid extension registration request.".to_owned())?;
    let mut lines = headers.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| "Invalid extension registration request.".to_owned())?;
    let mut request_parts = request_line.split_whitespace();
    let method = match request_parts.next() {
        Some("GET") => RegistrationMethod::Get,
        Some("OPTIONS") => RegistrationMethod::Options,
        _ => return Err("Unsupported extension registration method.".to_owned()),
    };
    let target = request_parts
        .next()
        .ok_or_else(|| "Invalid extension registration request.".to_owned())?;
    if request_parts.next() != Some("HTTP/1.1") || request_parts.next().is_some() {
        return Err("Invalid extension registration request.".to_owned());
    }
    let extension_id = target
        .strip_prefix("/register?id=")
        .filter(|id| !id.contains('&') && valid_extension_id(id))
        .ok_or_else(|| "Invalid extension ID.".to_owned())?;
    if extension_id != TRUSTED_EXTENSION_ID {
        return Err("Extension ID is not trusted.".to_owned());
    }

    let mut host = None;
    let mut origin = None;
    let mut content_length = None;
    let mut access_control_method = None;
    let mut access_control_headers = None;
    let mut private_network = false;
    let mut transfer_encoding = false;
    for line in lines {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| "Invalid extension registration header.".to_owned())?;
        let value = value.trim();
        let slot = match name {
            header if header.eq_ignore_ascii_case("host") => &mut host,
            header if header.eq_ignore_ascii_case("origin") => &mut origin,
            header if header.eq_ignore_ascii_case("content-length") => &mut content_length,
            header if header.eq_ignore_ascii_case("access-control-request-method") => {
                &mut access_control_method
            }
            header if header.eq_ignore_ascii_case("access-control-request-headers") => {
                &mut access_control_headers
            }
            header if header.eq_ignore_ascii_case("access-control-request-private-network") => {
                if value != "true" || private_network {
                    return Err("Invalid private network preflight.".to_owned());
                }
                private_network = true;
                continue;
            }
            header if header.eq_ignore_ascii_case("transfer-encoding") => {
                transfer_encoding = true;
                continue;
            }
            _ => continue,
        };
        if slot.replace(value).is_some() {
            return Err("Duplicate extension registration header.".to_owned());
        }
    }
    if host != Some(EXTENSION_REGISTRATION_ADDRESS)
        || origin.and_then(|origin| origin.strip_prefix("chrome-extension://"))
            != Some(extension_id)
        || content_length.is_some_and(|length| length != "0")
        || transfer_encoding
        || access_control_headers.is_some_and(|headers| !headers.is_empty())
    {
        return Err("Extension registration origin did not match its ID.".to_owned());
    }
    match method {
        RegistrationMethod::Get if access_control_method.is_none() => {}
        RegistrationMethod::Options if access_control_method == Some("GET") => {}
        _ => return Err("Invalid extension registration preflight.".to_owned()),
    }

    Ok(RegistrationRequest {
        method,
        private_network,
    })
}

fn read_registration_headers(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
    let mut request = Vec::with_capacity(512);
    let mut buffer = [0u8; 512];
    loop {
        let count = stream.read(&mut buffer)?;
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Extension registration request ended before its headers.",
            ));
        }
        request.extend_from_slice(&buffer[..count]);
        if request.len() > MAX_REGISTRATION_REQUEST_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Extension registration request exceeded the size limit.",
            ));
        }
        if let Some(end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            if end + 4 != request.len() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Extension registration request had a body.",
                ));
            }
            return Ok(request);
        }
    }
}

fn write_registration_response(
    stream: &mut TcpStream,
    status: &str,
    extension_id: Option<&str>,
    allow_private_network: bool,
) -> io::Result<()> {
    let mut response = format!(
        "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\nCache-Control: no-store\r\n"
    );
    if let Some(extension_id) = extension_id {
        response.push_str(&format!(
            "Access-Control-Allow-Origin: chrome-extension://{extension_id}\r\nVary: Origin\r\n"
        ));
        response.push_str("Access-Control-Allow-Methods: GET, OPTIONS\r\n");
    }
    if allow_private_network {
        response.push_str("Access-Control-Allow-Private-Network: true\r\n");
    }
    response.push_str("\r\n");
    stream.write_all(response.as_bytes())
}
fn valid_extension_id(extension_id: &str) -> bool {
    extension_id.len() == 32
        && extension_id
            .bytes()
            .all(|byte| (b'a'..=b'p').contains(&byte))
}
#[cfg(test)]
mod tests {
    use super::{
        handle_extension_registration, parse_registration_request, valid_extension_id,
        RegistrationMethod, EXTENSION_REGISTRATION_ADDRESS, TRUSTED_EXTENSION_ID,
    };
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    #[test]
    fn extension_id_validation_rejects_malformed_ids() {
        assert!(valid_extension_id("a".repeat(32).as_str()));
        assert!(!valid_extension_id("a".repeat(31).as_str()));
        assert!(!valid_extension_id("z".repeat(32).as_str()));
    }
    #[test]
    fn loopback_registration_requires_the_trusted_extension_origin() {
        let extension_id = TRUSTED_EXTENSION_ID;
        let origin = format!("chrome-extension://{extension_id}");
        let request = format!(
            "GET /register?id={extension_id} HTTP/1.1\r\nHost: {EXTENSION_REGISTRATION_ADDRESS}\r\nOrigin: {origin}\r\n\r\n"
        );
        let registration = parse_registration_request(request.as_bytes()).unwrap();
        assert_eq!(registration.method, RegistrationMethod::Get);
        let mixed_case_headers = request
            .replace("Host:", "hOsT:")
            .replace("Origin:", "oRiGiN:");
        assert_eq!(
            parse_registration_request(mixed_case_headers.as_bytes())
                .unwrap()
                .method,
            RegistrationMethod::Get
        );

        let page_origin = request.replace(&origin, "https://attacker.example");
        assert!(parse_registration_request(page_origin.as_bytes()).is_err());

        let other_id = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let other_origin = request
            .replace(extension_id, other_id)
            .replace(&origin, &format!("chrome-extension://{other_id}"));
        assert!(parse_registration_request(other_origin.as_bytes()).is_err());
    }

    #[test]
    fn only_the_trusted_extension_can_sync_native_host_access() {
        let extension_id = TRUSTED_EXTENSION_ID;
        let request = format!(
            "GET /register?id={extension_id} HTTP/1.1\r\nHost: {EXTENSION_REGISTRATION_ADDRESS}\r\nOrigin: chrome-extension://{extension_id}\r\n\r\n"
        );
        let response = exchange_registration(request.as_bytes());
        assert!(response.starts_with("HTTP/1.1 204 No Content\r\n"));

        let other_id = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let untrusted_request = format!(
            "GET /register?id={other_id} HTTP/1.1\r\nHost: {EXTENSION_REGISTRATION_ADDRESS}\r\nOrigin: chrome-extension://{other_id}\r\n\r\n"
        );
        let response = exchange_registration(untrusted_request.as_bytes());
        assert!(response.starts_with("HTTP/1.1 403 Forbidden\r\n"));
    }

    #[test]
    fn accepts_private_network_preflight_only_for_the_trusted_extension() {
        let extension_id = TRUSTED_EXTENSION_ID;
        let request = format!(
            "OPTIONS /register?id={extension_id} HTTP/1.1\r\nHost: {EXTENSION_REGISTRATION_ADDRESS}\r\nOrigin: chrome-extension://{extension_id}\r\nAccess-Control-Request-Method: GET\r\nAccess-Control-Request-Private-Network: true\r\n\r\n"
        );
        let registration = parse_registration_request(request.as_bytes()).unwrap();
        assert_eq!(registration.method, RegistrationMethod::Options);
        assert!(registration.private_network);

        let response = exchange_registration(request.as_bytes());
        assert!(response.starts_with("HTTP/1.1 204 No Content\r\n"));
        assert!(response.contains("Access-Control-Allow-Private-Network: true\r\n"));
    }

    fn exchange_registration(request: &[u8]) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        client.write_all(request).unwrap();
        handle_extension_registration(server);
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        response
    }
}
