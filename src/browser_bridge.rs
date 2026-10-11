use crate::browser_protocol::{BrowserCommand, BrowserMessage};
use directories::BaseDirs;
use serde::{de::DeserializeOwned, Serialize};
use std::fs;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub const NATIVE_HOST_NAME: &str = "com.twitchstreaksaver.bridge";
pub const BRIDGE_ADDRESS: &str = "127.0.0.1:43127";
const MAX_MESSAGE_SIZE: usize = 1_048_576;

pub enum BridgeEvent {
    Connected,
    Disconnected,
    Message(BrowserMessage),
    Failed(String),
}

pub struct BrowserBridge {
    commands: Sender<BrowserCommand>,
    events: Receiver<BridgeEvent>,
    stop: Arc<AtomicBool>,
    server: Option<JoinHandle<()>>,
}

impl BrowserBridge {
    pub fn start() -> Result<Self, String> {
        let listener = TcpListener::bind(BRIDGE_ADDRESS).map_err(|error| {
            format!("Could not start browser bridge on {BRIDGE_ADDRESS}: {error}")
        })?;
        listener
            .set_nonblocking(true)
            .map_err(|error| format!("Could not configure browser bridge: {error}"))?;
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let server_stop = Arc::clone(&stop);
        let server = thread::Builder::new()
            .name("browser-native-bridge".to_owned())
            .spawn(move || serve(listener, command_rx, event_tx, server_stop))
            .map_err(|error| format!("Could not start browser bridge: {error}"))?;

        Ok(Self {
            commands: command_tx,
            events: event_rx,
            stop,
            server: Some(server),
        })
    }

    pub fn send(&self, command: BrowserCommand) -> Result<(), String> {
        self.commands
            .send(command)
            .map_err(|_| "Browser bridge is not running.".to_owned())
    }

    pub fn try_recv(&self) -> Result<BridgeEvent, TryRecvError> {
        self.events.try_recv()
    }
}

impl Drop for BrowserBridge {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(server) = self.server.take() {
            let _ = server.join();
        }
    }
}

fn serve(
    listener: TcpListener,
    commands: Receiver<BrowserCommand>,
    events: Sender<BridgeEvent>,
    stop: Arc<AtomicBool>,
) {
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => handle_connection(stream, &commands, &events, &stop),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(error) => {
                let _ = events.send(BridgeEvent::Failed(format!(
                    "Browser bridge listener failed: {error}"
                )));
                break;
            }
        }
    }
}

