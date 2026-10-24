use super::api::{build_client, fetch_followed_channels, fetch_live_channel_ids};
use super::oauth::{clear_keyring_token, obtain_token};
use super::pubsub::run_pubsub_listener;
use super::*;
pub fn run_connection(cancel: Arc<AtomicBool>, generation: u64, updates: Sender<TwitchUpdate>) {
    let client = match build_client() {
        Ok(client) => client,
        Err(error) => {
            send_failed(&updates, generation, error.message());
            return;
        }
    };
    let (token, user) = match retry_until_success(
        &cancel,
        generation,
        &updates,
        super::TwitchNetworkSource::Connection,
        NETWORK_RETRY_INITIAL,
        || obtain_token(&client, &cancel),
    ) {
        Ok(credentials) => credentials,
        Err(TwitchError::Cancelled) => return,
        Err(error @ TwitchError::Reauthorize(_)) => {
            fail_expired_session(&updates, generation, error);
            return;
        }
        Err(error) => {
            send_failed(&updates, generation, error.message());
            return;
        }
    };
    let (mut channels, mut live_channel_ids) = match retry_until_success(
        &cancel,
        generation,
        &updates,
        super::TwitchNetworkSource::Connection,
        NETWORK_RETRY_INITIAL,
        || {
            let channels = fetch_followed_channels(&client, &token, &user, &cancel)?;
            let live_channel_ids = fetch_live_channel_ids(&client, &token, &channels, &cancel)?;
            Ok((channels, live_channel_ids))
        },
    ) {
        Ok(baseline) => baseline,
        Err(TwitchError::Cancelled) => return,
        Err(error @ TwitchError::Reauthorize(_)) => {
            fail_expired_session(&updates, generation, error);
            return;
        }
        Err(error) => {
            send_failed(&updates, generation, error.message());
            return;
        }
    };
    if updates
        .send(TwitchUpdate::Connected {
            generation,
            login: user.login.clone(),
            channels: channels.clone(),
            live_channel_ids: live_channel_ids.clone(),
        })
        .is_err()
    {
        return;
    }

    let pubsub_cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = Arc::clone(&pubsub_cancel);
    let pubsub_updates = updates.clone();
    let pubsub_token = token.clone();
    let pubsub_user_id = user.id.clone();
    if let Err(error) = thread::Builder::new()
        .name("twitch-pubsub-listener".to_owned())
        .spawn(move || {
            run_pubsub_listener(
                worker_cancel,
                generation,
                pubsub_user_id,
                pubsub_token,
                pubsub_updates,
            )
        })
    {
        let _ = updates.send(TwitchUpdate::PubSubStatus {
            generation,
            connected: false,
            detail: Some(format!(
                "Could not start Channel Points/Streak event listener: {error}"
            )),
        });
    }

    let mut last_live_refresh = Instant::now();
    let mut last_follow_refresh = Instant::now();
    let mut live_refresh_interval = LIVE_REFRESH_INTERVAL;
    let mut live_retry_delay = NETWORK_RETRY_INITIAL;
    let mut follow_refresh_interval = FOLLOW_REFRESH_INTERVAL;
    let mut follow_retry_delay = NETWORK_RETRY_INITIAL;
    while !cancel.load(Ordering::Acquire) {
        thread::sleep(Duration::from_millis(250));
        if cancel.load(Ordering::Acquire) {
            break;
        }

        if last_live_refresh.elapsed() >= live_refresh_interval {
            match fetch_live_channel_ids(&client, &token, &channels, &cancel) {
                Ok(live) => {
                    live_channel_ids = live;
                    live_refresh_interval = LIVE_REFRESH_INTERVAL;
                    live_retry_delay = NETWORK_RETRY_INITIAL;
                    if updates
                        .send(TwitchUpdate::LiveStatus {
                            generation,
                            live_channel_ids: live_channel_ids.clone(),
                        })
                        .is_err()
                    {
                        break;
                    }
                }
                Err(TwitchError::Cancelled) => break,
                Err(error @ TwitchError::Reauthorize(_)) => {
                    fail_expired_session(&updates, generation, error);
                    break;
                }
                Err(TwitchError::Retryable(message)) => {
                    let delay = live_retry_delay;
                    live_refresh_interval = delay;
                    live_retry_delay = next_retry_delay(live_retry_delay);
                    let _ = updates.send(TwitchUpdate::NetworkUnavailable {
                        generation,
                        source: super::TwitchNetworkSource::LiveStatus,
                        message,
                    });
                }
                Err(error) => {
                    let _ = updates.send(TwitchUpdate::Warning {
                        generation,
                        message: error.message(),
                    });
                }
            }
            last_live_refresh = Instant::now();
        }

        if last_follow_refresh.elapsed() >= follow_refresh_interval {
            match fetch_followed_channels(&client, &token, &user, &cancel) {
                Ok(refreshed_channels) => {
                    channels = refreshed_channels;
                    follow_refresh_interval = FOLLOW_REFRESH_INTERVAL;
                    follow_retry_delay = NETWORK_RETRY_INITIAL;
                    let live_result = fetch_live_channel_ids(&client, &token, &channels, &cancel);
                    last_live_refresh = Instant::now();
                    match live_result {
                        Ok(live) => {
                            live_channel_ids = live;
                            live_refresh_interval = LIVE_REFRESH_INTERVAL;
                            live_retry_delay = NETWORK_RETRY_INITIAL;
                            if updates
                                .send(TwitchUpdate::Refreshed {
                                    generation,
                                    channels: channels.clone(),
                                    live_channel_ids: live_channel_ids.clone(),
                                })
                                .is_err()
                            {
                                break;
                            }
                        }
                        Err(TwitchError::Cancelled) => break,
                        Err(error @ TwitchError::Reauthorize(_)) => {
                            fail_expired_session(&updates, generation, error);
                            break;
                        }
                        Err(TwitchError::Retryable(message)) => {
                            let live_delay = live_retry_delay;
                            live_refresh_interval = live_delay;
                            live_retry_delay = next_retry_delay(live_retry_delay);
                            if updates
                                .send(TwitchUpdate::FollowedChannels {
                                    generation,
                                    channels: channels.clone(),
                                    live_status_failure: super::TwitchUpdateFailure::Retryable(
                                        message,
                                    ),
                                })
                                .is_err()
                            {
                                break;
                            }
                        }
                        Err(error) => {
                            if updates
                                .send(TwitchUpdate::FollowedChannels {
                                    generation,
                                    channels: channels.clone(),
                                    live_status_failure: super::TwitchUpdateFailure::Warning(
                                        error.message(),
                                    ),
                                })
                                .is_err()
                            {
                                break;
                            }
                        }
                    }
                }
                Err(TwitchError::Cancelled) => break,
                Err(error @ TwitchError::Reauthorize(_)) => {
                    fail_expired_session(&updates, generation, error);
                    break;
                }
                Err(TwitchError::Retryable(message)) => {
                    let follow_delay = follow_retry_delay;
                    follow_refresh_interval = follow_delay;
                    follow_retry_delay = next_retry_delay(follow_retry_delay);
                    let _ = updates.send(TwitchUpdate::NetworkUnavailable {
                        generation,
                        source: super::TwitchNetworkSource::FollowedChannels,
                        message,
                    });
                }
                Err(error) => {
                    let _ = updates.send(TwitchUpdate::Warning {
                        generation,
                        message: error.message(),
                    });
                }
            }
            last_follow_refresh = Instant::now();
        }
    }
    pubsub_cancel.store(true, Ordering::Release);
}
fn next_retry_delay(current: Duration) -> Duration {
    current.saturating_mul(2).min(NETWORK_RETRY_MAX)
}

