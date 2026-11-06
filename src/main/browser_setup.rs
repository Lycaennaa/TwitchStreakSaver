#[cfg(any(target_os = "macos", target_os = "windows"))]
use super::firefox_profile_lock::is_running as is_firefox_profile_running;
use super::{
    BrowserKind, BrowserLaunchTarget, Dashboard, FirefoxProfileLockState, BROWSER_EXTENSION_ID,
    FIREFOX_EXTENSION_ID,
};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
fn browser_extensions_url(browser: BrowserKind) -> &'static str {
    match browser {
        BrowserKind::Chrome => "chrome://extensions/",
        BrowserKind::Brave => "brave://extensions/",
        BrowserKind::Edge => "edge://extensions/",
        BrowserKind::Helium => "helium://extensions/",
        BrowserKind::Firefox => "about:debugging#/runtime/this-firefox",
    }
}

pub(super) fn detected_setup_browsers() -> Vec<BrowserKind> {
    [
        BrowserKind::Chrome,
        BrowserKind::Brave,
        BrowserKind::Edge,
        BrowserKind::Helium,
        BrowserKind::Firefox,
    ]
    .into_iter()
    .filter(|browser| browser_application_path(*browser).is_some())
    .collect()
}

#[cfg(target_os = "macos")]
fn browser_application_path(browser: BrowserKind) -> Option<std::path::PathBuf> {
    let application_name = match browser {
        BrowserKind::Chrome => "Google Chrome.app",
        BrowserKind::Brave => "Brave Browser.app",
        BrowserKind::Edge => "Microsoft Edge.app",
        BrowserKind::Helium => "Helium.app",
        BrowserKind::Firefox => "Firefox.app",
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
fn browser_application_path(browser: BrowserKind) -> Option<std::path::PathBuf> {
    let relative_path = match browser {
        BrowserKind::Chrome => "Google/Chrome/Application/chrome.exe",
        BrowserKind::Brave => "BraveSoftware/Brave-Browser/Application/brave.exe",
        BrowserKind::Edge => "Microsoft/Edge/Application/msedge.exe",
        BrowserKind::Helium => "imput/Helium/Application/chrome.exe",
        BrowserKind::Firefox => "Mozilla Firefox/firefox.exe",
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
fn browser_application_path(_: BrowserKind) -> Option<std::path::PathBuf> {
    None
}

#[derive(Clone, Debug)]
pub(super) struct DetectedBrowserProfile {
    pub(super) target: BrowserLaunchTarget,
    pub(super) label: String,
    pub(super) extension_enabled: bool,
    pub(super) extension_scan_error: Option<String>,
    pub(super) profile_root: PathBuf,
}

#[derive(Clone, Debug)]
pub(super) struct DetectedBrowserProfileRoot {
    pub(super) browser: BrowserKind,
    pub(super) path: PathBuf,
    pub(super) scan_error: Option<String>,
    pub(super) overridden: bool,
}

fn selected_browser_profile_data_directory(
    browser: BrowserKind,
    overrides: &[(BrowserKind, String)],
    standard_root: Option<PathBuf>,
) -> Option<(PathBuf, bool)> {
    if let Some((_, path)) = overrides
        .iter()
        .find(|(override_browser, _)| *override_browser == browser)
    {
        return Some((PathBuf::from(path), true));
    }
    standard_root.map(|path| (path, false))
}

pub(super) fn detected_browser_profile_roots(
    overrides: &[(BrowserKind, String)],
) -> Vec<DetectedBrowserProfileRoot> {
    detected_setup_browsers()
        .into_iter()
        .filter_map(|browser| {
            let (path, overridden) = selected_browser_profile_data_directory(
                browser,
                overrides,
                browser_profile_root(browser),
            )?;
            let scan_error = fs::read_dir(&path).err().map(|error| error.to_string());
            Some(DetectedBrowserProfileRoot {
                browser,
                path,
                scan_error,
                overridden,
            })
        })
        .collect()
}

pub(super) fn detected_browser_profiles_for_roots(
    roots: &mut [DetectedBrowserProfileRoot],
) -> Vec<DetectedBrowserProfile> {
    let mut profiles = Vec::new();
    for root in roots {
        if root.scan_error.is_some() {
            continue;
        }
        match detected_browser_profiles_for(root.browser, &root.path) {
            Ok(mut root_profiles) => profiles.append(&mut root_profiles),
            Err(error) => root.scan_error = Some(error),
        }
    }
    profiles.sort_by(|left, right| {
        right
            .extension_enabled
            .cmp(&left.extension_enabled)
            .then_with(|| left.label.cmp(&right.label))
    });
    profiles
}

fn detected_browser_profiles_for(
    browser: BrowserKind,
    root: &Path,
) -> Result<Vec<DetectedBrowserProfile>, String> {
    if browser == BrowserKind::Firefox {
        return detected_firefox_profiles(root);
    }
    let mut browser_profiles = Vec::new();
    let mut known_directories = HashSet::new();

    if let Ok(local_state) = fs::read_to_string(root.join("Local State")) {
        if let Ok(local_state) = serde_json::from_str::<serde_json::Value>(&local_state) {
            if let Some(info_cache) = local_state
                .get("profile")
                .and_then(|profile| profile.get("info_cache"))
                .and_then(serde_json::Value::as_object)
            {
                for (directory, profile) in info_cache {
                    if !is_valid_profile_directory(directory) || !root.join(directory).is_dir() {
                        continue;
                    }
                    let name = profile
                        .get("name")
                        .and_then(serde_json::Value::as_str)
                        .filter(|name| !name.is_empty())
                        .unwrap_or(directory);
                    known_directories.insert(directory.clone());
                    browser_profiles.push(detected_profile(browser, root, directory, name));
                }
            }
        }
    }

    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            if !entry.file_type().is_ok_and(|file_type| file_type.is_dir()) {
                continue;
            }
            let Some(directory) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if !is_valid_profile_directory(&directory)
                || !entry.path().join("Preferences").is_file()
                || !known_directories.insert(directory.clone())
            {
                continue;
            }
            browser_profiles.push(detected_profile(browser, root, &directory, &directory));
        }
    }

    Ok(browser_profiles)
}
fn detected_firefox_profiles(root: &Path) -> Result<Vec<DetectedBrowserProfile>, String> {
    let profiles_ini_path = root.join("profiles.ini");
    let profiles_ini = match fs::read_to_string(&profiles_ini_path) {
        Ok(profiles_ini) => profiles_ini,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!(
                "Could not read Firefox profiles.ini at {}: {error}",
                profiles_ini_path.display()
            ));
        }
    };
    let mut profiles = Vec::new();
    let mut is_profile_section = false;
    let mut name = None;
    let mut path = None;
    let mut is_relative = true;

    for line in profiles_ini.lines().chain(std::iter::once("[End]")) {
        let line = line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            if is_profile_section {
                append_firefox_profile(&mut profiles, root, name.take(), path.take(), is_relative);
            }
            is_profile_section = line[1..line.len() - 1].starts_with("Profile");
            name = None;
            path = None;
            is_relative = true;
            continue;
        }
        if !is_profile_section {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "Name" => name = Some(value.trim().to_owned()),
            "Path" => path = Some(value.trim().to_owned()),
            "IsRelative" => is_relative = value.trim() != "0",
            _ => {}
        }
    }

    Ok(profiles)
}