fn handle_connection(
    mut stream: TcpStream,
    commands: &Receiver<BrowserCommand>,
    events: &Sender<BridgeEvent>,
    stop: &AtomicBool,
) {
    if let Err(error) = stream.set_nonblocking(false) {
        let _ = events.send(BridgeEvent::Failed(format!(
            "Could not configure browser bridge socket: {error}"
        )));
        return;
    }
    while commands.try_recv().is_ok() {}
    let reader_stream = match stream.try_clone() {
        Ok(stream) => stream,
        Err(error) => {
            let _ = events.send(BridgeEvent::Failed(format!(
                "Could not initialize browser bridge connection: {error}"
            )));
            return;
        }
    };
    let active = Arc::new(AtomicBool::new(true));
    let reader_active = Arc::clone(&active);
    let (incoming_tx, incoming_rx) = mpsc::channel();
    let reader_events = events.clone();
    let reader = thread::spawn(move || {
        let mut reader_stream = reader_stream;
        loop {
            match read_message::<BrowserMessage>(&mut reader_stream) {
                Ok(Some(message)) => {
                    if incoming_tx.send(message).is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    let _ = reader_events.send(BridgeEvent::Failed(format!(
                        "Browser bridge message failed: {error}"
                    )));
                    break;
                }
            }
        }
        reader_active.store(false, Ordering::Release);
    });
    let _ = events.send(BridgeEvent::Connected);

    while !stop.load(Ordering::Acquire) && active.load(Ordering::Acquire) {
        while let Ok(message) = incoming_rx.try_recv() {
            if events.send(BridgeEvent::Message(message)).is_err() {
                break;
            }
        }
        match commands.recv_timeout(Duration::from_millis(50)) {
            Ok(command) => {
                if let Err(error) = write_message(&mut stream, &command) {
                    let _ = events.send(BridgeEvent::Failed(format!(
                        "Could not send command to browser: {error}"
                    )));
                    break;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    let _ = stream.shutdown(Shutdown::Both);
    let _ = reader.join();
    let _ = events.send(BridgeEvent::Disconnected);
}

pub fn read_message<T: DeserializeOwned>(reader: &mut impl Read) -> io::Result<Option<T>> {
    let mut length_bytes = [0u8; 4];
    match reader.read(&mut length_bytes[..1])? {
        0 => return Ok(None),
        1 => {}
        _ => unreachable!(),
    }
    reader.read_exact(&mut length_bytes[1..])?;
    let length = u32::from_le_bytes(length_bytes) as usize;
    if length > MAX_MESSAGE_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Browser bridge message exceeded the size limit",
        ));
    }
    let mut payload = vec![0; length];
    reader.read_exact(&mut payload)?;
    serde_json::from_slice(&payload)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

pub fn write_message(writer: &mut impl Write, message: &impl Serialize) -> io::Result<()> {
    let payload = serde_json::to_vec(message)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if payload.len() > MAX_MESSAGE_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Browser bridge message exceeded the size limit",
        ));
    }
    let length = u32::try_from(payload.len())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    writer.write_all(&length.to_le_bytes())?;
    writer.write_all(&payload)?;
    writer.flush()
}

pub fn install_native_host(extension_id: &str) -> Result<String, String> {
    if !valid_extension_id(extension_id) {
        return Err("Extension ID must be 32 lowercase letters from a to p.".to_owned());
    }
    let host_path = native_host_path()?
        .canonicalize()
        .map_err(|error| format!("Could not resolve native host executable: {error}"))?;
    let base_dirs =
        BaseDirs::new().ok_or_else(|| "Could not locate user config directory.".to_owned())?;
    let manifest_paths = native_manifest_paths(&base_dirs)?;
    let manifest = NativeHostManifest {
        name: NATIVE_HOST_NAME,
        description: "Twitch Streak Saver browser bridge",
        path: host_path.to_string_lossy().into_owned(),
        host_type: "stdio",
        allowed_origins: vec![format!("chrome-extension://{extension_id}/")],
    };
    let encoded = serde_json::to_vec_pretty(&manifest)
        .map_err(|error| format!("Could not encode native host manifest: {error}"))?;
    for manifest_path in &manifest_paths {
        let manifest_dir = manifest_path
            .parent()
            .ok_or_else(|| "Native host manifest path is invalid.".to_owned())?;
        fs::create_dir_all(manifest_dir)
            .map_err(|error| format!("Could not create native host directory: {error}"))?;
        fs::write(manifest_path, &encoded)
            .map_err(|error| format!("Could not install native host manifest: {error}"))?;
    }

    #[cfg(target_os = "windows")]
    if let Some(manifest_path) = manifest_paths.first() {
        register_windows_hosts(manifest_path)?;
    }

    Ok(manifest_paths
        .iter()
        .map(|path| path.to_string_lossy())
        .collect::<Vec<_>>()
        .join(", "))
}

#[derive(Serialize)]
struct NativeHostManifest {
    name: &'static str,
    description: &'static str,
    path: String,
    #[serde(rename = "type")]
    host_type: &'static str,
    allowed_origins: Vec<String>,
}

fn valid_extension_id(extension_id: &str) -> bool {
    extension_id.len() == 32
        && extension_id
            .bytes()
            .all(|byte| (b'a'..=b'p').contains(&byte))
}

