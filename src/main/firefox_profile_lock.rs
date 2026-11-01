use super::FirefoxProfileLockState;
use std::path::Path;

#[cfg(target_os = "macos")]
pub(super) fn is_running(profile_path: &Path) -> Result<FirefoxProfileLockState, String> {
    use std::os::fd::AsRawFd;
    let mut lock_state = FirefoxProfileLockState::Unlocked;
    let mut fcntl_lock_available = false;

    for lock_name in [".parentlock", "parent.lock"] {
        let lock_path = profile_path.join(lock_name);
        let metadata = match std::fs::symlink_metadata(&lock_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "Could not inspect Firefox profile lock {}: {error}",
                    lock_path.display()
                ));
            }
        };
        if metadata.file_type().is_symlink() {
            match symlink_lock_owner_state(&lock_path, fcntl_lock_available)? {
                FirefoxProfileLockState::Running => {
                    return Ok(FirefoxProfileLockState::Running);
                }
                FirefoxProfileLockState::Unlocked => {}
                FirefoxProfileLockState::Unknown => {
                    lock_state = FirefoxProfileLockState::Unknown;
                }
            }
            continue;
        }
        let lock_file = match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "Could not inspect Firefox profile lock {}: {error}",
                    lock_path.display()
                ));
            }
        };
        let mut lock: libc::flock = unsafe { std::mem::zeroed() };
        lock.l_type = libc::F_WRLCK as _;
        lock.l_whence = libc::SEEK_SET as _;
        if unsafe { libc::fcntl(lock_file.as_raw_fd(), libc::F_GETLK, &mut lock) } == -1 {
            return Err(format!(
                "Could not inspect Firefox profile lock {}: {}",
                lock_path.display(),
                std::io::Error::last_os_error()
            ));
        }
        fcntl_lock_available = true;
        if lock.l_type != libc::F_UNLCK as libc::c_short {
            return Ok(FirefoxProfileLockState::Running);
        }
    }
    Ok(lock_state)
}

#[cfg(target_os = "windows")]
pub(super) fn is_running(profile_path: &Path) -> Result<FirefoxProfileLockState, String> {
    use std::os::windows::fs::OpenOptionsExt;

    let lock_path = profile_path.join("parent.lock");
    match std::fs::symlink_metadata(&lock_path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(format!(
                "Could not safely determine whether Firefox profile {} is running because its lock is a symlink.",
                profile_path.display()
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(FirefoxProfileLockState::Unlocked);
        }
        Err(error) => {
            return Err(format!(
                "Could not inspect Firefox profile lock {}: {error}",
                lock_path.display()
            ));
        }
    }

    match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(0)
        .open(&lock_path)
    {
        Ok(_) => Ok(FirefoxProfileLockState::Unlocked),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(FirefoxProfileLockState::Unlocked)
        }
        Err(error) if error.raw_os_error() == Some(32) => Ok(FirefoxProfileLockState::Running),
        Err(error) => Err(format!(
            "Could not inspect Firefox profile lock {}: {error}",
            lock_path.display()
        )),
    }
}

#[cfg(target_os = "macos")]
fn symlink_lock_owner_state(
    lock_path: &Path,
    fcntl_lock_available: bool,
) -> Result<FirefoxProfileLockState, String> {
    use std::net::Ipv4Addr;

    let target = match std::fs::read_link(lock_path) {
        Ok(target) => target,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(FirefoxProfileLockState::Unlocked);
        }
        Err(error) => {
            return Err(format!(
                "Could not inspect Firefox profile lock {}: {error}",
                lock_path.display()
            ));
        }
    };
    let Some((address, pid)) = target.to_str().and_then(|target| target.split_once(':')) else {
        return Ok(FirefoxProfileLockState::Unknown);
    };
    let Ok(address) = address.parse::<Ipv4Addr>() else {
        return Ok(FirefoxProfileLockState::Unknown);
    };
    let fcntl_lock_owner = pid.starts_with('+');
    if fcntl_lock_owner && fcntl_lock_available {
        return Ok(FirefoxProfileLockState::Unlocked);
    }
    let pid = pid.strip_prefix('+').unwrap_or(pid);
    let Ok(pid) = pid.parse::<libc::pid_t>() else {
        return Ok(FirefoxProfileLockState::Unknown);
    };
    if pid <= 0 || !is_local_ipv4_address(address)? {
        return Ok(FirefoxProfileLockState::Unknown);
    }

    if unsafe { libc::kill(pid, 0) } == 0 {
        return Ok(FirefoxProfileLockState::Running);
    }
    match std::io::Error::last_os_error().raw_os_error() {
        Some(libc::ESRCH) => Ok(FirefoxProfileLockState::Unlocked),
        Some(libc::EPERM) => Ok(FirefoxProfileLockState::Running),
        _ => Ok(FirefoxProfileLockState::Unknown),
    }
}
#[cfg(target_os = "macos")]
fn is_local_ipv4_address(address: std::net::Ipv4Addr) -> Result<bool, String> {
    if address.is_loopback() {
        return Ok(true);
    }

    let mut interfaces: *mut libc::ifaddrs = std::ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut interfaces) } != 0 {
        return Err(format!(
            "Could not inspect local network interfaces: {}",
            std::io::Error::last_os_error()
        ));
    }

    let mut current = interfaces;
    let mut found = false;
    while !current.is_null() {
        let interface = unsafe { &*current };
        if !interface.ifa_addr.is_null()
            && unsafe { (*interface.ifa_addr).sa_family as i32 } == libc::AF_INET
        {
            let socket_address = unsafe { &*(interface.ifa_addr as *const libc::sockaddr_in) };
            if std::net::Ipv4Addr::from(u32::from_be(socket_address.sin_addr.s_addr)) == address {
                found = true;
                break;
            }
        }
        current = interface.ifa_next;
    }
    unsafe { libc::freeifaddrs(interfaces) };
    Ok(found)
}
#[cfg(test)]
mod tests {

