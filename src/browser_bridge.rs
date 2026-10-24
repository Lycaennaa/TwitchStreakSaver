use crate::browser_protocol::{BrowserCommand, BrowserMessage, KnownBrowserMessage};
use directories::BaseDirs;
use serde::{de::DeserializeOwned, Serialize};
use std::fs;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
const BRIDGE_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
// Bound the complete socket write because recoverable sends hold the generation lock.
const BRIDGE_COMMAND_WRITE_TIMEOUT: Duration = Duration::from_millis(250);
mod registration;

pub const NATIVE_HOST_NAME: &str = "com.twitchstreaksaver.bridge";
pub const BRIDGE_ADDRESS: &str = "127.0.0.1:43127";
const MAX_MESSAGE_SIZE: usize = 1_048_576;
pub const EXTENSION_REGISTRATION_ADDRESS: &str = "127.0.0.1:43128";
const TRUSTED_EXTENSION_ID: &str = include_str!("../chrome-extension/trusted-extension-id");

pub enum BridgeEvent {
    Connected,
    Disconnected,
    Message(BrowserMessage),
    Failed(String),
    CommandFailed {
        command: BrowserCommand,
        recovery_generation: Option<u64>,
        message: String,
    },
}

struct QueuedBrowserCommand {
    command: BrowserCommand,
    recovery_generation: Option<u64>,
}

pub struct BrowserBridge {
    commands: Sender<QueuedBrowserCommand>,
    events: Receiver<BridgeEvent>,
    stop: Arc<AtomicBool>,
    recovery_generation: Arc<Mutex<u64>>,
    server: Option<JoinHandle<()>>,
    registration_server: Option<JoinHandle<()>>,
}

impl BrowserBridge {
    pub fn start() -> Result<Self, String> {
        let listener = TcpListener::bind(BRIDGE_ADDRESS).map_err(|error| {
            format!("Could not start browser bridge on {BRIDGE_ADDRESS}: {error}")
        })?;
        listener
            .set_nonblocking(true)
            .map_err(|error| format!("Could not configure browser bridge: {error}"))?;
        let registration_listener =
            TcpListener::bind(EXTENSION_REGISTRATION_ADDRESS).map_err(|error| {
                format!(
                    "Could not start extension registration on {EXTENSION_REGISTRATION_ADDRESS}: {error}"
                )
            })?;
        registration_listener
            .set_nonblocking(true)
            .map_err(|error| format!("Could not configure extension registration: {error}"))?;
        install_native_host()?;

        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let server_stop = Arc::clone(&stop);
        let server_events = event_tx.clone();
        let recovery_generation = Arc::new(Mutex::new(0));
        let server_recovery_generation = Arc::clone(&recovery_generation);
        let server = thread::Builder::new()
            .name("browser-native-bridge".to_owned())
            .spawn(move || {
                serve(
                    listener,
                    command_rx,
                    server_events,
                    server_stop,
                    server_recovery_generation,
                )
            })
            .map_err(|error| format!("Could not start browser bridge: {error}"))?;

        let registration_stop = Arc::clone(&stop);
        let registration_server = match thread::Builder::new()
            .name("browser-extension-registration".to_owned())
            .spawn(move || {
                registration::serve_extension_registrations(
                    registration_listener,
                    event_tx,
                    registration_stop,
                )
            }) {
            Ok(server) => server,
            Err(error) => {
                stop.store(true, Ordering::Release);
                let _ = server.join();
                return Err(format!(
                    "Could not start extension registration server: {error}"
                ));
            }
        };

        Ok(Self {
            commands: command_tx,
            events: event_rx,
            stop,
            recovery_generation,
            server: Some(server),
            registration_server: Some(registration_server),
        })
    }
    pub fn send(&self, command: BrowserCommand) -> Result<(), String> {
        self.enqueue(command, None)
    }

    pub fn send_recoverable(
        &self,
        command: BrowserCommand,
        recovery_generation: u64,
    ) -> Result<(), String> {
        self.enqueue(command, Some(recovery_generation))
    }

