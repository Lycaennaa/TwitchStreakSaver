use std::fs::{self, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const LAUNCH_AGENT_LABEL: &str = "com.lycaennaa.twitchstreaksaver.login";
const LAUNCH_AGENT_FILE: &str = "com.lycaennaa.twitchstreaksaver.login.plist";
static NEXT_TEMP_FILE_ID: AtomicU64 = AtomicU64::new(0);

pub(super) fn initialize() -> (bool, Option<String>) {
    match launch_agent_path() {
        Ok(path) => (path.is_file(), None),
        Err(error) => (false, Some(error)),
    }
}

pub(super) fn set_enabled(enabled: bool) -> Result<(), String> {
    let path = launch_agent_path()?;
    if enabled {
        let executable = std::env::current_exe()
            .map_err(|error| format!("Could not locate the application executable: {error}"))?;
        let contents = launch_agent_contents(&executable)?;
        let parent = path
            .parent()
            .ok_or_else(|| "Could not determine the LaunchAgents directory".to_owned())?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("Could not create {}: {error}", parent.display()))?;
        write_launch_agent_atomically(&path, &contents)
    } else {
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("Could not remove {}: {error}", path.display())),
        }
    }
}

fn launch_agent_path() -> Result<PathBuf, String> {
    let base_dirs = directories::BaseDirs::new()
        .ok_or_else(|| "Could not locate the current user's home directory".to_owned())?;
    Ok(base_dirs
        .home_dir()
        .join("Library")
        .join("LaunchAgents")
        .join(LAUNCH_AGENT_FILE))
}

fn write_launch_agent_atomically(path: &Path, contents: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Could not determine the LaunchAgents directory".to_owned())?;
    let temporary_path = parent.join(format!(
        ".{LAUNCH_AGENT_FILE}.{}.{}.tmp",
        std::process::id(),
        NEXT_TEMP_FILE_ID.fetch_add(1, Ordering::Relaxed)
    ));
    write_temp_and_replace(path, &temporary_path, contents)
        .map_err(|error| format!("Could not update {}: {error}", path.display()))
}

fn write_temp_and_replace(
    destination: &Path,
    temporary_path: &Path,
    contents: &str,
) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temporary_path)?;
    let result = (|| {
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(temporary_path, destination)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary_path);
    }
    result
}

fn launch_agent_contents(executable: &Path) -> Result<String, String> {
    let executable = executable
        .to_str()
        .ok_or_else(|| "The application path is not valid Unicode".to_owned())?;
    let executable = xml_escape(executable);
    Ok(format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{LAUNCH_AGENT_LABEL}</string>
	<key>ProgramArguments</key>
	<array>
		<string>{executable}</string>
		<string>--background</string>
	</array>
	<key>RunAtLoad</key>
	<true/>
</dict>
</plist>
"#
    ))
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::{
        launch_agent_contents, write_launch_agent_atomically, write_temp_and_replace,
        NEXT_TEMP_FILE_ID,
    };
    use std::fs;
    use std::io::ErrorKind;
    use std::path::{Path, PathBuf};

    struct ProjectTempDirectory {
        path: PathBuf,
        root: PathBuf,
        remove_root_on_drop: bool,
    }

    impl ProjectTempDirectory {
        fn new() -> Self {
            let root = std::env::current_dir().unwrap().join(".tmp");
            let remove_root_on_drop = match fs::create_dir(&root) {
                Ok(()) => true,
                Err(error) if error.kind() == ErrorKind::AlreadyExists => false,
                Err(error) => panic!("Could not create project-local temp root: {error}"),
            };
            loop {
                let id = NEXT_TEMP_FILE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let path = root.join(format!("launch-agent-{}-{id}", std::process::id()));
                match fs::create_dir(&path) {
                    Ok(()) => {
                        return Self {
                            path,
                            root,
                            remove_root_on_drop,
                        };
                    }
                    Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                    Err(error) => {
                        if remove_root_on_drop {
                            let _ = fs::remove_dir(&root);
                        }
                        panic!("Could not create project-local test directory: {error}");
                    }
                }
            }
        }
    }

    impl Drop for ProjectTempDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
            if self.remove_root_on_drop {
                let _ = fs::remove_dir(&self.root);
            }
        }
    }

    #[test]
    fn login_agent_starts_the_app_hidden_and_escapes_its_path() {
        let contents = launch_agent_contents(Path::new(
            "/Applications/Twitch & Saver.app/Contents/MacOS/twitch-streak-saver",
        ))
        .unwrap();

        assert!(contents.contains(
            "<string>/Applications/Twitch &amp; Saver.app/Contents/MacOS/twitch-streak-saver</string>"
        ));
        assert!(contents.contains("<string>--background</string>"));
        assert!(contents.contains("<key>RunAtLoad</key>\n\t<true/>"));
    }

    #[test]
    fn launch_agent_write_keeps_the_old_file_until_the_temp_write_succeeds() {
        let directory = ProjectTempDirectory::new();
        let destination = directory.path.join("login.plist");
        fs::write(&destination, "old plist").unwrap();

        write_launch_agent_atomically(&destination, "complete plist").unwrap();
        assert_eq!(fs::read_to_string(&destination).unwrap(), "complete plist");

        let occupied_temp = directory.path.join("occupied.tmp");
        fs::create_dir(&occupied_temp).unwrap();
        let error =
            write_temp_and_replace(&destination, &occupied_temp, "partial plist").unwrap_err();
        assert_eq!(error.kind(), ErrorKind::AlreadyExists);
        assert_eq!(fs::read_to_string(&destination).unwrap(), "complete plist");
    }
}
