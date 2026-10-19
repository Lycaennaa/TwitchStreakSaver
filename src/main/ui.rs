use super::*;

impl eframe::App for Dashboard {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, PREFERENCES_KEY, &self.preferences());
    }

    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_twitch_updates();
        self.poll_browser_port_check();
        self.poll_browser_bridge();
        let now = self.started_at.elapsed();
        let poll_interval = Duration::from_secs(1);
        self.reconcile_automatic_tabs(now);
        context.request_repaint_after(poll_interval);
        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = egui::Color32::BLACK;
        visuals.window_fill = egui::Color32::BLACK;
        visuals.faint_bg_color = egui::Color32::BLACK;
        visuals.extreme_bg_color = egui::Color32::BLACK;
        visuals.code_bg_color = egui::Color32::BLACK;
        for widget in [
            &mut visuals.widgets.noninteractive,
            &mut visuals.widgets.inactive,
            &mut visuals.widgets.hovered,
            &mut visuals.widgets.active,
            &mut visuals.widgets.open,
        ] {
            widget.bg_fill = egui::Color32::BLACK;
            widget.weak_bg_fill = egui::Color32::BLACK;
        }
        context.set_visuals(visuals);

        egui::TopBottomPanel::top("app_header")
            .show_separator_line(false)
            .show(context, |ui| {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.heading("Twitch Streak Saver");
                    let status = match &self.connection_status {
                        TwitchConnectionStatus::Disconnected => "NOT CONNECTED".to_owned(),
                        TwitchConnectionStatus::Connecting => "CONNECTING TO TWITCH…".to_owned(),
                        TwitchConnectionStatus::Connected(login) => format!("CONNECTED · {login}"),
                    };
                    let connecting =
                        matches!(&self.connection_status, TwitchConnectionStatus::Connecting);
                    let connected = matches!(
                        &self.connection_status,
                        TwitchConnectionStatus::Connected(_)
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(egui::RichText::new(status).strong());
                        if connecting {
                            if ui.button("Cancel").clicked() {
                                if let Some(cancel) = self.twitch_cancel.take() {
                                    cancel.store(true, Ordering::Release);
                                }
                                self.connection_generation =
                                    self.connection_generation.wrapping_add(1);
                                self.connection_status = TwitchConnectionStatus::Disconnected;
                            }
                        } else if connected {
                            if ui.button("Disconnect").clicked() {
                                self.disconnect_twitch();
                            }
                        } else if ui.button("Connect Twitch").clicked() {
                            self.start_twitch_connection();
                        }
                    });
                });
                ui.add_space(8.0);
            });

        egui::CentralPanel::default().show(context, |ui| {
            let content_max_height = dashboard_content_max_height(ui.available_height());
            egui::ScrollArea::vertical()
                .max_height(content_max_height)
                .show(ui, |ui| {
                    ui.add_space(8.0);
                    ui.columns(2, |columns| {
                        self.channel_list(&mut columns[0]);
                        self.watch_settings(&mut columns[1]);
                    });

                    if let Some(error) = &self.connection_error {
                        ui.colored_label(egui::Color32::LIGHT_RED, format!("Twitch: {error}"));
                    }
                });
            ui.add_space(8.0);
            Self::activity_log_panel(
                ui,
                &self.activity_log,
                &mut self.activity_log_kind_filter,
                &mut self.activity_log_search,
            );
        });
    }
}

const MIN_ACTIVITY_LOG_HEIGHT: f32 = 48.0;
const ACTIVITY_LOG_HEIGHT_FRACTION: f32 = 0.3;

fn dashboard_content_max_height(available_height: f32) -> f32 {
    let reserved_log_height =
        (available_height * ACTIVITY_LOG_HEIGHT_FRACTION).max(MIN_ACTIVITY_LOG_HEIGHT);
    (available_height - reserved_log_height).max(0.0)
}

fn activity_log_matches(
    entry: &ActivityLogEntry,
    event_filter: Option<ActivityLogKind>,
    search: &str,
) -> bool {
    let query = search.trim();
    event_filter.map_or(true, |kind| entry.kind == kind)
        && (query.is_empty()
            || fuzzy_contains(&entry.time, query)
            || fuzzy_contains(&entry.streamer, query)
            || fuzzy_contains(&entry.event, query)
            || fuzzy_contains(entry.kind.label(), query))
}