fn append_firefox_profile(
    profiles: &mut Vec<DetectedBrowserProfile>,
    root: &Path,
    name: Option<String>,
    path: Option<String>,
    is_relative: bool,
) {
    let Some(path) = path else {
        return;
    };
    let Some(profile_directory) = normalize_firefox_profile_directory(&path, is_relative) else {
        return;
    };
    let profile_path = if Path::new(&profile_directory).is_absolute() {
        PathBuf::from(&profile_directory)
    } else {
        root.join(&profile_directory)
    };
    if !profile_path.is_dir() {
        return;
    }
    let profile_name = name
        .filter(|name| !name.is_empty())
        .or_else(|| {
            Path::new(&profile_directory)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "Firefox profile".to_owned());
    profiles.push(detected_profile(
        BrowserKind::Firefox,
        root,
        &profile_directory,
        &profile_name,
    ));
}

fn normalize_firefox_profile_directory(path: &str, is_relative: bool) -> Option<String> {
    let path = Path::new(path.trim());
    if path.as_os_str().is_empty()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::CurDir | std::path::Component::ParentDir
            )
        })
    {
        return None;
    }
    if is_relative {
        if path.is_absolute() {
            return None;
        }
        let mut components = Vec::new();
        for component in path.components() {
            let std::path::Component::Normal(component) = component else {
                return None;
            };
            components.push(component.to_string_lossy().into_owned());
        }
        (!components.is_empty()).then(|| components.join("/"))
    } else if path.is_absolute() {
        Some(path.to_string_lossy().into_owned())
    } else {
        None
    }
}

fn detected_profile(
    browser: BrowserKind,
    root: &Path,
    profile_directory: &str,
    profile_name: &str,
) -> DetectedBrowserProfile {
    let (extension_enabled, extension_scan_error) =
        match browser_extension_state(browser, &root.join(profile_directory)) {
            Ok(Some(enabled)) => (enabled, None),
            Ok(None) => (false, None),
            Err(error) => (false, Some(error)),
        };
    let label = if extension_enabled {
        format!(
            "{} — {} ({}) — extension enabled",
            browser.name(),
            profile_name,
            profile_directory
        )
    } else {
        format!(
            "{} — {} ({})",
            browser.name(),
            profile_name,
            profile_directory
        )
    };
    DetectedBrowserProfile {
        target: BrowserLaunchTarget {
            browser,
            profile_directory: profile_directory.to_owned(),
        },
        label,
        extension_enabled,
        profile_root: root.to_path_buf(),
        extension_scan_error,
    }
}

