use super::{TwitchError, TwitchUser};
use crate::model::TwitchAccount;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

const KEYRING_SERVICE: &str = "Twitch Streak Saver";
const KEYRING_ACCOUNT: &str = "twitch-access-token";
const KEYRING_ACCOUNT_PREFIX: &str = "twitch-access-token-";
const LEGACY_TOKEN_OWNER_ACCOUNT: &str = "twitch-access-token-legacy-owner";
const SAVED_ACCOUNTS_ACCOUNT: &str = "twitch-saved-accounts";

static ACCOUNT_CREDENTIALS_LOCK: Mutex<()> = Mutex::new(());

fn lock_account_credentials() -> Result<MutexGuard<'static, ()>, TwitchError> {
    ACCOUNT_CREDENTIALS_LOCK.lock().map_err(|_| {
        TwitchError::Failed("Secure Twitch account storage is unavailable.".to_owned())
    })
}

pub(super) fn with_active_account_credentials<T>(
    cancel: &AtomicBool,
    operation: impl FnOnce() -> Result<T, TwitchError>,
) -> Result<T, TwitchError> {
    let _guard = lock_account_credentials()?;
    if cancel.load(Ordering::Acquire) {
        return Err(TwitchError::Cancelled);
    }
    // Forget waits for this lock, so an already-started token operation must finish first.
    operation()
}

fn load_keyring_token(keyring_account: &str) -> Result<Option<String>, TwitchError> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        let entry = keyring::Entry::new(KEYRING_SERVICE, keyring_account).map_err(|error| {
            TwitchError::Failed(format!("Could not open secure token storage: {error}"))
        })?;
        return match entry.get_password() {
            Ok(token) => Ok(Some(token)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(TwitchError::Failed(format!(
                "Could not read secure token storage: {error}"
            ))),
        };
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = keyring_account;
        Err(TwitchError::Failed(
            "Secure Twitch token storage is supported on macOS and Windows.".to_owned(),
        ))
    }
}

fn account_keyring_name(account_id: &str) -> Result<String, TwitchError> {
    if account_id.is_empty()
        || account_id.len() > 32
        || !account_id.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(TwitchError::Failed(
            "Twitch returned an invalid account ID.".to_owned(),
        ));
    }
    Ok(format!("{KEYRING_ACCOUNT_PREFIX}{account_id}"))
}

pub(super) fn load_account_token(account_id: &str) -> Result<Option<String>, TwitchError> {
    load_keyring_token(&account_keyring_name(account_id)?)
}
pub(super) fn load_legacy_token() -> Result<Option<String>, TwitchError> {
    load_keyring_token(KEYRING_ACCOUNT)
}

pub(super) fn legacy_token_owner() -> Result<Option<String>, TwitchError> {
    load_keyring_token(LEGACY_TOKEN_OWNER_ACCOUNT)
}

fn load_saved_accounts_from_keyring() -> Result<Vec<TwitchAccount>, TwitchError> {
    let Some(serialized) = load_keyring_token(SAVED_ACCOUNTS_ACCOUNT)? else {
        return Ok(Vec::new());
    };
    parse_saved_accounts(&serialized)
}

fn parse_saved_accounts(serialized: &str) -> Result<Vec<TwitchAccount>, TwitchError> {
    let accounts: Vec<TwitchAccount> = serde_json::from_str(serialized)
        .map_err(|_| TwitchError::Failed("Could not read saved Twitch accounts.".to_owned()))?;
    for account in &accounts {
        account_keyring_name(&account.id)?;
    }
    Ok(accounts)
}

fn upsert_saved_account_identity(accounts: &mut Vec<TwitchAccount>, account_id: &str, login: &str) {
    if let Some(account) = accounts.iter_mut().find(|account| account.id == account_id) {
        account.login = login.to_owned();
    } else {
        accounts.push(TwitchAccount {
            id: account_id.to_owned(),
            login: login.to_owned(),
        });
    }
}