fn wait_for_retry(cancel: &AtomicBool, delay: Duration) -> bool {
    let deadline = Instant::now() + delay;
    while !cancel.load(Ordering::Acquire) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }
        thread::sleep(remaining.min(Duration::from_millis(100)));
    }
    true
}

fn send_failed(updates: &Sender<TwitchUpdate>, generation: u64, message: String) {
    let _ = updates.send(TwitchUpdate::Failed {
        generation,
        message,
    });
}

fn fail_expired_session(updates: &Sender<TwitchUpdate>, generation: u64, error: TwitchError) {
    let message = match clear_keyring_token() {
        Ok(()) => error.message(),
        Err(clear_error) => format!(
            "{}; could not remove saved token: {}",
            error.message(),
            clear_error.message()
        ),
    };
    send_failed(updates, generation, message);
}

#[cfg(test)]
mod tests {
    use super::{next_retry_delay, wait_for_retry};
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;

    #[test]
    fn network_retry_delay_doubles_and_caps_at_one_minute() {
        let mut delay = Duration::from_secs(2);
        let mut delays = vec![delay];
        while delay < Duration::from_secs(60) {
            delay = next_retry_delay(delay);
            delays.push(delay);
        }
        assert_eq!(
            delays,
            vec![
                Duration::from_secs(2),
                Duration::from_secs(4),
                Duration::from_secs(8),
                Duration::from_secs(16),
                Duration::from_secs(32),
                Duration::from_secs(60),
            ]
        );
        assert_eq!(
            next_retry_delay(Duration::from_secs(60)),
            Duration::from_secs(60)
        );
    }

