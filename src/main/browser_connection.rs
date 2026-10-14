use super::*;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};

fn check_browser_connection_port() -> Result<(), String> {
    let address = EXTENSION_REGISTRATION_ADDRESS
        .parse::<SocketAddr>()
        .map_err(|error| format!("Invalid browser registration address: {error}"))?;
    check_browser_connection_port_at(address)
}

fn check_browser_connection_port_at(address: SocketAddr) -> Result<(), String> {
    let timeout = Duration::from_secs(2);
    let mut stream = TcpStream::connect_timeout(&address, timeout)
        .map_err(|error| format!("Could not reach {address}: {error}"))?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|error| format!("Could not configure port check: {error}"))?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|error| format!("Could not configure port check: {error}"))?;
    write!(
        stream,
        "GET /register?id={BROWSER_EXTENSION_ID} HTTP/1.1\r\nHost: {EXTENSION_REGISTRATION_ADDRESS}\r\nOrigin: chrome-extension://{BROWSER_EXTENSION_ID}\r\n\r\n"
    )
    .map_err(|error| format!("Could not send browser port check: {error}"))?;

    const EXPECTED_STATUS_LINE: &[u8] = b"HTTP/1.1 204 No Content\r\n";
    let mut status_line = [0; EXPECTED_STATUS_LINE.len()];
    stream
        .read_exact(&mut status_line)
        .map_err(|error| format!("Could not read browser port check response: {error}"))?;
    if status_line.as_slice() != EXPECTED_STATUS_LINE {
        return Err(format!(
            "Browser registration port {address} returned an unexpected response."
        ));
    }
    Ok(())
}

impl Dashboard {
    pub(super) fn connect_to_browser(&mut self) {
        if self.browser_port_check_in_flight {
            return;
        }
        self.browser_port_check_in_flight = true;
        self.browser_notice = Some("Checking the browser connection port.".to_owned());
        let sender = self.browser_port_check_sender.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("browser-port-check".to_owned())
            .spawn(move || {
                let _ = sender.send(check_browser_connection_port());
            })
        {
            self.browser_port_check_in_flight = false;
            self.browser_notice = Some(format!("Could not start browser port check: {error}"));
        }
    }

    pub(super) fn poll_browser_port_check(&mut self) {
        let Ok(result) = self.browser_port_check_receiver.try_recv() else {
            return;
        };
        self.browser_port_check_in_flight = false;
        match result {
            Ok(()) => {
                self.browser_notice = Some(if self.browser_connected {
                    format!(
                        "Browser connected; port {EXTENSION_REGISTRATION_ADDRESS} responds. Click Open selected channels to load tabs."
                    )
                } else {
                    format!(
                        "Port {EXTENSION_REGISTRATION_ADDRESS} responds, but Browser not connected. This check does not open tabs."
                    )
                });
            }
            Err(error) => {
                self.browser_notice = Some(if self.browser_connected {
                    format!("Browser connected, but port check failed: {error}")
                } else {
                    format!("Browser not connected; port check failed: {error}")
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        check_browser_connection_port_at, BROWSER_EXTENSION_ID, EXTENSION_REGISTRATION_ADDRESS,
    };
    use std::io::{BufRead, BufReader, Write};
    use std::net::{SocketAddr, TcpListener};
    use std::thread::{self, JoinHandle};

    fn start_port_check_server(response: &'static [u8]) -> (SocketAddr, JoinHandle<String>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut request = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                request.push_str(&line);
                if line == "\r\n" {
                    break;
                }
            }
            reader.get_mut().write_all(response).unwrap();
            request
        });
        (address, server)
    }

    #[test]
    fn browser_port_check_probes_the_extension_registration_endpoint() {
        let (address, server) = start_port_check_server(b"HTTP/1.1 204 No Content\r\n\r\n");

        assert_eq!(check_browser_connection_port_at(address), Ok(()));
        assert_eq!(
            server.join().unwrap(),
            format!(
                "GET /register?id={BROWSER_EXTENSION_ID} HTTP/1.1\r\nHost: {EXTENSION_REGISTRATION_ADDRESS}\r\nOrigin: chrome-extension://{BROWSER_EXTENSION_ID}\r\n\r\n"
            )
        );
    }

    #[test]
    fn browser_port_check_rejects_unexpected_response_status() {
        let (address, server) = start_port_check_server(b"HTTP/1.1 503 Unavailable\r\n\r\n");

        assert_eq!(
            check_browser_connection_port_at(address),
            Err(format!(
                "Browser registration port {address} returned an unexpected response."
            ))
        );
        server.join().unwrap();
    }
}