fn store_saved_accounts(accounts: &[TwitchAccount]) -> Result<(), TwitchError> {
    let serialized = serde_json::to_string(accounts).map_err(|_| {
        TwitchError::Failed("Could not serialize saved Twitch accounts.".to_owned())
    })?;
    store_keyring_token(SAVED_ACCOUNTS_ACCOUNT, &serialized)
}

fn save_account_identity(account_id: &str, login: &str) -> Result<(), TwitchError> {
    let mut accounts = load_saved_accounts_from_keyring()?;
    upsert_saved_account_identity(&mut accounts, account_id, login);
    store_saved_accounts(&accounts)
}

fn remove_account_identity(account_id: &str) -> Result<(), TwitchError> {
    let mut accounts = load_saved_accounts_from_keyring()?;
    let previous_len = accounts.len();
    accounts.retain(|account| account.id != account_id);
    if accounts.len() == previous_len {
        return Ok(());
    }
    if accounts.is_empty() {
        clear_keyring_token(SAVED_ACCOUNTS_ACCOUNT)
    } else {
        store_saved_accounts(&accounts)
    }
}

fn store_keyring_token(keyring_account: &str, token: &str) -> Result<(), TwitchError> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        let entry = keyring::Entry::new(KEYRING_SERVICE, keyring_account).map_err(|error| {
            TwitchError::Failed(format!("Could not open secure token storage: {error}"))
        })?;
        return entry.set_password(token).map_err(|error| {
            TwitchError::Failed(format!("Could not save Twitch token securely: {error}"))
        });
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (keyring_account, token);
        Err(TwitchError::Failed(
            "Secure Twitch token storage is supported on macOS and Windows.".to_owned(),
        ))
    }
}

fn clear_keyring_token(keyring_account: &str) -> Result<(), TwitchError> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        let entry = keyring::Entry::new(KEYRING_SERVICE, keyring_account).map_err(|error| {
            TwitchError::Failed(format!("Could not open secure token storage: {error}"))
        })?;
        return match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(TwitchError::Failed(format!(
                "Could not remove saved token: {error}"
            ))),
        };
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = keyring_account;
        Err(TwitchError::Failed(
            "Secure Twitch token storage is supported on macOS and Windows.".to_owned(),
        ))
    }
}

pub(super) fn clear_account_token(
    account_id: &str,
    cancel: &AtomicBool,
) -> Result<(), TwitchError> {
    with_active_account_credentials(cancel, || clear_account_token_unlocked(account_id))
}

fn clear_account_token_unlocked(account_id: &str) -> Result<(), TwitchError> {
    clear_keyring_token(&account_keyring_name(account_id)?)
}

pub fn remove_saved_account_token(account_id: &str) -> Result<(), String> {
    let _guard = lock_account_credentials().map_err(TwitchError::message)?;
    clear_account_token_unlocked(account_id).map_err(TwitchError::message)?;
    if load_keyring_token(LEGACY_TOKEN_OWNER_ACCOUNT)
        .map_err(TwitchError::message)?
        .as_deref()
        == Some(account_id)
    {
        clear_legacy_keyring_token_unlocked().map_err(TwitchError::message)?;
    }
    remove_account_identity(account_id).map_err(TwitchError::message)
}

pub fn load_saved_accounts() -> Result<Vec<TwitchAccount>, String> {
    load_saved_accounts_from_keyring().map_err(TwitchError::message)
}

pub(super) fn clear_legacy_keyring_token(cancel: &AtomicBool) -> Result<(), TwitchError> {
    with_active_account_credentials(cancel, clear_legacy_keyring_token_unlocked)
}