    #[test]
    fn retry_wait_exits_immediately_when_cancelled() {
        let cancel = AtomicBool::new(true);
        assert!(wait_for_retry(&cancel, Duration::from_secs(60)));
    }
}

fn retry_until_success<T>(
    cancel: &AtomicBool,
    generation: u64,
    updates: &Sender<TwitchUpdate>,
    source: super::TwitchNetworkSource,
    initial_delay: Duration,
    mut attempt: impl FnMut() -> Result<T, TwitchError>,
) -> Result<T, TwitchError> {
    let mut delay = initial_delay;
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(TwitchError::Cancelled);
        }
        match attempt() {
            Ok(result) => return Ok(result),
            Err(TwitchError::Retryable(message)) => {
                let _ = updates.send(TwitchUpdate::NetworkUnavailable {
                    generation,
                    source,
                    message: format!(
                        "Twitch network unavailable; retrying in {} seconds: {message}",
                        delay.as_secs()
                    ),
                });
                if wait_for_retry(cancel, delay) {
                    return Err(TwitchError::Cancelled);
                }
                delay = next_retry_delay(delay);
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod initial_connection_retry_tests {
    use super::retry_until_success;
    use crate::twitch::{TwitchError, TwitchUpdate};
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn transient_initial_helix_failure_retries_and_keeps_the_authenticated_token() {
        let cancel = AtomicBool::new(false);
        let (updates, received_updates) = mpsc::channel();
        let saved_token = "validated-session-token".to_owned();
        let mut attempts = 0;
        let recovered = retry_until_success(
            &cancel,
            9,
            &updates,
            super::TwitchNetworkSource::Connection,
            Duration::ZERO,
            || {
                attempts += 1;
                if attempts == 1 {
                    Err(TwitchError::Retryable(
                        "Could not load followed Twitch channels".to_owned(),
                    ))
                } else {
                    Ok(saved_token.clone())
                }
            },
        )
        .unwrap();

        assert_eq!(recovered, saved_token);
        assert_eq!(attempts, 2);
        assert!(matches!(
            received_updates.recv().unwrap(),
            TwitchUpdate::NetworkUnavailable {
                generation: 9,
                source: super::TwitchNetworkSource::Connection,
                message,
            } if message.contains("Could not load followed Twitch channels")
        ));
    }
}