    pub fn set_recovery_generation(&self, generation: u64) {
        update_recovery_generation(&self.recovery_generation, generation);
    }
    fn enqueue(
        &self,
        command: BrowserCommand,
        recovery_generation: Option<u64>,
    ) -> Result<(), String> {
        self.commands
            .send(QueuedBrowserCommand {
                command,
                recovery_generation,
            })
            .map_err(|_| "Browser bridge is not running.".to_owned())
    }

    pub fn try_recv(&self) -> Result<BridgeEvent, TryRecvError> {
        self.events.try_recv()
    }
}
fn update_recovery_generation(recovery_generation: &Mutex<u64>, generation: u64) {
    *recovery_generation
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = generation;
}

impl Drop for BrowserBridge {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(server) = self.registration_server.take() {
            let _ = server.join();
        }
        if let Some(server) = self.server.take() {
            let _ = server.join();
        }
    }
}

fn serve(
    listener: TcpListener,
    commands: Receiver<QueuedBrowserCommand>,
    events: Sender<BridgeEvent>,
    stop: Arc<AtomicBool>,
    recovery_generation: Arc<Mutex<u64>>,
) {
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => {
                handle_connection(stream, &commands, &events, &stop, &recovery_generation)
            }
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
fn send_command(
    queued_command: QueuedBrowserCommand,
    current_recovery_generation: &Mutex<u64>,
    events: &mpsc::Sender<BridgeEvent>,
    write: impl FnOnce(&BrowserCommand) -> std::io::Result<()>,
) -> bool {
    let QueuedBrowserCommand {
        command,
        recovery_generation,
    } = queued_command;
    let generation_guard = recovery_generation.map(|saved_generation| {
        let current_generation = current_recovery_generation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (saved_generation, current_generation)
    });
    if generation_guard
        .as_ref()
        .is_some_and(|(saved_generation, current_generation)| {
            *saved_generation != **current_generation
        })
    {
        return true;
    }
    if let Err(error) = write(&command) {
        let _ = events.send(BridgeEvent::CommandFailed {
            command,
            recovery_generation,
            message: format!("Could not send command to browser: {error}"),
        });
        false
    } else {
        true
    }
}

fn handle_connection(
    mut stream: TcpStream,
    commands: &Receiver<QueuedBrowserCommand>,
    events: &Sender<BridgeEvent>,
    stop: &AtomicBool,
    recovery_generation: &Mutex<u64>,
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
    let ready = match incoming_rx.recv_timeout(BRIDGE_HANDSHAKE_TIMEOUT) {
        Ok(message @ BrowserMessage::Known(KnownBrowserMessage::Ready { .. })) => {
            let _ = events.send(BridgeEvent::Connected);
            let _ = events.send(BridgeEvent::Message(message));
            true
        }
        Ok(_) => {
            let _ = events.send(BridgeEvent::Failed(
                "Browser bridge did not start with a ready message.".to_owned(),
            ));
            false
        }
        Err(RecvTimeoutError::Timeout) => {
            let _ = events.send(BridgeEvent::Failed(
                "Browser bridge did not complete its handshake.".to_owned(),
            ));
            false
        }
        Err(RecvTimeoutError::Disconnected) => false,
    };
    if !ready {
        let _ = stream.shutdown(Shutdown::Both);
        let _ = reader.join();
        let _ = events.send(BridgeEvent::Disconnected);
        return;
    }

    while !stop.load(Ordering::Acquire) && active.load(Ordering::Acquire) {
        while let Ok(message) = incoming_rx.try_recv() {
            if events.send(BridgeEvent::Message(message)).is_err() {
                break;
            }
        }
        match commands.recv_timeout(Duration::from_millis(50)) {
            Ok(command) => {
                if !send_command(command, recovery_generation, events, |command| {
                    write_command_with_timeout(&mut stream, command)
                }) {
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
struct CommandWriteDeadline<'a> {
    stream: &'a mut TcpStream,
    deadline: Instant,
}

impl CommandWriteDeadline<'_> {
    fn set_stream_write_timeout(&self) -> io::Result<()> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Browser command write timed out",
            ));
        }
        self.stream.set_write_timeout(Some(remaining))
    }
}

impl Write for CommandWriteDeadline<'_> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.set_stream_write_timeout()?;
        self.stream.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.set_stream_write_timeout()?;
        self.stream.flush()
    }
}