    use super::{is_running, FirefoxProfileLockState};
    use std::fs;
    use std::path::PathBuf;

    struct TestProfile(PathBuf);

    impl TestProfile {
        fn new(name: &str) -> Self {
            let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join(format!(
                    "firefox-profile-lock-{name}-{}",
                    std::process::id()
                ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestProfile {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn unlocked_profile_lock_file_is_not_mistaken_for_a_running_profile() {
        let profile = TestProfile::new("stale");
        fs::write(profile.0.join(".parentlock"), b"").unwrap();
        assert_eq!(
            is_running(&profile.0).unwrap(),
            FirefoxProfileLockState::Unlocked
        );
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn live_fcntl_marker_is_stale_when_the_lock_is_unheld() {
        use std::os::unix::fs::symlink;

        let profile = TestProfile::new("stale-plus-marker");
        fs::write(profile.0.join(".parentlock"), b"").unwrap();
        symlink(
            format!("127.0.0.1:+{}", std::process::id()),
            profile.0.join("parent.lock"),
        )
        .unwrap();
        assert_eq!(
            is_running(&profile.0).unwrap(),
            FirefoxProfileLockState::Unlocked
        );
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn stale_parentlock_symlink_is_unlocked() {
        use std::os::unix::fs::symlink;

        let profile = TestProfile::new("stale-symlink");
        fs::write(profile.0.join(".parentlock"), b"").unwrap();
        symlink(
            format!("127.0.0.1:{}", i32::MAX),
            profile.0.join("parent.lock"),
        )
        .unwrap();
        assert_eq!(
            is_running(&profile.0).unwrap(),
            FirefoxProfileLockState::Unlocked
        );
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn stale_profile_lock_symlink_is_unlocked() {
        use std::os::unix::fs::symlink;

        let profile = TestProfile::new("stale-fallback-symlink");
        symlink(
            format!("127.0.0.1:{}", i32::MAX),
            profile.0.join(".parentlock"),
        )
        .unwrap();
        assert_eq!(
            is_running(&profile.0).unwrap(),
            FirefoxProfileLockState::Unlocked
        );
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn detects_a_live_profile_lock_symlink_owner() {
        use std::os::unix::fs::symlink;

        let profile = TestProfile::new("active-fallback-symlink");
        symlink(
            format!("127.0.0.1:{}", std::process::id()),
            profile.0.join(".parentlock"),
        )
        .unwrap();
        assert_eq!(
            is_running(&profile.0).unwrap(),
            FirefoxProfileLockState::Running
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn remote_profile_lock_symlink_owner_is_unknown() {
        use std::os::unix::fs::symlink;

        let profile = TestProfile::new("remote-fallback-symlink");
        symlink("203.0.113.1:12345", profile.0.join(".parentlock")).unwrap();
        assert_eq!(
            is_running(&profile.0).unwrap(),
            FirefoxProfileLockState::Unknown
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn unparseable_profile_lock_symlink_owner_is_unknown() {
        use std::os::unix::fs::symlink;

        let profile = TestProfile::new("unknown-fallback-symlink");
        symlink("not-a-lock-owner", profile.0.join(".parentlock")).unwrap();
        assert_eq!(
            is_running(&profile.0).unwrap(),
            FirefoxProfileLockState::Unknown
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn profile_lock_holder_child() {
        let Some(lock_path) = std::env::var_os("TSS_FIREFOX_PROFILE_LOCK_TEST_PATH") else {
            return;
        };
        use std::os::fd::AsRawFd;

        let lock_file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(lock_path)
            .unwrap();
        let mut lock: libc::flock = unsafe { std::mem::zeroed() };
        lock.l_type = libc::F_WRLCK as _;
        lock.l_whence = libc::SEEK_SET as _;
        assert_eq!(
            unsafe { libc::fcntl(lock_file.as_raw_fd(), libc::F_SETLK, &mut lock) },
            0
        );
        println!("LOCK_HELD");
        std::io::Write::flush(&mut std::io::stdout()).unwrap();
        std::thread::sleep(std::time::Duration::from_secs(10));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn detects_a_profile_locked_by_another_process() {
        use std::io::BufRead;
        use std::os::unix::fs::symlink;

        let profile = TestProfile::new("active");
        let lock_path = profile.0.join(".parentlock");
        fs::write(&lock_path, b"").unwrap();
        symlink("profile-owner", profile.0.join("parent.lock")).unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--nocapture", "profile_lock_holder_child"])
            .env("TSS_FIREFOX_PROFILE_LOCK_TEST_PATH", &lock_path)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut output = std::io::BufReader::new(stdout);
        let mut line = String::new();
        let mut lock_held = false;
        while output.read_line(&mut line).unwrap_or(0) > 0 {
            if line.contains("LOCK_HELD") {
                lock_held = true;
                break;
            }
            line.clear();
        }
        let running = is_running(&profile.0);
        assert_eq!(running.unwrap(), FirefoxProfileLockState::Running);
        let _ = child.kill();
        let _ = child.wait();
        assert!(lock_held);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn detects_and_releases_an_exclusively_open_profile_lock() {
        use std::os::windows::fs::OpenOptionsExt;

        let profile = TestProfile::new("active");
        let lock_file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(0)
            .open(profile.0.join("parent.lock"))
            .unwrap();
        assert_eq!(
            is_running(&profile.0).unwrap(),
            FirefoxProfileLockState::Running
        );
        drop(lock_file);
        assert_eq!(
            is_running(&profile.0).unwrap(),
            FirefoxProfileLockState::Unlocked
        );
    }
}
