use super::{BrowserKind, BrowserLaunchTarget, Dashboard, BROWSER_EXTENSION_ID};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
fn browser_extensions_url(browser: BrowserKind) -> &'static str {
    match browser {
        BrowserKind::Chrome => "chrome://extensions/",
        BrowserKind::Brave => "brave://extensions/",
        BrowserKind::Edge => "edge://extensions/",
        BrowserKind::Helium => "helium://extensions/",
    }
}

pub(super) fn detected_setup_browsers() -> Vec<BrowserKind> {
    [
        BrowserKind::Chrome,
        BrowserKind::Brave,
        BrowserKind::Edge,
        BrowserKind::Helium,
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
    roots: &[DetectedBrowserProfileRoot],
) -> Vec<DetectedBrowserProfile> {
    let mut profiles = Vec::new();
    for root in roots {
        profiles.extend(detected_browser_profiles_for(root.browser, &root.path));
    }
    profiles.sort_by(|left, right| {
        right
            .extension_enabled
            .cmp(&left.extension_enabled)
            .then_with(|| left.label.cmp(&right.label))
    });
    profiles
}

fn detected_browser_profiles_for(browser: BrowserKind, root: &Path) -> Vec<DetectedBrowserProfile> {
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

    browser_profiles
}

fn detected_profile(
    browser: BrowserKind,
    root: &Path,
    profile_directory: &str,
    profile_name: &str,
) -> DetectedBrowserProfile {
    let (extension_enabled, extension_scan_error) =
        match browser_extension_state(&root.join(profile_directory)) {
            Ok(state) => (state == Some(true), None),
            Err(error) => (false, Some(error)),
        };
    let extension_status = if extension_enabled {
        " — extension enabled"
    } else {
        ""
    };
    DetectedBrowserProfile {
        target: BrowserLaunchTarget {
            browser,
            profile_directory: profile_directory.to_owned(),
        },
        label: format!(
            "{} — {} ({profile_directory}){extension_status}",
            browser.name(),
            profile_name
        ),
        extension_enabled,
        profile_root: root.to_path_buf(),
        extension_scan_error,
    }
}

fn browser_extension_state(profile_directory: &Path) -> Result<Option<bool>, String> {
    let preferences_path = profile_directory.join("Preferences");
    let preferences = match fs::read_to_string(&preferences_path) {
        Ok(preferences) => preferences,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let preferences = serde_json::from_str::<serde_json::Value>(&preferences)
        .map_err(|error| error.to_string())?;
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

#[cfg(target_os = "macos")]
fn browser_profile_root(browser: BrowserKind) -> Option<PathBuf> {
    let relative_directory = match browser {
        BrowserKind::Chrome => "Google/Chrome",
        BrowserKind::Brave => "BraveSoftware/Brave-Browser",
        BrowserKind::Edge => "Microsoft Edge",
        BrowserKind::Helium => "net.imput.helium",
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

impl Dashboard {
    pub(super) fn refresh_browser_profiles(&mut self) {
        self.browser_profile_roots =
            detected_browser_profile_roots(&self.browser_profile_data_directories);
        self.browser_profiles = detected_browser_profiles_for_roots(&self.browser_profile_roots);
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
    if !is_valid_profile_directory(&target.profile_directory) {
        return Err("The selected browser profile is invalid.".to_owned());
    }
    if target.profile_directory != "Default"
        && !profile_root.join(&target.profile_directory).is_dir()
    {
        return Err(format!(
            "The {} profile {} could not be found under {}.",
            target.browser.name(),
            target.profile_directory,
            profile_root.display()
        ));
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("-n")
            .arg("-a")
            .arg(application)
            .arg("--args")
            .arg(format!("--user-data-dir={}", profile_root.display()))
            .arg(format!("--profile-directory={}", target.profile_directory))
            .arg("--new-window")
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("Could not open {}: {error}", target.browser.name()))
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new(application)
            .arg(format!("--user-data-dir={}", profile_root.display()))
            .arg(format!("--profile-directory={}", target.profile_directory))
            .arg("--new-window")
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("Could not open {}: {error}", target.browser.name()))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Err("Automatic browser launch is supported only on macOS and Windows.".to_owned())
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
fn launch_browser_extensions(target: BrowserKind) -> Result<(), String> {
    let application = browser_application_path(target)
        .ok_or_else(|| format!("{} is no longer available.", target.name()))?;
    let mut command = std::process::Command::new("open");
    if target == BrowserKind::Helium {
        command
            .arg("-a")
            .arg(application)
            .arg(browser_extensions_url(target));
    } else {
        command
            .arg("-n")
            .arg("-a")
            .arg(application)
            .arg("--args")
            .arg("--new-window")
            .arg(browser_extensions_url(target));
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
    let executable = browser_application_path(target)
        .ok_or_else(|| format!("{} is no longer available.", target.name()))?;
    std::process::Command::new(executable)
        .arg("--new-window")
        .arg(browser_extensions_url(target))
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Could not launch {}: {error}", target.name()))
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn launch_browser_extensions(_: BrowserKind) -> Result<(), String> {
    Err("Opening browser extension settings is supported on macOS and Windows.".to_owned())
}

fn browser_extension_directory() -> Result<std::path::PathBuf, String> {
    #[cfg(debug_assertions)]
    {
        let source_directory =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("chrome-extension");
        if source_directory.join("manifest.json").is_file() {
            return Ok(source_directory);
        }
    }

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
        assert_eq!(
            directory,
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("chrome-extension")
        );
        assert!(directory.join("manifest.json").is_file());
        assert!(directory.join("background.js").is_file());
        assert!(directory.join("trusted-extension-id").is_file());
    }
}

#[cfg(test)]
mod browser_profile_tests {
    use super::{
        detected_browser_profiles_for, detected_browser_profiles_for_roots,
        replace_browser_profile_data_directory, selected_browser_profile_data_directory,
        BrowserKind, DetectedBrowserProfileRoot, BROWSER_EXTENSION_ID,
    };
    use std::fs;
    use std::path::{Path, PathBuf};

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

        let roots = [DetectedBrowserProfileRoot {
            browser: BrowserKind::Chrome,
            path: selected_root.clone(),
            scan_error: None,
            overridden: true,
        }];
        let profiles = detected_browser_profiles_for_roots(&roots);

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

        let profiles = detected_browser_profiles_for(BrowserKind::Chrome, &root);

        assert!(profiles.is_empty());
    }
}