#[cfg(target_os = "macos")]
fn native_host_path() -> Result<std::path::PathBuf, String> {
    let app_path = std::env::current_exe()
        .map_err(|error| format!("Could not locate Twitch Streak Saver: {error}"))?;
    let executable_name = "twitch_streak_saver_host";
    app_path
        .parent()
        .map(|directory| directory.join(executable_name))
        .filter(|path| path.is_file())
        .ok_or_else(|| {
            format!(
                "Native host executable not found beside {}.",
                app_path.display()
            )
        })
}

#[cfg(target_os = "windows")]
fn native_host_path() -> Result<std::path::PathBuf, String> {
    let app_path = std::env::current_exe()
        .map_err(|error| format!("Could not locate Twitch Streak Saver: {error}"))?;
    let executable_name = "twitch_streak_saver_host.exe";
    app_path
        .parent()
        .map(|directory| directory.join(executable_name))
        .filter(|path| path.is_file())
        .ok_or_else(|| {
            format!(
                "Native host executable not found beside {}.",
                app_path.display()
            )
        })
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn native_host_path() -> Result<std::path::PathBuf, String> {
    Err("Browser native host installation supports macOS and Windows.".to_owned())
}

#[cfg(target_os = "macos")]
fn native_manifest_paths(base_dirs: &BaseDirs) -> Result<Vec<std::path::PathBuf>, String> {
    let file_name = format!("{NATIVE_HOST_NAME}.json");
    let config_dir = base_dirs.config_dir();
    Ok(vec![
        config_dir
            .join("Google/Chrome/NativeMessagingHosts")
            .join(&file_name),
        config_dir
            .join("BraveSoftware/Brave-Browser/NativeMessagingHosts")
            .join(file_name),
    ])
}

#[cfg(target_os = "windows")]
fn native_manifest_paths(base_dirs: &BaseDirs) -> Result<Vec<std::path::PathBuf>, String> {
    Ok(vec![base_dirs
        .data_local_dir()
        .join("TwitchStreakSaver")
        .join(format!("{NATIVE_HOST_NAME}.json"))])
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn native_manifest_paths(_: &BaseDirs) -> Result<Vec<std::path::PathBuf>, String> {
    Err("Browser native host installation supports macOS and Windows.".to_owned())
}

#[cfg(target_os = "windows")]
fn register_windows_hosts(manifest_path: &std::path::Path) -> Result<(), String> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let user_key = RegKey::predef(HKEY_CURRENT_USER);
    let registry_paths = [
        format!("Software\\Google\\Chrome\\NativeMessagingHosts\\{NATIVE_HOST_NAME}"),
        format!("Software\\BraveSoftware\\Brave-Browser\\NativeMessagingHosts\\{NATIVE_HOST_NAME}"),
    ];
    let manifest_value = manifest_path.to_string_lossy().into_owned();
    for registry_path in registry_paths {
        let (host_key, _) = user_key
            .create_subkey(registry_path)
            .map_err(|error| format!("Could not register browser native host: {error}"))?;
        host_key
            .set_value("", &manifest_value)
            .map_err(|error| format!("Could not register browser native host: {error}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{read_message, valid_extension_id, write_message};
    use serde_json::json;
    use std::io::Cursor;

    #[test]
    fn native_message_frames_round_trip_json() {
        let message = json!({"type": "channel_status", "playing": true});
        let mut bytes = Vec::new();
        write_message(&mut bytes, &message).unwrap();

        assert_eq!(
            read_message::<serde_json::Value>(&mut Cursor::new(bytes)).unwrap(),
            Some(message)
        );
    }

    #[test]
    fn extension_id_validation_rejects_untrusted_manifest_origins() {
        assert!(valid_extension_id("a".repeat(32).as_str()));
        assert!(!valid_extension_id("a".repeat(31).as_str()));
        assert!(!valid_extension_id("z".repeat(32).as_str()));
    }
}
