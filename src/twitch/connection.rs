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
    let (token, user) = match obtain_token(&client, &cancel) {
        Ok(result) => result,
        Err(TwitchError::Cancelled) => return,
        Err(error) => {
            send_failed(&updates, generation, error.message());
            return;
        }
    };
    if cancel.load(Ordering::Acquire) {
        return;
    }

    let mut channels = match fetch_followed_channels(&client, &token, &user, &cancel) {
        Ok(channels) => channels,
        Err(TwitchError::Cancelled) => return,
        Err(error) => {
            send_failed(&updates, generation, error.message());
            return;
        }
    };
    let mut live_channel_ids = match fetch_live_channel_ids(&client, &token, &channels, &cancel) {
        Ok(live) => live,
        Err(TwitchError::Cancelled) => return,
        Err(error) => {
            send_failed(&updates, generation, error.message());
            return;
        }
    };
    if cancel.load(Ordering::Acquire) {
        return;
    }
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
    while !cancel.load(Ordering::Acquire) {
        thread::sleep(Duration::from_millis(250));
        if cancel.load(Ordering::Acquire) {
            break;
        }

        if last_live_refresh.elapsed() >= LIVE_REFRESH_INTERVAL {
            match fetch_live_channel_ids(&client, &token, &channels, &cancel) {
                Ok(live) => {
                    live_channel_ids = live;
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
                Err(error) => {
                    let _ = updates.send(TwitchUpdate::Warning {
                        generation,
                        message: error.message(),
                    });
                }
            }
            last_live_refresh = Instant::now();
        }

        if last_follow_refresh.elapsed() >= FOLLOW_REFRESH_INTERVAL {
            match fetch_followed_channels(&client, &token, &user, &cancel) {
                Ok(refreshed_channels) => {
                    match fetch_live_channel_ids(&client, &token, &refreshed_channels, &cancel) {
                        Ok(live) => {
                            channels = refreshed_channels;
                            live_channel_ids = live;
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
                        Err(error) => {
                            let _ = updates.send(TwitchUpdate::Warning {
                                generation,
                                message: error.message(),
                            });
                        }
                    }
                }
                Err(TwitchError::Cancelled) => break,
                Err(error @ TwitchError::Reauthorize(_)) => {
                    fail_expired_session(&updates, generation, error);
                    break;
                }
                Err(error) => {
                    let _ = updates.send(TwitchUpdate::Warning {
                        generation,
                        message: error.message(),
                    });
                }
            }
            last_follow_refresh = Instant::now();
            last_live_refresh = Instant::now();
        }
    }
    pubsub_cancel.store(true, Ordering::Release);
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