fn browser_extension_state(
    browser: BrowserKind,
    profile_directory: &Path,
) -> Result<Option<bool>, String> {
    let file_name = if browser == BrowserKind::Firefox {
        "extensions.json"
    } else {
        "Preferences"
    };
    let path = profile_directory.join(file_name);
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let preferences =
        serde_json::from_str::<serde_json::Value>(&contents).map_err(|error| error.to_string())?;
    if browser == BrowserKind::Firefox {
        let Some(addons) = preferences
            .get("addons")
            .and_then(serde_json::Value::as_array)
        else {
            return Ok(None);
        };
        let Some(addon) = addons.iter().find(|addon| {
            addon.get("id").and_then(serde_json::Value::as_str) == Some(FIREFOX_EXTENSION_ID.trim())
        }) else {
            return Ok(None);
        };
        return Ok(Some(
            addon
                .get("active")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        ));
    }
    let Some(extension) = preferences
        .get("extensions")
        .and_then(|extensions| extensions.get("settings"))
        .and_then(|settings| settings.get(BROWSER_EXTENSION_ID.trim()))
    else {
        return Ok(None);
    };
    Ok(Some(
        extension.get("state").and_then(serde_json::Value::as_i64) == Some(1),
    ))
}
fn is_valid_profile_directory(directory: &str) -> bool {
    !directory.is_empty()
        && !directory.contains('/')
        && !directory.contains('\\')
        && directory != "."
        && directory != ".."
        && directory != "Guest Profile"
        && directory != "System Profile"
}
fn is_valid_firefox_profile_directory(directory: &str) -> bool {
    !directory.is_empty()
        && !Path::new(directory).components().any(|component| {
            matches!(
                component,
                std::path::Component::CurDir | std::path::Component::ParentDir
            )
        })
}

#[cfg(target_os = "macos")]
fn browser_profile_root(browser: BrowserKind) -> Option<PathBuf> {
    let relative_directory = match browser {
        BrowserKind::Chrome => "Google/Chrome",
        BrowserKind::Brave => "BraveSoftware/Brave-Browser",
        BrowserKind::Edge => "Microsoft Edge",
        BrowserKind::Helium => "net.imput.helium",
        BrowserKind::Firefox => "Firefox",
    };
    Some(
        directories::BaseDirs::new()?
            .data_dir()
            .join(relative_directory),
    )
}

#[cfg(target_os = "windows")]
fn browser_profile_root(browser: BrowserKind) -> Option<PathBuf> {
    let relative_directory = match browser {
        BrowserKind::Chrome => "Google/Chrome",
        BrowserKind::Brave => "BraveSoftware/Brave-Browser",
        BrowserKind::Edge => "Microsoft/Edge",
        BrowserKind::Helium => "imput/Helium",
        BrowserKind::Firefox => {
            return directories::BaseDirs::new()
                .map(|base_dirs| base_dirs.config_dir().join("Mozilla/Firefox"));
        }
    };
    let local_app_data = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| {
            directories::BaseDirs::new().map(|base_dirs| base_dirs.data_local_dir().to_path_buf())
        })?;
    Some(local_app_data.join(relative_directory).join("User Data"))
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn browser_profile_root(_: BrowserKind) -> Option<PathBuf> {
    None
}

fn replace_browser_profile_data_directory(
    directories: &mut Vec<(BrowserKind, String)>,
    browser: BrowserKind,
    path: Option<PathBuf>,
) -> Result<(), String> {
    let path = match path {
        Some(path) if !path.is_absolute() => {
            return Err("The selected browser folder must be an absolute path.".to_owned());
        }
        Some(path) => Some(
            path.to_str()
                .ok_or_else(|| "The selected browser folder path is not valid UTF-8.".to_owned())?
                .to_owned(),
        ),
        None => None,
    };
    directories.retain(|(existing_browser, _)| *existing_browser != browser);
    if let Some(path) = path {
        directories.push((browser, path));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub(super) fn choose_firefox_profile_data_directory() -> Result<Option<PathBuf>, String> {
    let output = std::process::Command::new("/usr/bin/osascript")
        .args([
            "-e",
            "POSIX path of (choose folder with prompt \"Select the Firefox data folder\")",
        ])
        .output()
        .map_err(|error| format!("Could not open the Firefox folder chooser: {error}"))?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr);
        if error.contains("(-128)") {
            return Ok(None);
        }
        return Err(format!(
            "Could not choose the Firefox data folder: {}",
            error.trim()
        ));
    }
    let path = String::from_utf8(output.stdout)
        .map_err(|_| "The selected Firefox folder path is not valid UTF-8.".to_owned())?;
    let path = path.trim();
    if path.is_empty() {
        return Ok(None);
    }
    let path = PathBuf::from(path);
    let profiles_ini_path = path.join("profiles.ini");
    match fs::metadata(&profiles_ini_path) {
        Ok(metadata) if metadata.is_file() => Ok(Some(path)),
        Ok(_) => Err("Select the Firefox data folder containing profiles.ini.".to_owned()),
        Err(error) => Err(format!(
            "Could not inspect Firefox profiles.ini at {}: {error}",
            profiles_ini_path.display()
        )),
    }
}