impl Dashboard {
    fn activity_log_panel(
        ui: &mut egui::Ui,
        activity_log: &std::collections::VecDeque<ActivityLogEntry>,
        event_filter: &mut Option<ActivityLogKind>,
        search: &mut String,
    ) -> egui::Response {
        ui.group(|ui| {
            ui.horizontal_wrapped(|ui| {
                ui.heading("Activity log");
                ui.add(
                    egui::TextEdit::singleline(search)
                        .hint_text("Search event, streamer, or time")
                        .desired_width(190.0),
                );
                egui::ComboBox::from_id_salt("activity_log_event_filter")
                    .selected_text((*event_filter).map_or("All events", ActivityLogKind::label))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(event_filter, None, "All events");
                        for kind in ActivityLogKind::ALL {
                            ui.selectable_value(event_filter, Some(kind), kind.label());
                        }
                    });
            });
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if activity_log.is_empty() {
                        ui.label("Point, Streak, and stream-switch events will appear here.");
                        return;
                    }
                    let mut has_matching_entries = false;
                    egui::Grid::new("activity_log_entries")
                        .num_columns(3)
                        .striped(true)
                        .show(ui, |ui| {
                            ui.strong("Time");
                            ui.strong("Streamer");
                            ui.strong("Event");
                            ui.end_row();
                            for entry in activity_log
                                .iter()
                                .filter(|entry| activity_log_matches(entry, *event_filter, search))
                            {
                                has_matching_entries = true;
                                ui.label(egui::RichText::new(&entry.time).monospace());
                                ui.label(&entry.streamer);
                                ui.label(&entry.event);
                                ui.end_row();
                            }
                        });
                    if !has_matching_entries {
                        ui.label("No activity log entries match these filters.");
                    }
                });
        })
        .response
    }

    fn watch_settings(&mut self, ui: &mut egui::Ui) {
        let browser_notice = self.browser_notice.clone();
        let detected_browsers = browser_setup::detected_setup_browsers();
        let port_check_in_flight = self.browser_port_check_in_flight;
        let mut connect_browser = false;
        let mut setup_browser = None;
        ui.group(|ui| {
            ui.strong("Browser integration");
            ui.label(if self.browser_connected {
                "Native host connected."
            } else {
                "Pairing is automatic. Click the extension toolbar icon to wake and retry its worker."
            });
            let button_label = if port_check_in_flight {
                "Checking browser connection…"
            } else {
                "Check app bridge"
            };
            if ui
                .add_enabled(!port_check_in_flight, egui::Button::new(button_label))
                .clicked()
            {
                connect_browser = true;
            }
            ui.menu_button("Set up browser extension", |ui| {
                if detected_browsers.is_empty() {
                    ui.label("No supported browsers detected.");
                }
                for browser in detected_browsers.iter().copied() {
                    if ui.button(browser.name()).clicked() {
                        setup_browser = Some(browser);
                        ui.close_menu();
                    }
                }
            });
            if let Some(notice) = &browser_notice {
                ui.label(egui::RichText::new(notice).small().weak());
            }
        });
        if connect_browser {
            self.connect_to_browser();
        }
        if let Some(target) = setup_browser {
            self.open_extension_setup(target);
        }
        let mut close_on_raid_redirect_changed = false;
        ui.group(|ui| {
            ui.strong("Twitch tab opening");
            ui.label(
                "Each stream opens as a combined Twitch page in its own window.",
            );
            ui.checkbox(
                &mut self.wake_browser_on_open,
                "Restore and focus browser when opening",
            );
            ui.label(
                "Enabling this brings each stream window to the foreground as it opens and may interrupt other work. Twitch and the browser can still throttle or freeze background pages.",
            );
            close_on_raid_redirect_changed = ui
                .checkbox(
                    &mut self.close_on_raid_redirect,
                    "Close app-opened stream tabs on Twitch raid redirects",
                )
                .changed();
            ui.label("Closes a managed stream when its URL includes referrer=raid.");
        });
        if close_on_raid_redirect_changed {
            self.sync_extension_channels();
        }

        ui.add_space(8.0);
        self.quality_settings(ui);
        ui.add_space(8.0);
        self.automation_settings(ui);

        let selected_streaks = self
            .channels
            .iter()
            .filter(|channel| channel.selection.streak)
            .count();
        let selected_points = self
            .channels
            .iter()
            .filter(|channel| channel.selection.points)
            .count();
        let selected_channels = self
            .channels
            .iter()
            .filter(|channel| channel.selection.is_selected())
            .count();

        ui.add_space(8.0);
        ui.group(|ui| {
            ui.strong("Selection summary");
            ui.label(format!("{selected_channels} selected channels"));
            ui.label(format!("{selected_streaks} Streak channels"));
            ui.label(format!("{selected_points} Points channels"));
        });
    }
    fn quality_settings(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        ui.group(|ui| {
            ui.strong("Stream quality");
            ui.label("Automatically choose this quality on Twitch players.");
            egui::ComboBox::from_id_salt("preferred_stream_quality")
                .selected_text(self.preferred_quality.label())
                .show_ui(ui, |ui| {
                    for quality in PreferredQuality::ALL {
                        changed |= ui
                            .selectable_value(&mut self.preferred_quality, quality, quality.label())
                            .changed();
                    }
                });
            ui.label("Unavailable choices use the lowest available video quality.");
        });
        if changed && self.browser_connected {
            self.set_extension_preferred_quality();
        }
    }
}
impl Dashboard {
    fn automation_settings(&mut self, ui: &mut egui::Ui) {
        let mut start = false;
        let mut stop = false;
        let mut profile_folder_reset: Option<BrowserKind> = None;
        ui.group(|ui| {
            ui.strong("Automation");
            ui.label(if self.automation_enabled {
                "Automation enabled."
            } else {
                "Automation stopped."
            });
            ui.checkbox(&mut self.auto_start_on_launch, "Auto-start on app launch");
            if ui.button("Refresh browser profiles").clicked() {
                self.refresh_browser_profiles();
            }
                ui.menu_button("Browser data folders", |ui| {
                    ui.label("Detected browser user-data folders:");
                    for root in &self.browser_profile_roots {
                        ui.horizontal(|ui| {
                            ui.label(root.browser.name());
                            ui.label(root.path.display().to_string());
                            if root.overridden && ui.button("Reset").clicked() {
                                profile_folder_reset = Some(root.browser);
                                ui.close_menu();
                            }
                        });
                    }
                });
            let selected_profile = self
                .browser_launch_target
                .as_ref()
                .map(|target| {
                    self.browser_profiles
                        .iter()
                        .find(|profile| profile.target == *target)
                        .map(|profile| profile.label.clone())
                        .unwrap_or_else(|| {
                            format!(
                                "Unavailable: {} — {}",
                                target.browser.name(),
                                target.profile_directory
                            )
                        })
                })
                .unwrap_or_else(|| "Disabled".to_owned());
            egui::ComboBox::from_id_salt("automatic_browser_profile")
                .selected_text(selected_profile)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.browser_launch_target, None, "Disabled");
                    for profile in &self.browser_profiles {
                        ui.selectable_value(
                            &mut self.browser_launch_target,
                            Some(profile.target.clone()),
                            &profile.label,
                        )
                        .on_hover_text(profile.profile_root.display().to_string());
                    }
                });
            for profile in &self.browser_profiles {
                if let Some(error) = &profile.extension_scan_error {
                    ui.colored_label(
                        egui::Color32::RED,
                        format!(
                            "Could not inspect extension status for {} at {}: {error}",
                            profile.label,
                            profile
                                .profile_root
                                .join(&profile.target.profile_directory)
                                .join("Preferences")
                                .display()
                        ),
                    );
                }
            }
            if self.browser_profiles.is_empty() {
                ui.label("No profiles were found in the readable browser data folders.");
            }
            for root in &self.browser_profile_roots {
                if !self
                    .browser_profiles
                    .iter()
                    .any(|profile| profile.target.browser == root.browser)
                {
                    let message = match &root.scan_error {
                        Some(error) => format!(
                            "{} profile scan failed at {}: {error}",
                            root.browser.name(),
                            root.path.display()
                        ),
                        None => format!(
                            "{}: no profiles found under {}",
                            root.browser.name(),
                            root.path.display()
                        ),
                    };
                    ui.label(message);
                }
            }
            ui.label(
                "Hover a profile for its detected folder. Profiles with the extension enabled are marked; if multiple are enabled, the bridge cannot identify the connected one.",
            );
            #[cfg(target_os = "windows")]
            {
                let changed = ui
                    .checkbox(&mut self.start_with_windows, "Launch at Windows sign-in")
                    .changed();
                if changed {
                    match windows_startup::set_enabled(self.start_with_windows) {
                        Ok(()) => self.windows_startup_error = None,
                        Err(error) => {
                            self.start_with_windows = !self.start_with_windows;
                            self.windows_startup_error = Some(error);
                        }
                    }
                }
                ui.label("Applies to the current Windows user.");
                if let Some(error) = &self.windows_startup_error {
                    ui.colored_label(egui::Color32::RED, error);
                }
            }
            ui.label("Twitch and browser connections are required for automatic opening.");
            ui.horizontal(|ui| {
                start = ui
                    .add_enabled(!self.automation_enabled, egui::Button::new("Start"))
                    .clicked();
                stop = ui
                    .add_enabled(self.automation_enabled, egui::Button::new("Stop All"))
                    .clicked();
            });
        });
        if let Some(browser) = profile_folder_reset {
            if let Err(error) = self.set_browser_profile_data_directory(browser, None) {
                self.browser_error = Some(error);
            }
        }
        if start {
            self.start_automation();
        }
        if stop {
            self.stop_automation();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_log_filters_by_kind_and_fuzzy_text_across_fields() {
        let entry = ActivityLogEntry {
            time: "21:15:09".to_owned(),
            streamer: "Streamer One".to_owned(),
            event: "Watch Streak completed; reward: 400 points.".to_owned(),
            kind: ActivityLogKind::WatchStreak,
        };

        assert!(activity_log_matches(
            &entry,
            Some(ActivityLogKind::WatchStreak),
            "wstrk"
        ));
        assert!(activity_log_matches(&entry, None, "strmr"));
        assert!(activity_log_matches(&entry, None, "211509"));
        let points_entry = ActivityLogEntry {
            time: "21:15:10".to_owned(),
            streamer: "Streamer Two".to_owned(),
            event: "Earned 400 points (CLAIM); new balance: 400.".to_owned(),
            kind: ActivityLogKind::PointsEarned,
        };
        assert!(activity_log_matches(&points_entry, None, "points earned"));
        assert!(!activity_log_matches(
            &entry,
            Some(ActivityLogKind::PointsSpent),
            ""
        ));
        assert!(!activity_log_matches(
            &entry,
            Some(ActivityLogKind::WatchStreak),
            "missing"
        ));
    }

    #[test]
    fn empty_activity_log_fills_the_reserved_bottom_region() {
        for screen_size in [egui::vec2(1471.0, 274.0), egui::vec2(800.0, 600.0)] {
            let context = egui::Context::default();
            let mut panel_response = None;
            let mut minimum_panel_height = 0.0;
            let mut event_filter = None;
            let mut search = String::new();
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, screen_size)),
                ..Default::default()
            };

            let _ = context.run(input, |context| {
                egui::CentralPanel::default().show(context, |ui| {
                    let available_height = ui.available_height();
                    let content_max_height = dashboard_content_max_height(available_height);
                    minimum_panel_height = (available_height - content_max_height - 16.0).max(0.0);
                    egui::ScrollArea::vertical()
                        .max_height(content_max_height)
                        .show(ui, |ui| {
                            ui.add_space(content_max_height + 200.0);
                        });
                    ui.add_space(8.0);
                    panel_response = Some(Dashboard::activity_log_panel(
                        ui,
                        &std::collections::VecDeque::new(),
                        &mut event_filter,
                        &mut search,
                    ));
                });
            });

            let panel_response = panel_response.expect("activity log panel was not rendered");
            assert!(
                panel_response.rect.height() >= minimum_panel_height,
                "activity log only used {} points instead of the reserved {}",
                panel_response.rect.height(),
                minimum_panel_height
            );
            assert!(
                panel_response.rect.bottom() >= screen_size.y - 15.0,
                "activity log ended at {} in a {}-point viewport",
                panel_response.rect.bottom(),
                screen_size.y
            );
        }
    }
}
