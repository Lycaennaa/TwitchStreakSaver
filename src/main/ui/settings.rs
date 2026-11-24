use super::*;

impl Dashboard {
    pub(super) fn watch_settings(&mut self, ui: &mut egui::Ui) {
        self.twitch_account_settings(ui);
        ui.add_space(8.0);
        self.theme_settings(ui);
        ui.add_space(8.0);
        ui.group(|ui| {
            ui.set_min_width(ui.available_width());
            ui.strong("Privacy");
            ui.checkbox(&mut self.hide_sensitive_info, "Screenshot privacy")
                .on_hover_text(
                    "Hides your Twitch login, streamer names, browser profile names, activity dates and details, searches, channel-selection totals, and variable channel/queue lists. Local browser paths and setup diagnostics remain visible.",
                );
        });
        ui.add_space(8.0);
        let browser_notice = self.browser_notice.clone();
        let detected_browsers = browser_setup::detected_setup_browsers();
        let port_check_in_flight = self.browser_port_check_in_flight;
        let mut connect_browser = false;
        let mut setup_browser = None;
        ui.group(|ui| {
            ui.set_min_width(ui.available_width());
            section_heading(ui, "Browser integration");
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
        let mut adopt_existing_twitch_tabs_changed = false;
        ui.group(|ui| {
            ui.set_min_width(ui.available_width());
            section_heading(ui, "Twitch tab opening");
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
            adopt_existing_twitch_tabs_changed = ui
                .checkbox(
                    &mut self.adopt_existing_twitch_tabs,
                    "Adopt eligible existing Twitch player tabs",
                )
                .changed();
            ui.label("Turning this off releases adopted tabs without closing them.");
        });
        if close_on_raid_redirect_changed || adopt_existing_twitch_tabs_changed {
            self.sync_extension_channels();
        }

        ui.add_space(8.0);
        self.quality_settings(ui);
        ui.add_space(8.0);
        self.automation_settings(ui);
    }
    fn twitch_account_settings(&mut self, ui: &mut egui::Ui) {
        let accounts = self.twitch_accounts.clone();
        let selected_account_id = self.selected_account_id.clone();
        let hide_sensitive_info = self.hide_sensitive_info;
        let connecting = matches!(&self.connection_status, TwitchConnectionStatus::Connecting);
        let removal_pending = self.pending_twitch_account_forget.is_some();
        let mut account_to_select = None;
        let mut account_to_forget: Option<TwitchAccount> = None;
        let mut add_account = false;
        ui.group(|ui| {
            ui.set_min_width(ui.available_width());
            section_heading(ui, "Twitch accounts");
            ui.label("Only the selected account is connected. Each account keeps its own channel selections.");
            ui.label("Forgetting an account removes its saved login and local channel choices.");
            if accounts.is_empty() {
                ui.label("No saved accounts.");
            }
            for (index, account) in accounts.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(if hide_sensitive_info {
                        format!("Account {}", index + 1)
                    } else {
                        account.login.clone()
                    });
                    if selected_account_id.as_deref() == Some(account.id.as_str()) {
                        ui.label("Selected");
                    } else if ui
                        .add_enabled(
                            !connecting && !removal_pending,
                            egui::Button::new("Use"),
                        )
                        .clicked()
                    {
                        account_to_select = Some(account.id.clone());
                    }
                    if ui
                        .add_enabled(
                            !connecting && !removal_pending,
                            egui::Button::new("Forget"),
                        )
                        .clicked()
                    {
                        account_to_forget = Some(account.clone());
                    }
                });
            }
            if ui
                .add_enabled(
                    !connecting && !removal_pending,
                    egui::Button::new("Add account"),
                )
                .clicked()
            {
                add_account = true;
            }
        });
        if let Some(account) = account_to_forget {
            self.pending_twitch_account_forget = Some(account);
        }
        if let Some(account) = self.pending_twitch_account_forget.clone() {
            let account_name = if hide_sensitive_info {
                "this account".to_owned()
            } else {
                account.login.clone()
            };
            let mut confirmed = false;
            let mut cancelled = false;
            egui::Window::new("Forget Twitch account?")
                .id(egui::Id::new("forget_twitch_account_confirmation"))
                .collapsible(false)
                .resizable(false)
                .show(ui.ctx(), |ui| {
                    ui.label(format!(
                        "Remove the saved login and local channel choices for {account_name}?"
                    ));
                    ui.horizontal(|ui| {
                        if ui.button("Forget account").clicked() {
                            confirmed = true;
                        }
                        if ui.button("Cancel").clicked() {
                            cancelled = true;
                        }
                    });
                });
            if confirmed {
                self.pending_twitch_account_forget = None;
                self.forget_twitch_account(&account.id);
            } else if cancelled {
                self.pending_twitch_account_forget = None;
            }
        }
        if let Some(account_id) = account_to_select {
            self.select_twitch_account(account_id);
        } else if add_account {
            self.add_twitch_account();
        }
    }
    fn theme_settings(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        ui.group(|ui| {
            ui.set_min_width(ui.available_width());
            ui.strong("Appearance");
            ui.horizontal(|ui| {
                ui.label("Theme");
                egui::ComboBox::from_id_salt("app_theme")
                    .selected_text(self.theme.label())
                    .show_ui(ui, |ui| {
                        for theme in twitch_streak_saver::model::AppTheme::ALL {
                            changed |= ui
                                .selectable_value(&mut self.theme, theme, theme.label())
                                .changed();
                        }
                    });
            });
            ui.label("Follow system matches your operating system appearance.");
        });
        if changed && self.browser_connected {
            self.set_extension_theme();
        }
    }
    fn quality_settings(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        ui.group(|ui| {
            ui.set_min_width(ui.available_width());
            section_heading(ui, "Stream quality");
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
    fn automation_settings(&mut self, ui: &mut egui::Ui) {
        let mut profile_folder_reset: Option<BrowserKind> = None;
        let mut choose_firefox_profile_folder = false;
        ui.group(|ui| {
            ui.set_min_width(ui.available_width());
            section_heading(ui, "Automation");
            ui.checkbox(&mut self.auto_start_on_launch, "Auto-start on app launch");
            if ui.button("Refresh browser profiles").clicked() {
                self.refresh_browser_profiles();
            }
                ui.menu_button("Browser data folders", |ui| {
                    ui.label("Detected browser user-data folders:");

                    #[cfg(target_os = "macos")]
                    if ui.button("Choose Firefox data folder…").clicked() {
                        choose_firefox_profile_folder = true;
                        ui.close_menu();
                    }
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
            if let Some(error) = &self.browser_profile_folder_error {
                ui.colored_label(
                    egui::Color32::RED,
                    format!("Firefox data folder: {error}"),
                );
            }
            let selected_profile = self
                .browser_launch_target
                .as_ref()
                .map(|target| {
                    self.browser_profiles
                        .iter()
                        .enumerate()
                        .find(|(_, profile)| profile.target == *target)
                        .map(|(index, profile)| {
                            browser_profile_display_name(
                                self.hide_sensitive_info,
                                index,
                                &profile.label,
                            )
                            .into_owned()
                        })
                        .unwrap_or_else(|| {
                            if self.hide_sensitive_info {
                                "Unavailable browser profile".to_owned()
                            } else {
                                format!(
                                    "Unavailable: {} — {}",
                                    target.browser.name(),
                                    target.profile_directory
                                )
                            }
                        })
                })
                .unwrap_or_else(|| "Disabled".to_owned());
            egui::ComboBox::from_id_salt("automatic_browser_profile")
                .selected_text(selected_profile)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.browser_launch_target, None, "Disabled");
                    for (index, profile) in self.browser_profiles.iter().enumerate() {
                        let profile_label = browser_profile_display_name(
                            self.hide_sensitive_info,
                            index,
                            &profile.label,
                        );
                        ui.selectable_value(
                            &mut self.browser_launch_target,
                            Some(profile.target.clone()),
                            profile_label.as_ref(),
                        )
                        .on_hover_text(profile.profile_root.display().to_string());
                    }
                });
            for (index, profile) in self.browser_profiles.iter().enumerate() {
                if let Some(error) = &profile.extension_scan_error {
                    let profile_label = browser_profile_display_name(
                        self.hide_sensitive_info,
                        index,
                        &profile.label,
                    );
                    ui.colored_label(
                        egui::Color32::RED,
                        format!(
                            "Could not inspect extension status for {} at {}: {error}",
                            profile_label.as_ref(),
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
                if let Some(error) = &root.scan_error {
                    ui.colored_label(
                        egui::Color32::RED,
                        format!(
                            "Could not scan {} profiles under {}: {error}",
                            root.browser.name(),
                            root.path.display()
                        ),
                    );
                } else if !self
                    .browser_profiles
                    .iter()
                    .any(|profile| profile.target.browser == root.browser)
                {
                    ui.label(format!(
                        "{}: no profiles found under {}",
                        root.browser.name(),
                        root.path.display()
                    ));
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
            #[cfg(target_os = "macos")]
            {
                let changed = ui
                    .checkbox(&mut self.start_at_macos_login, "Launch at macOS sign-in")
                    .changed();
                if changed {
                    match macos_startup::set_enabled(self.start_at_macos_login) {
                        Ok(()) => self.macos_startup_error = None,
                        Err(error) => {
                            self.start_at_macos_login = !self.start_at_macos_login;
                            self.macos_startup_error = Some(error);
                        }
                    }
                }
                ui.label("The sign-in item starts the app hidden in the menu bar.");
                if let Some(error) = &self.macos_startup_error {
                    ui.colored_label(egui::Color32::RED, error);
                }
            }
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            {
                ui.checkbox(
                    &mut self.close_to_tray_on_close,
                    "Keep running in the menu bar/tray when the window closes",
                );
                ui.label(
                    "Close hides the window only when enabled; minimize behaves normally. Use the menu-bar/tray Quit action to exit while hidden.",
                );
                if let Some(error) = &self.system_tray_error {
                    ui.colored_label(
                        egui::Color32::RED,
                        format!("Menu-bar/tray controls are unavailable: {error}"),
                    );
                }
            }
            ui.label("Twitch and browser connections are required for automatic opening.");
        });

        #[cfg(target_os = "macos")]
        if choose_firefox_profile_folder {
            self.apply_firefox_profile_data_directory_selection(
                browser_setup::choose_firefox_profile_data_directory(),
            );
        }
        if let Some(browser) = profile_folder_reset {
            self.reset_browser_profile_data_directory(browser);
        }
    }
}