impl Dashboard {
    pub(super) fn refresh_browser_profiles(&mut self) {
        self.browser_profile_roots =
            detected_browser_profile_roots(&self.browser_profile_data_directories);
        self.browser_profiles =
            detected_browser_profiles_for_roots(&mut self.browser_profile_roots);
        self.browser_launch_attempted = None;
    }

    pub(super) fn set_browser_profile_data_directory(
        &mut self,
        browser: BrowserKind,
        path: Option<PathBuf>,
    ) -> Result<(), String> {
        replace_browser_profile_data_directory(
            &mut self.browser_profile_data_directories,
            browser,
            path,
        )?;
        if self
            .browser_launch_target
            .as_ref()
            .is_some_and(|target| target.browser == browser)
        {
            self.browser_launch_target = None;
        }
        self.refresh_browser_profiles();
        Ok(())
    }
    pub(super) fn apply_firefox_profile_data_directory_selection(
        &mut self,
        selection: Result<Option<PathBuf>, String>,
    ) {
        match selection {
            Ok(Some(path)) => {
                self.browser_profile_folder_error = match self
                    .set_browser_profile_data_directory(BrowserKind::Firefox, Some(path))
                {
                    Ok(()) => None,
                    Err(error) => Some(error),
                };
            }
            Ok(None) => {}
            Err(error) => self.browser_profile_folder_error = Some(error),
        }
    }
    pub(super) fn reset_browser_profile_data_directory(&mut self, browser: BrowserKind) {
        let is_firefox = browser == BrowserKind::Firefox;
        match self.set_browser_profile_data_directory(browser, None) {
            Ok(()) if is_firefox => self.browser_profile_folder_error = None,
            Ok(()) => {}
            Err(error) if is_firefox => self.browser_profile_folder_error = Some(error),
            Err(error) => self.browser_error = Some(error),
        }
    }
}

pub(super) fn launch_browser_profile(
    target: &BrowserLaunchTarget,
    profile_roots: &[DetectedBrowserProfileRoot],
) -> Result<(), String> {
    let application = browser_application_path(target.browser)
        .ok_or_else(|| format!("{} was not found on this computer.", target.browser.name()))?;
    let profile_root = profile_roots
        .iter()
        .find(|root| root.browser == target.browser)
        .ok_or_else(|| {
            format!(
                "Could not find the {} profile data folder.",
                target.browser.name()
            )
        })?;
    if let Some(error) = &profile_root.scan_error {
        return Err(format!(
            "Could not read the {} profile data folder {}: {error}",
            target.browser.name(),
            profile_root.path.display()
        ));
    }
    let profile_root = profile_root.path.clone();
    if !profile_root.is_dir() {
        return Err(format!(
            "The {} profile data folder cannot be read: {}",
            target.browser.name(),
            profile_root.display()
        ));
    }
    let profile_directory_is_valid = if target.browser == BrowserKind::Firefox {
        is_valid_firefox_profile_directory(&target.profile_directory)
    } else {
        is_valid_profile_directory(&target.profile_directory)
    };
    if !profile_directory_is_valid {
        return Err("The selected browser profile is invalid.".to_owned());
    }
    let profile_directory = Path::new(&target.profile_directory);
    let profile_path = if target.browser == BrowserKind::Firefox && profile_directory.is_absolute()
    {
        profile_directory.to_path_buf()
    } else {
        profile_root.join(profile_directory)
    };
    if (target.browser == BrowserKind::Firefox || target.profile_directory != "Default")
        && !profile_path.is_dir()
    {
        return Err(format!(
            "The {} profile {} could not be found at {}.",
            target.browser.name(),
            target.profile_directory,
            profile_path.display()
        ));
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    let firefox_profile_lock_state = if target.browser == BrowserKind::Firefox {
        is_firefox_profile_running(&profile_path)?
    } else {
        FirefoxProfileLockState::Unlocked
    };

    #[cfg(target_os = "macos")]
    {
        let mut command = macos_browser_command(&application, target.browser);
        command.args(browser_profile_launch_arguments(
            target.browser,
            &profile_root,
            &profile_path,
            &target.profile_directory,
            firefox_profile_lock_state,
        )?);
        command
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("Could not open {}: {error}", target.browser.name()))
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new(application)
            .args(browser_profile_launch_arguments(
                target.browser,
                &profile_root,
                &profile_path,
                &target.profile_directory,
                firefox_profile_lock_state,
            )?)
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("Could not open {}: {error}", target.browser.name()))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Err("Automatic browser launch is supported only on macOS and Windows.".to_owned())
    }
}
fn browser_profile_launch_arguments(
    browser: BrowserKind,
    profile_root: &Path,
    profile_path: &Path,
    profile_directory: &str,
    firefox_profile_lock_state: FirefoxProfileLockState,
) -> Result<Vec<std::ffi::OsString>, String> {
    if browser == BrowserKind::Firefox {
        #[cfg(any(target_os = "macos", test))]
        if firefox_profile_lock_state == FirefoxProfileLockState::Unknown {
            return Err(
                "Could not safely launch Firefox because its profile lock owner is unknown."
                    .to_owned(),
            );
        }
        let mut arguments: Vec<std::ffi::OsString> = Vec::with_capacity(5);
        if firefox_profile_lock_state == FirefoxProfileLockState::Unlocked {
            arguments.push("-no-remote".into());
        }
        arguments.extend([
            "-profile".into(),
            profile_path.as_os_str().to_owned(),
            "-new-window".into(),
            browser_extensions_url(browser).into(),
        ]);
        Ok(arguments)
    } else {
        Ok(vec![
            format!("--user-data-dir={}", profile_root.display()).into(),
            format!("--profile-directory={profile_directory}").into(),
            "--new-window".into(),
            browser_extensions_url(browser).into(),
        ])
    }
}

