#[cfg(any(target_os = "windows", test))]
use std::path::Path;

#[cfg(any(target_os = "windows", test))]
fn run_command(executable: &Path) -> Result<String, String> {
    let executable = executable.to_str().ok_or_else(|| {
        "The app executable path is not valid Unicode and cannot be registered for Windows startup."
            .to_owned()
    })?;
    Ok(format!("\"{executable}\""))
}
#[cfg(target_os = "windows")]
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(target_os = "windows")]
const VALUE_NAME: &str = "TwitchStreakSaver";

#[cfg(target_os = "windows")]
pub(super) fn initialize() -> (bool, Option<String>) {
    match is_registered() {
        Ok(false) => (false, None),
        Ok(true) => match set_enabled(true) {
            Ok(()) => (true, None),
            Err(error) => (true, Some(error)),
        },
        Err(error) => (false, Some(error)),
    }
}

#[cfg(target_os = "windows")]
pub(super) fn set_enabled(enabled: bool) -> Result<(), String> {
    use std::io::ErrorKind;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_SET_VALUE};
    use winreg::RegKey;

    let user_key = RegKey::predef(HKEY_CURRENT_USER);
    if enabled {
        let executable = std::env::current_exe().map_err(|error| {
            format!("Could not locate the app executable for Windows startup: {error}")
        })?;
        let command = run_command(&executable)?;
        let (run_key, _) = user_key
            .create_subkey(RUN_KEY)
            .map_err(|error| format!("Could not open the Windows startup registry key: {error}"))?;
        run_key
            .set_value(VALUE_NAME, &command)
            .map_err(|error| format!("Could not enable Windows startup: {error}"))
    } else {
        let run_key = match user_key.open_subkey_with_flags(RUN_KEY, KEY_SET_VALUE) {
            Ok(run_key) => run_key,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(format!(
                    "Could not open the Windows startup registry key: {error}"
                ));
            }
        };
        match run_key.delete_value(VALUE_NAME) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("Could not disable Windows startup: {error}")),
        }
    }
}

#[cfg(target_os = "windows")]
fn is_registered() -> Result<bool, String> {
    use std::io::ErrorKind;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_QUERY_VALUE};
    use winreg::RegKey;

    let user_key = RegKey::predef(HKEY_CURRENT_USER);
    let run_key = match user_key.open_subkey_with_flags(RUN_KEY, KEY_QUERY_VALUE) {
        Ok(run_key) => run_key,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(format!(
                "Could not read the Windows startup registry key: {error}"
            ));
        }
    };
    match run_key.get_raw_value(VALUE_NAME) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("Could not read the Windows startup entry: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::run_command;
    use std::path::Path;

    #[test]
    fn windows_startup_command_quotes_executable_paths_with_spaces() {
        let executable = Path::new(
            r"C:\Users\Example User\AppData\Local\Twitch Streak Saver\twitch-streak-saver.exe",
        );

        assert_eq!(
            run_command(executable).unwrap(),
            r#""C:\Users\Example User\AppData\Local\Twitch Streak Saver\twitch-streak-saver.exe""#
        );
    }
    #[cfg(unix)]
    #[test]
    fn windows_startup_rejects_non_unicode_paths_instead_of_corrupting_them() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let executable = Path::new(OsStr::from_bytes(b"/apps/\xff.exe"));

        assert_eq!(
            run_command(executable),
            Err("The app executable path is not valid Unicode and cannot be registered for Windows startup.".to_owned())
        );
    }
}
