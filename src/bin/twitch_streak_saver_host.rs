use std::io;
use std::net::{Shutdown, TcpStream};
use std::thread;
use twitch_streak_saver::browser_bridge::{read_message, write_message};

const BRIDGE_ADDRESS: &str = "127.0.0.1:43127";

fn main() {
    if let Err(error) = run() {
        eprintln!("Twitch Streak Saver native host: {error}");
        std::process::exit(1);
    }
}

fn run() -> io::Result<()> {
    let app_stream = TcpStream::connect(BRIDGE_ADDRESS)?;
    app_stream.set_nodelay(true)?;
    let mut app_reader = app_stream.try_clone()?;
    let native_to_app = app_stream.try_clone()?;
    let native_to_app_thread = native_to_app.try_clone()?;
    let forwarder = thread::spawn(move || {
        let stdin = io::stdin();
        let mut input = stdin.lock();
        let mut app_writer = native_to_app_thread;
        loop {
            match read_message(&mut input) {
                Ok(Some(message)) => {
                    if let Err(error) = write_message(&mut app_writer, &message) {
                        eprintln!("Could not forward Chrome message: {error}");
                        break;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    eprintln!("Could not read Chrome message: {error}");
                    break;
                }
            }
        }
        let _ = app_writer.shutdown(Shutdown::Both);
    });

    let stdout = io::stdout();
    let mut output = stdout.lock();
    let result = loop {
        match read_message(&mut app_reader)? {
            Some(message) => write_message(&mut output, &message)?,
            None => break Ok(()),
        }
    };
    let _ = native_to_app.shutdown(Shutdown::Both);
    drop(forwarder);
    result
}