#[cfg(target_os = "macos")]
fn macos_browser_command(application: &Path, browser: BrowserKind) -> std::process::Command {
    if browser == BrowserKind::Firefox {
        std::process::Command::new(application.join("Contents/MacOS/firefox"))
    } else {
        let mut command = std::process::Command::new("open");
        command.arg("-n").arg("-a").arg(application).arg("--args");
        command
    }
}

impl Dashboard {
    pub(super) fn open_extension_setup(&mut self, target: BrowserKind) {
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

fn open_browser_extension_setup(target: BrowserKind) -> Result<(), String> {
    let extension_directory = browser_extension_directory(target)?;
    reveal_extension_directory(&extension_directory)?;
    launch_browser_extensions(target).map_err(|error| {
        format!(
            "The extension folder is open, but {} extensions could not be opened: {error}",
            target.name()
        )
    })
}
#[cfg(target_os = "macos")]
fn launch_browser_extensions(target: BrowserKind) -> Result<(), String> {
    let application = browser_application_path(target)
        .ok_or_else(|| format!("{} is no longer available.", target.name()))?;
    let mut command = std::process::Command::new("open");
    if target == BrowserKind::Helium {
        command
            .arg("-a")
            .arg(application)
            .arg(browser_extensions_url(target));
    } else if target == BrowserKind::Firefox {
        command
            .arg("-a")
            .arg(application)
            .arg("--args")
            .args(browser_extension_window_arguments(target));
    } else {
        command
            .arg("-n")
            .arg("-a")
            .arg(application)
            .arg("--args")
            .args(browser_extension_window_arguments(target));
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
fn launch_browser_extensions(target: BrowserKind) -> Result<(), String> {
    let application = browser_application_path(target)
        .ok_or_else(|| format!("{} is no longer available.", target.name()))?;
    std::process::Command::new(application)
        .args(browser_extension_window_arguments(target))
        .status()
        .map_err(|error| format!("Could not launch {}: {error}", target.name()))
        .and_then(|status| {
            if status.success() {
                Ok(())
            } else {
                Err(format!(
                    "Could not launch {}. Check that it is installed.",
                    target.name()
                ))
            }
        })
}
fn browser_extension_window_arguments(browser: BrowserKind) -> [std::ffi::OsString; 2] {
    let new_window = if browser == BrowserKind::Firefox {
        "-new-window"
    } else {
        "--new-window"
    };
    [new_window.into(), browser_extensions_url(browser).into()]
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn launch_browser_extensions(_: BrowserKind) -> Result<(), String> {
    Err("Opening browser extension settings is supported on macOS and Windows.".to_owned())
}

fn browser_extension_directory(browser: BrowserKind) -> Result<std::path::PathBuf, String> {
    let directory_name = if browser == BrowserKind::Firefox {
        "firefox-extension"
    } else {
        "chrome-extension"
    };
    #[cfg(debug_assertions)]
    {
        let repository_directory = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let source_directory = if browser == BrowserKind::Firefox {
            repository_directory.join("dist").join(directory_name)
        } else {
            repository_directory.join("web-extension")
        };
        if source_directory.join("manifest.json").is_file() {
            return Ok(source_directory);
        }
        if browser == BrowserKind::Firefox {
            return Err(
                "The Firefox extension package is missing. Run `just package-extensions` first."
                    .to_owned(),
            );
        }
    }
    let executable = std::env::current_exe()
        .map_err(|error| format!("Could not locate the app executable: {error}"))?;
    let executable_directory = executable
        .parent()
        .ok_or_else(|| "The app executable has no parent directory.".to_owned())?;
    #[cfg(target_os = "macos")]
    {
        if let Some(contents_directory) = executable_directory.parent() {
            let bundled_directory = contents_directory.join("Resources").join(directory_name);
            if bundled_directory.join("manifest.json").is_file() {
                return Ok(bundled_directory);
            }
        }
    }
    let adjacent_directory = executable_directory.join(directory_name);
    if adjacent_directory.join("manifest.json").is_file() {
        return Ok(adjacent_directory);
    }
    Err(format!(
        "Could not find the {} extension folder next to {}.",
        browser.name(),
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
    use super::{browser_extension_directory, BrowserKind};

    #[test]
    fn resolves_a_complete_extension_folder_for_development() {
        let directory = browser_extension_directory(BrowserKind::Chrome).unwrap();
        assert_eq!(
            directory,
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("web-extension")
        );
        assert!(directory.join("manifest.json").is_file());
        assert!(directory.join("background.js").is_file());
        assert!(directory.join("trusted-extension-id").is_file());
    }
}

#[cfg(test)]
mod browser_profile_tests {
    #[cfg(target_os = "macos")]
    use super::macos_browser_command;
    use super::{
        browser_extension_window_arguments, browser_profile_launch_arguments,
        detected_browser_profiles_for, detected_browser_profiles_for_roots,
        replace_browser_profile_data_directory, selected_browser_profile_data_directory,
        BrowserKind, DetectedBrowserProfileRoot, FirefoxProfileLockState, BROWSER_EXTENSION_ID,
        FIREFOX_EXTENSION_ID,
    };
    use std::fs;
    use std::path::{Path, PathBuf};

    #[cfg(target_os = "macos")]
    #[test]
    fn firefox_profile_root_uses_macos_application_support_directory() {
        let expected_root = directories::BaseDirs::new()
            .unwrap()
            .data_dir()
            .join("Firefox");

        assert_eq!(
            super::browser_profile_root(BrowserKind::Firefox),
            Some(expected_root)
        );
    }

    struct ProfileTestDirectory(PathBuf);

    impl Drop for ProfileTestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write_extension_state(profile_directory: &Path, state: i64) {
        let settings = serde_json::Map::from_iter([(
            BROWSER_EXTENSION_ID.trim().to_owned(),
            serde_json::json!({"state": state}),
        )]);
        let preferences = serde_json::json!({
            "extensions": {
                "settings": settings,
            },
        });
        fs::write(
            profile_directory.join("Preferences"),
            serde_json::to_vec(&preferences).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn custom_browser_folder_enumerates_persists_and_resets_without_changing_other_browsers() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("browser-profile-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("Default")).unwrap();
        fs::create_dir_all(root.join("Profile 1")).unwrap();
        fs::create_dir_all(root.join("Profile 2")).unwrap();
        fs::create_dir_all(root.join("WorkProfile")).unwrap();
        fs::create_dir_all(root.join("Guest Profile")).unwrap();
        fs::write(
            root.join("Local State"),
            r#"{"profile":{"info_cache":{"Default":{"name":"Personal"},"Profile 1":{"name":"Work"},"Guest Profile":{"name":"Guest"}}}}"#,
        )
        .unwrap();
        write_extension_state(&root.join("Default"), 0);
        write_extension_state(&root.join("Profile 1"), 1);
        fs::write(
            root.join("Profile 2").join("Preferences"),
            br#"{"extensions":{"settings":{}}}"#,
        )
        .unwrap();
        write_extension_state(&root.join("WorkProfile"), 1);
        let _cleanup = ProfileTestDirectory(root.clone());

        let root_path = root.to_str().unwrap().to_owned();
        let mut overrides = vec![(BrowserKind::Brave, "/existing/brave-data".to_owned())];
        replace_browser_profile_data_directory(
            &mut overrides,
            BrowserKind::Chrome,
            Some(root.clone()),
        )
        .unwrap();
        let standard_root = root.with_file_name("standard-root");
        let (selected_root, overridden) = selected_browser_profile_data_directory(
            BrowserKind::Chrome,
            &overrides,
            Some(standard_root.clone()),
        )
        .unwrap();
        assert_eq!(selected_root, root);
        assert!(overridden);
        assert_eq!(
            overrides,
            vec![
                (BrowserKind::Brave, "/existing/brave-data".to_owned()),
                (BrowserKind::Chrome, root_path.clone()),
            ]
        );
        let mut roots = [DetectedBrowserProfileRoot {
            browser: BrowserKind::Chrome,
            path: selected_root.clone(),
            scan_error: None,
            overridden: true,
        }];
        let profiles = detected_browser_profiles_for_roots(&mut roots);

        assert_eq!(profiles.len(), 4);
        let enabled = profiles
            .iter()
            .find(|profile| profile.target.profile_directory == "Profile 1")
            .unwrap();
        assert!(enabled.extension_enabled);
        assert_eq!(
            enabled.label,
            "Chrome — Work (Profile 1) — extension enabled"
        );
        let disabled = profiles
            .iter()
            .find(|profile| profile.target.profile_directory == "Default")
            .unwrap();
        assert!(!disabled.extension_enabled);
        assert_eq!(disabled.label, "Chrome — Personal (Default)");
        let absent = profiles
            .iter()
            .find(|profile| profile.target.profile_directory == "Profile 2")
            .unwrap();
        assert!(!absent.extension_enabled);
        let custom = profiles
            .iter()
            .find(|profile| profile.target.profile_directory == "WorkProfile")
            .unwrap();
        assert!(custom.extension_enabled);
        assert_eq!(
            custom.label,
            "Chrome — WorkProfile (WorkProfile) — extension enabled"
        );

        let before_invalid_update = overrides.clone();
        assert!(replace_browser_profile_data_directory(
            &mut overrides,
            BrowserKind::Chrome,
            Some(PathBuf::from("relative-folder")),
        )
        .is_err());
        assert_eq!(overrides, before_invalid_update);
        replace_browser_profile_data_directory(&mut overrides, BrowserKind::Chrome, None).unwrap();
        assert_eq!(
            overrides,
            vec![(BrowserKind::Brave, "/existing/brave-data".to_owned())]
        );
        let (reset_root, overridden) = selected_browser_profile_data_directory(
            BrowserKind::Chrome,
            &overrides,
            Some(standard_root.clone()),
        )
        .unwrap();
        assert_eq!(reset_root, standard_root);
        assert!(!overridden);
    }
    #[test]
    fn unreadable_profile_preferences_report_extension_scan_error() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!(
                "invalid-profile-preferences-test-{}",
                std::process::id()
            ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("Default")).unwrap();
        fs::create_dir(root.join("Default").join("Preferences")).unwrap();
        let _cleanup = ProfileTestDirectory(root.clone());

        let profile = super::detected_profile(BrowserKind::Chrome, &root, "Default", "Personal");
        assert!(!profile.extension_enabled);
        assert!(profile.extension_scan_error.is_some());
    }
    #[test]
    fn profile_directory_names_cannot_escape_browser_root() {
        assert!(!super::is_valid_profile_directory("."));
        assert!(!super::is_valid_profile_directory(".."));
        assert!(super::is_valid_profile_directory("Default"));
    }

    #[test]
    fn missing_profile_data_does_not_invent_default() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!(
                "missing-browser-profile-test-{}",
                std::process::id()
            ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let _cleanup = ProfileTestDirectory(root.clone());

        let profiles = detected_browser_profiles_for(BrowserKind::Chrome, &root).unwrap();

        assert!(profiles.is_empty());
    }
    #[test]
    fn firefox_profiles_load_extension_state_and_reject_traversal() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("firefox-profile-test-{}", std::process::id()));
        let _directory = ProfileTestDirectory(root.clone());
        let default_profile = root.join("Profiles/default-release");
        let work_profile = root.join("Profiles/work");
        let absolute_profile = root.join("external-profile");
        fs::create_dir_all(&default_profile).unwrap();
        fs::create_dir_all(&work_profile).unwrap();
        fs::create_dir_all(&absolute_profile).unwrap();
        fs::write(
            default_profile.join("extensions.json"),
            serde_json::json!({
                "addons": [{
                    "id": FIREFOX_EXTENSION_ID.trim(),
                    "active": true
                }]
            })
            .to_string(),
        )
        .unwrap();
        fs::write(
            work_profile.join("extensions.json"),
            serde_json::json!({
                "addons": [{
                    "id": FIREFOX_EXTENSION_ID.trim(),
                    "active": false
                }]
            })
            .to_string(),
        )
        .unwrap();
        let profiles_ini = format!(
            "[Profile0]\nName=default-release\nIsRelative=1\nPath=Profiles/default-release\n\
             \n[Profile1]\nName=Work\nIsRelative=1\nPath=Profiles/work\n\
             \n[Profile2]\nName=External\nIsRelative=0\nPath={}\n\
             \n[Profile3]\nName=Escaping\nIsRelative=1\nPath=../outside\n",
            absolute_profile.display()
        );
        fs::write(root.join("profiles.ini"), profiles_ini).unwrap();

        let profiles = detected_browser_profiles_for(BrowserKind::Firefox, &root).unwrap();
        assert_eq!(profiles.len(), 3);
        let default = profiles
            .iter()
            .find(|profile| profile.target.profile_directory == "Profiles/default-release")
            .unwrap();
        assert!(default.extension_enabled);
        let work = profiles
            .iter()
            .find(|profile| profile.target.profile_directory == "Profiles/work")
            .unwrap();
        assert!(!work.extension_enabled);
        let absolute = profiles
            .iter()
            .find(|profile| Path::new(&profile.target.profile_directory).is_absolute())
            .unwrap();
        assert_eq!(
            PathBuf::from(&absolute.target.profile_directory),
            absolute_profile
        );
        assert!(profiles.iter().all(|profile| profile.label != "Escaping"));
    }

    #[test]
    fn firefox_profiles_ini_read_error_is_reported() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!(
                "firefox-profiles-ini-error-test-{}",
                std::process::id()
            ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::create_dir(root.join("profiles.ini")).unwrap();
        let _cleanup = ProfileTestDirectory(root.clone());
        let mut roots = [DetectedBrowserProfileRoot {
            browser: BrowserKind::Firefox,
            path: root,
            scan_error: None,
            overridden: false,
        }];

        let profiles = detected_browser_profiles_for_roots(&mut roots);

        assert!(profiles.is_empty());
        let error = roots[0].scan_error.as_deref().unwrap();
        assert!(error.contains("profiles.ini"));
    }
    #[test]
    fn firefox_profile_launch_rejects_unknown_lock_owner() {
        let error = browser_profile_launch_arguments(
            BrowserKind::Firefox,
            Path::new("/firefox"),
            Path::new("/firefox/Profiles/Work Profile"),
            "Profiles/Work Profile",
            FirefoxProfileLockState::Unknown,
        )
        .unwrap_err();
        assert_eq!(
            error,
            "Could not safely launch Firefox because its profile lock owner is unknown."
        );
    }
    #[test]
    fn firefox_profile_launch_uses_gecko_arguments() {
        let profile_root = Path::new("/firefox");
        let profile_path = Path::new("/firefox/Profiles/Work Profile");
        let arguments = browser_profile_launch_arguments(
            BrowserKind::Firefox,
            profile_root,
            profile_path,
            "Profiles/Work Profile",
            FirefoxProfileLockState::Unlocked,
        )
        .unwrap()
        .into_iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
        assert_eq!(
            arguments,
            vec![
                "-no-remote",
                "-profile",
                "/firefox/Profiles/Work Profile",
                "-new-window",
                "about:debugging#/runtime/this-firefox",
            ]
        );
    }
    #[test]
    fn firefox_profile_launch_reuses_running_selected_profile() {
        let profile_root = Path::new("/firefox");
        let profile_path = Path::new("/firefox/Profiles/Work Profile");
        let arguments = browser_profile_launch_arguments(
            BrowserKind::Firefox,
            profile_root,
            profile_path,
            "Profiles/Work Profile",
            FirefoxProfileLockState::Running,
        )
        .unwrap()
        .into_iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
        assert_eq!(
            arguments,
            vec![
                "-profile",
                "/firefox/Profiles/Work Profile",
                "-new-window",
                "about:debugging#/runtime/this-firefox",
            ]
        );
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_firefox_launch_uses_bundled_binary_and_preserves_chromium_open() {
        let firefox =
            macos_browser_command(Path::new("/Applications/Firefox.app"), BrowserKind::Firefox);
        assert_eq!(
            firefox.get_program().to_string_lossy(),
            "/Applications/Firefox.app/Contents/MacOS/firefox",
        );
        assert_eq!(firefox.get_args().count(), 0);

        let chrome_path = Path::new("/Applications/Google Chrome.app");
        let chrome = macos_browser_command(chrome_path, BrowserKind::Chrome);
        assert_eq!(chrome.get_program().to_string_lossy(), "open");
        assert_eq!(
            chrome
                .get_args()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            vec![
                "-n".to_owned(),
                "-a".to_owned(),
                "/Applications/Google Chrome.app".to_owned(),
                "--args".to_owned(),
            ]
        );
    }
    #[test]
    fn extension_setup_uses_browser_specific_window_flags() {
        let firefox = browser_extension_window_arguments(BrowserKind::Firefox)
            .map(|argument| argument.to_string_lossy().into_owned());
        assert_eq!(
            firefox,
            [
                "-new-window".to_owned(),
                "about:debugging#/runtime/this-firefox".to_owned(),
            ]
        );
        let chrome = browser_extension_window_arguments(BrowserKind::Chrome)
            .map(|argument| argument.to_string_lossy().into_owned());
        assert_eq!(
            chrome,
            ["--new-window".to_owned(), "chrome://extensions/".to_owned(),]
        );
    }
}