fn write_command_with_timeout(stream: &mut TcpStream, command: &BrowserCommand) -> io::Result<()> {
    let mut writer = CommandWriteDeadline {
        stream,
        deadline: Instant::now() + BRIDGE_COMMAND_WRITE_TIMEOUT,
    };
    write_message(&mut writer, command)
}

fn install_native_host() -> Result<(), String> {
    let host_path = native_host_path()?
        .canonicalize()
        .map_err(|error| format!("Could not resolve native host executable: {error}"))?;
    let base_dirs =
        BaseDirs::new().ok_or_else(|| "Could not locate user config directory.".to_owned())?;
    let manifest_paths = native_manifest_paths(&base_dirs)?;
    let manifest = native_host_manifest(&host_path);
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

    Ok(())
}

fn native_host_manifest(host_path: &Path) -> NativeHostManifest {
    NativeHostManifest {
        name: NATIVE_HOST_NAME,
        description: "Twitch Streak Saver browser bridge",
        path: host_path.to_string_lossy().into_owned(),
        host_type: "stdio",
        allowed_origins: vec![format!("chrome-extension://{TRUSTED_EXTENSION_ID}/")],
    }
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
            .join(&file_name),
        config_dir
            .join("Microsoft Edge/NativeMessagingHosts")
            .join(&file_name),
        config_dir
            .join("net.imput.helium/NativeMessagingHosts")
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

#[cfg(any(target_os = "windows", test))]
fn windows_native_host_registry_paths() -> [String; 3] {
    [
        format!("Software\\Google\\Chrome\\NativeMessagingHosts\\{NATIVE_HOST_NAME}"),
        format!("Software\\BraveSoftware\\Brave-Browser\\NativeMessagingHosts\\{NATIVE_HOST_NAME}"),
        format!("Software\\Microsoft\\Edge\\NativeMessagingHosts\\{NATIVE_HOST_NAME}"),
    ]
}

#[cfg(target_os = "windows")]
fn register_windows_hosts(manifest_path: &std::path::Path) -> Result<(), String> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let user_key = RegKey::predef(HKEY_CURRENT_USER);
    let registry_paths = windows_native_host_registry_paths();
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
    use super::{
        native_host_manifest, native_manifest_paths, read_message, send_command,
        update_recovery_generation, windows_native_host_registry_paths, write_message, BridgeEvent,
        BrowserCommand, QueuedBrowserCommand, NATIVE_HOST_NAME, TRUSTED_EXTENSION_ID,
    };
    use serde_json::{json, Value};
    use std::io::Cursor;
    use std::path::Path;
    use std::sync::mpsc;
    use std::sync::Mutex;

    #[test]
    fn native_message_frames_round_trip_json() {
        let message = json!({"type": "channel_status", "playing": true});
        let mut bytes = Vec::new();
        write_message(&mut bytes, &message).unwrap();

        assert_eq!(
            read_message::<Value>(&mut Cursor::new(bytes)).unwrap(),
            Some(message)
        );
    }

    #[test]
    fn bridge_rejects_connections_without_the_extension_ready_message() {
        use super::{handle_connection, BridgeEvent};
        use std::net::{TcpListener, TcpStream};
        use std::sync::atomic::AtomicBool;
        use std::sync::mpsc;
        use std::sync::Arc;
        use std::thread;
        use std::time::Duration;

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server_stream, _) = listener.accept().unwrap();
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            handle_connection(
                server_stream,
                &command_rx,
                &event_tx,
                &Arc::new(AtomicBool::new(false)),
                &Mutex::new(0),
            )
        });

        write_message(&mut client, &json!({"type": "unexpected"})).unwrap();
        assert!(matches!(
            event_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            BridgeEvent::Failed(_)
        ));
        assert!(matches!(
            event_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            BridgeEvent::Disconnected
        ));
        drop(command_tx);
        server.join().unwrap();
    }

    #[test]
    fn bridge_reports_connected_after_the_extension_ready_message() {
        use super::{handle_connection, BridgeEvent};
        use std::net::{TcpListener, TcpStream};
        use std::sync::atomic::AtomicBool;
        use std::sync::mpsc;
        use std::sync::Arc;
        use std::thread;
        use std::time::Duration;

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server_stream, _) = listener.accept().unwrap();
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            handle_connection(
                server_stream,
                &command_rx,
                &event_tx,
                &Arc::new(AtomicBool::new(false)),
                &Mutex::new(0),
            )
        });

        write_message(&mut client, &json!({"type": "ready"})).unwrap();
        assert!(matches!(
            event_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            BridgeEvent::Connected
        ));
        let ready = event_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(matches!(
            ready,
            BridgeEvent::Message(crate::browser_protocol::BrowserMessage::Known(
                crate::browser_protocol::KnownBrowserMessage::Ready {
                    command_acknowledgments: false,
                }
            ))
        ));
        drop(client);
        assert!(matches!(
            event_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            BridgeEvent::Disconnected
        ));
        drop(command_tx);
        server.join().unwrap();
    }

    #[test]
    fn native_host_manifest_authorizes_only_the_bundled_extension() {
        let manifest = native_host_manifest(Path::new("native-host"));
        assert_eq!(
            manifest.allowed_origins,
            vec![format!("chrome-extension://{TRUSTED_EXTENSION_ID}/")]
        );
    }
    #[test]
    fn windows_native_host_registration_covers_edge_and_chromium_browsers() {
        assert_eq!(
            windows_native_host_registry_paths(),
            [
                format!("Software\\Google\\Chrome\\NativeMessagingHosts\\{NATIVE_HOST_NAME}"),
                format!(
                    "Software\\BraveSoftware\\Brave-Browser\\NativeMessagingHosts\\{NATIVE_HOST_NAME}"
                ),
                format!("Software\\Microsoft\\Edge\\NativeMessagingHosts\\{NATIVE_HOST_NAME}"),
            ]
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn mac_native_host_manifests_cover_edge_and_helium() {
        let base_dirs = directories::BaseDirs::new().unwrap();
        let config_dir = base_dirs.config_dir();
        let file_name = format!("{NATIVE_HOST_NAME}.json");
        assert_eq!(
            native_manifest_paths(&base_dirs).unwrap(),
            vec![
                config_dir
                    .join("Google/Chrome/NativeMessagingHosts")
                    .join(&file_name),
                config_dir
                    .join("BraveSoftware/Brave-Browser/NativeMessagingHosts")
                    .join(&file_name),
                config_dir
                    .join("Microsoft Edge/NativeMessagingHosts")
                    .join(&file_name),
                config_dir
                    .join("net.imput.helium/NativeMessagingHosts")
                    .join(file_name),
            ]
        );
    }
    #[test]
    fn failed_bridge_command_write_reports_recoverable_command() {
        let (events, received) = mpsc::channel();
        assert!(!send_command(
            QueuedBrowserCommand {
                command: BrowserCommand::ReloadManagedPlayerTabs {
                    command_id: "session-9".to_owned(),
                },
                recovery_generation: Some(9),
            },
            &Mutex::new(9),
            &events,
            |_| Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "socket closed",
            )),
        ));
        assert!(matches!(
            received.recv().unwrap(),
            BridgeEvent::CommandFailed {
                command: BrowserCommand::ReloadManagedPlayerTabs {
                    command_id,
                },
                recovery_generation: Some(9),
                message,
            } if command_id == "session-9"
                && message.contains("Could not send command to browser")
        ));
    }
    #[test]
    fn queued_command_is_discarded_when_recovery_generation_advances() {
        let (events, received) = mpsc::channel();
        let current_generation = Mutex::new(9);
        let queued_command = QueuedBrowserCommand {
            command: BrowserCommand::ReloadManagedPlayerTabs {
                command_id: "stale-session".to_owned(),
            },
            recovery_generation: Some(9),
        };

        assert!(send_command(
            QueuedBrowserCommand {
                command: BrowserCommand::ReloadManagedPlayerTabs {
                    command_id: "session-9".to_owned(),
                },
                recovery_generation: Some(9),
            },
            &current_generation,
            &events,
            |_| {
                assert!(current_generation.try_lock().is_err());
                Ok(())
            },
        ));
        update_recovery_generation(&current_generation, 10);

        let mut wrote_stale_command = false;
        assert!(send_command(
            queued_command,
            &current_generation,
            &events,
            |_| {
                wrote_stale_command = true;
                Ok(())
            },
        ));

        assert!(!wrote_stale_command);
        assert!(received.try_recv().is_err());
    }
}