fn clear_legacy_keyring_token_unlocked() -> Result<(), TwitchError> {
    clear_keyring_token(KEYRING_ACCOUNT)?;
    clear_keyring_token(LEGACY_TOKEN_OWNER_ACCOUNT)
}
pub(super) fn save_validated_account(
    user: &TwitchUser,
    token: &str,
    legacy_token: bool,
    cancel: &AtomicBool,
) -> Result<(), TwitchError> {
    with_active_account_credentials(cancel, || {
        let account_keyring_name = account_keyring_name(&user.id)?;
        let remove_legacy = if legacy_token {
            store_keyring_token(LEGACY_TOKEN_OWNER_ACCOUNT, &user.id)?;
            true
        } else {
            load_keyring_token(LEGACY_TOKEN_OWNER_ACCOUNT)?.as_deref() == Some(user.id.as_str())
        };
        save_account_identity(&user.id, &user.login)?;
        store_keyring_token(&account_keyring_name, token)?;
        if remove_legacy {
            clear_legacy_keyring_token_unlocked()?;
        }
        Ok(())
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn account_credential_names_require_twitch_numeric_ids() {
        assert_eq!(
            account_keyring_name("123456").unwrap(),
            "twitch-access-token-123456"
        );
        assert!(account_keyring_name("").is_err());
        assert!(account_keyring_name("viewer-123").is_err());
        assert!(account_keyring_name("123456789012345678901234567890123").is_err());
    }

    #[test]
    fn saved_account_registry_keeps_ids_distinct_and_updates_login() {
        let mut accounts = Vec::new();
        upsert_saved_account_identity(&mut accounts, "111111", "main");
        upsert_saved_account_identity(&mut accounts, "222222", "test");
        upsert_saved_account_identity(&mut accounts, "111111", "main_renamed");

        let encoded = serde_json::to_string(&accounts).unwrap();
        assert_eq!(
            parse_saved_accounts(&encoded).unwrap(),
            vec![
                TwitchAccount {
                    id: "111111".to_owned(),
                    login: "main_renamed".to_owned(),
                },
                TwitchAccount {
                    id: "222222".to_owned(),
                    login: "test".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn cancelled_connection_cannot_commit_account_credentials() {
        use std::sync::atomic::AtomicBool;

        let cancel = AtomicBool::new(true);
        let mut wrote_credentials = false;
        let result = with_active_account_credentials(&cancel, || {
            wrote_credentials = true;
            Ok(())
        });

        assert!(matches!(result, Err(TwitchError::Cancelled)));
        assert!(!wrote_credentials);
    }

    #[test]
    fn completed_account_credential_commit_remains_successful_after_cancel() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let cancel = AtomicBool::new(false);
        let result = with_active_account_credentials(&cancel, || {
            cancel.store(true, Ordering::Release);
            Ok(42)
        });

        assert!(matches!(result, Ok(42)));
    }
    #[test]
    fn forget_waits_for_in_flight_account_token_use() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{mpsc, Arc, TryLockError};
        use std::thread;

        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let (event_tx, event_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let validation_events = event_tx.clone();
        let validation = thread::spawn(move || {
            with_active_account_credentials(&worker_cancel, || {
                validation_events.send("request started").unwrap();
                release_rx.recv().unwrap();
                validation_events.send("request finished").unwrap();
                Ok(())
            })
        });

        assert_eq!(event_rx.recv().unwrap(), "request started");
        cancel.store(true, Ordering::Release);
        let (forget_waiting_tx, forget_waiting_rx) = mpsc::channel();
        let forget_events = event_tx.clone();
        let forget = thread::spawn(move || {
            let waiting = matches!(
                ACCOUNT_CREDENTIALS_LOCK.try_lock(),
                Err(TryLockError::WouldBlock)
            );
            forget_waiting_tx.send(waiting).unwrap();
            if waiting {
                let _guard = lock_account_credentials().unwrap();
                forget_events.send("forget completed").unwrap();
            }
        });
        let waiting = forget_waiting_rx.recv().unwrap();
        release_tx.send(()).unwrap();

        assert_eq!(event_rx.recv().unwrap(), "request finished");
        let validation_result = validation.join().unwrap();
        forget.join().unwrap();
        assert!(waiting);
        assert_eq!(event_rx.recv().unwrap(), "forget completed");
        assert!(matches!(validation_result, Ok(())));
        let mut request_started = false;
        assert!(matches!(
            with_active_account_credentials(&cancel, || {
                request_started = true;
                Ok(())
            }),
            Err(TwitchError::Cancelled)
        ));
        assert!(!request_started);
    }
}
