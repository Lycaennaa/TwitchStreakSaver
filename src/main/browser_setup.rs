use super::Dashboard;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SetupBrowser {
    Chrome,
    Brave,
    Edge,
    Helium,
}

impl SetupBrowser {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Chrome => "Chrome",
            Self::Brave => "Brave",
            Self::Edge => "Microsoft Edge",
            Self::Helium => "Helium",
        }
    }

    fn url(self) -> &'static str {
        match self {
            Self::Chrome => "chrome://extensions/",
            Self::Brave => "brave://extensions/",
            Self::Edge => "edge://extensions/",
            Self::Helium => "helium://extensions/",
        }
    }
}

pub(super) fn detected_setup_browsers() -> Vec<SetupBrowser> {
    [
        SetupBrowser::Chrome,
        SetupBrowser::Brave,
        SetupBrowser::Edge,
        SetupBrowser::Helium,
    ]
    .into_iter()
    .filter(|browser| browser_application_path(*browser).is_some())
    .collect()
}

#[cfg(target_os = "macos")]
fn browser_application_path(browser: SetupBrowser) -> Option<std::path::PathBuf> {
    let application_name = match browser {
        SetupBrowser::Chrome => "Google Chrome.app",
        SetupBrowser::Brave => "Brave Browser.app",
        SetupBrowser::Edge => "Microsoft Edge.app",
        SetupBrowser::Helium => "Helium.app",
    };
    let mut application_directories = vec![std::path::PathBuf::from("/Applications")];
    if let Some(base_dirs) = directories::BaseDirs::new() {
        application_directories.push(base_dirs.home_dir().join("Applications"));
    }
    application_directories
        .into_iter()
        .map(|directory| directory.join(application_name))
        .find(|path| path.is_dir())
}

#[cfg(target_os = "windows")]
fn browser_application_path(browser: SetupBrowser) -> Option<std::path::PathBuf> {
    let relative_path = match browser {
        SetupBrowser::Chrome => "Google/Chrome/Application/chrome.exe",
        SetupBrowser::Brave => "BraveSoftware/Brave-Browser/Application/brave.exe",
        SetupBrowser::Edge => "Microsoft/Edge/Application/msedge.exe",
        SetupBrowser::Helium => "imput/Helium/Application/chrome.exe",
    };
    [
        "LOCALAPPDATA",
        "PROGRAMFILES",
        "PROGRAMFILES(X86)",
        "ProgramW6432",
    ]
    .into_iter()
    .filter_map(std::env::var_os)
    .map(std::path::PathBuf::from)
    .map(|directory| directory.join(relative_path))
    .find(|path| path.is_file())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn browser_application_path(_: SetupBrowser) -> Option<std::path::PathBuf> {
    None
}

impl Dashboard {
    pub(super) fn open_extension_setup(&mut self, target: SetupBrowser) {
        match open_browser_extension_setup(target) {
            Ok(()) => {
                self.browser_error = None;
                self.browser_notice = Some(format!(
                    "Enable Developer mode in {} and choose Load unpacked; select the revealed folder.",
                    target.name()
                ));
            }
            Err(error) => self.browser_error = Some(error),
        }
    }
}

fn open_browser_extension_setup(target: SetupBrowser) -> Result<(), String> {
    let extension_directory = browser_extension_directory()?;
    reveal_extension_directory(&extension_directory)?;
    launch_browser_extensions(target).map_err(|error| {
        format!(
            "The extension folder is open, but {} extensions could not be opened: {error}",
            target.name()
        )
    })
}
#[cfg(target_os = "macos")]
fn launch_browser_extensions(target: SetupBrowser) -> Result<(), String> {
    let application = browser_application_path(target)
        .ok_or_else(|| format!("{} is no longer available.", target.name()))?;
    let mut command = std::process::Command::new("open");
    if target == SetupBrowser::Helium {
        command.arg("-a").arg(application).arg(target.url());
    } else {
        command
            .arg("-n")
            .arg("-a")
            .arg(application)
            .arg("--args")
            .arg("--new-window")
            .arg(target.url());
    }
    let status = command
        .status()
        .map_err(|error| format!("Could not launch {}: {error}", target.name()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "Could not launch {}. Check that it is installed.",
            target.name()
        ))
    }
}

#[cfg(target_os = "windows")]
fn launch_browser_extensions(target: SetupBrowser) -> Result<(), String> {
    let executable = browser_application_path(target)
        .ok_or_else(|| format!("{} is no longer available.", target.name()))?;
    std::process::Command::new(executable)
        .arg("--new-window")
        .arg(target.url())
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Could not launch {}: {error}", target.name()))
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn launch_browser_extensions(_: SetupBrowser) -> Result<(), String> {
    Err("Opening browser extension settings is supported on macOS and Windows.".to_owned())
}

fn browser_extension_directory() -> Result<std::path::PathBuf, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("Could not locate Twitch Streak Saver: {error}"))?;
    let executable_directory = executable
        .parent()
        .ok_or_else(|| "Could not locate the desktop app folder.".to_owned())?;

    #[cfg(target_os = "macos")]
    if let Some(contents_directory) = executable_directory.parent() {
        let bundled_directory = contents_directory.join("Resources/chrome-extension");
        if bundled_directory.join("manifest.json").is_file() {
            return Ok(bundled_directory);
        }
    }

    let adjacent_directory = executable_directory.join("chrome-extension");
    if adjacent_directory.join("manifest.json").is_file() {
        return Ok(adjacent_directory);
    }

    #[cfg(debug_assertions)]
    {
        let source_directory =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("chrome-extension");
        if source_directory.join("manifest.json").is_file() {
            return Ok(source_directory);
        }
    }

    Err(format!(
        "Could not find the shipped chrome-extension folder beside {}.",
        executable.display()
    ))
}

#[cfg(target_os = "macos")]
fn reveal_extension_directory(directory: &std::path::Path) -> Result<(), String> {
    std::process::Command::new("open")
        .arg(directory)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Could not show the extension folder: {error}"))
}

#[cfg(target_os = "windows")]
fn reveal_extension_directory(directory: &std::path::Path) -> Result<(), String> {
    std::process::Command::new("explorer.exe")
        .arg(directory)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Could not show the extension folder: {error}"))
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn reveal_extension_directory(_: &std::path::Path) -> Result<(), String> {
    Err("Opening the extension folder is supported on macOS and Windows.".to_owned())
}

#[cfg(all(test, debug_assertions))]
mod extension_setup_tests {
    use super::browser_extension_directory;

    #[test]
    fn resolves_a_complete_extension_folder_for_development() {
        let directory = browser_extension_directory().unwrap();
        assert!(directory.join("manifest.json").is_file());
        assert!(directory.join("background.js").is_file());
        assert!(directory.join("trusted-extension-id").is_file());
    }
}
