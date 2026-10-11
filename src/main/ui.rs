use super::*;

impl eframe::App for Dashboard {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, PREFERENCES_KEY, &self.preferences());
        eframe::set_value(storage, EXTENSION_ID_KEY, &self.extension_id);
    }

    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_twitch_updates();
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

        egui::TopBottomPanel::top("app_header").show(context, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.heading("Twitch Streak Saver");
                    ui.label("A quieter way to manage the channels you follow");
                });
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
                            self.connection_generation = self.connection_generation.wrapping_add(1);
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
            ui.add_space(8.0);
            ui.heading("Your watch list");
            ui.label("Choose Streaks, Channel Points, or both for each followed channel.");
            ui.add_space(12.0);

            ui.columns(2, |columns| {
                self.channel_list(&mut columns[0]);
                self.watch_settings(&mut columns[1]);
            });

            ui.add_space(8.0);
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new(
                        "Selected live Streaks run first. Selected live Points streams open automatically after the Streak queue; a turn needs 16 eligible minutes with playing video, ready chat UI, and an available app slot.",
                    )
                    .weak(),
                );
            });
            if let Some(error) = &self.connection_error {
                ui.colored_label(egui::Color32::LIGHT_RED, format!("Twitch: {error}"));
            }
        });
        if let Some(notice) = self.streak_notices.front().cloned() {
            let mut is_open = true;
            let mut dismiss = false;
            egui::Window::new("Watch Streak detected")
                .id(egui::Id::new("watch_streak_event_dialog"))
                .order(egui::Order::Foreground)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .collapsible(false)
                .resizable(false)
                .open(&mut is_open)
                .show(context, |ui| {
                    ui.heading("Streak reward detected");
                    ui.label(format!("{} completed a Watch Streak.", notice.channel_name));
                    ui.label(format!("Reward: {} points", notice.points_earned));
                    ui.add_space(6.0);
                    dismiss = ui.button("Dismiss").clicked();
                });
            if dismiss || !is_open {
                self.streak_notices.pop_front();
            }
        } else if let Some(notice) = self.points_notices.front().cloned() {
            let (title, message) = match notice {
                PointsNotice::TurnCompleted { channel_name } => (
                    "Points rotation complete",
                    format!(
                        "{channel_name} completed an eligible 16-minute Points turn. The Points queue has advanced."
                    ),
                ),
                PointsNotice::Earned {
                    channel_name,
                    points,
                    reason_code,
                    balance,
                } => (
                    "Channel Points earned",
                    format!(
                        "{channel_name} earned {points} points ({reason_code}). New balance: {balance}."
                    ),
                ),
                PointsNotice::Spent {
                    channel_name,
                    balance,
                } => (
                    "Channel Points spent",
                    format!("{channel_name} points balance is now {balance}."),
                ),
                PointsNotice::ClaimAvailable { channel_name } => (
                    "Channel Points bonus available",
                    format!("A bonus is ready to claim on {channel_name}. Claim it manually on Twitch."),
                ),
            };
            let mut is_open = true;
            let mut dismiss = false;
            egui::Window::new(title)
                .id(egui::Id::new("points_event_dialog"))
                .order(egui::Order::Foreground)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .collapsible(false)
                .resizable(false)
                .open(&mut is_open)
                .show(context, |ui| {
                    ui.heading(title);
                    ui.label(message);
                    ui.add_space(6.0);
                    dismiss = ui.button("Dismiss").clicked();
                });
            if dismiss || !is_open {
                self.points_notices.pop_front();
            }
        }
    }
}

impl Dashboard {
    fn watch_settings(&mut self, ui: &mut egui::Ui) {
        ui.group(|ui| {
            ui.strong("Watch strategy");
            ui.add_space(6.0);

            ui.selectable_value(&mut self.mode, SchedulingMode::Earn, "Earn");
            ui.label("Prioritize streaks and keep to two app-managed streams.");
            ui.add_space(6.0);

            ui.selectable_value(&mut self.mode, SchedulingMode::OpenAll, "Open All");
            ui.label(
                "Keep up to four app-managed streams open. More than two may not earn progress.",
            );
            ui.label(format!(
                "App-managed stream limit: {}",
                self.mode.app_stream_limit()
            ));
        });

        let browser_connected = self.browser_connected;
        let browser_notice = self.browser_notice.clone();
        let mut install_native_host = false;
        ui.add_space(8.0);
        ui.group(|ui| {
            ui.strong("Chrome / Brave integration");
            ui.label(if browser_connected {
                "Native host connected."
            } else {
                "Load chrome-extension/ unpacked in Chrome or Brave, then install its native host."
            });
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.extension_id)
                        .hint_text("Browser extension ID"),
                );
                install_native_host = ui.button("Install host").clicked();
            });
            if let Some(notice) = &browser_notice {
                ui.label(egui::RichText::new(notice).small().weak());
            }
        });
        if install_native_host {
            self.install_chrome_native_host();
        }
        let previous_layout = self.tab_layout;
        ui.group(|ui| {
            ui.strong("Twitch tab opening");
            ui.horizontal_wrapped(|ui| {
                ui.label("Open streams as:");
                ui.selectable_value(
                    &mut self.tab_layout,
                    TwitchTabLayout::SeparatePlayerChat,
                    "Player + chat tabs",
                );
                ui.selectable_value(
                    &mut self.tab_layout,
                    TwitchTabLayout::RegularWebsite,
                    "Regular Twitch tab",
                );
            });
            ui.checkbox(
                &mut self.wake_browser_on_open,
                "Restore and focus browser when opening",
            );
            ui.label(
                "Waking restores a minimized window, focuses each stream tab, and waits for it to load. It may interrupt other work.",
            );
        });
        if previous_layout != self.tab_layout {
            self.apply_tab_layout_to_managed_tabs();
        }

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
}
impl Dashboard {
    fn automation_settings(&mut self, ui: &mut egui::Ui) {
        let mut start = false;
        let mut stop = false;
        ui.group(|ui| {
            ui.strong("Automation");
            ui.label(if self.automation_enabled {
                "Automation enabled."
            } else {
                "Automation stopped."
            });
            ui.checkbox(&mut self.auto_start_on_launch, "Auto-start on app launch");
            ui.label("Twitch and browser connections are required for automatic opening.");
            ui.horizontal(|ui| {
                start = ui
                    .add_enabled(!self.automation_enabled, egui::Button::new("Start"))
                    .clicked();
                stop = ui
                    .add_enabled(self.automation_enabled, egui::Button::new("Stop All"))
                    .clicked();
            });
            ui.label(
                egui::RichText::new(
                    "Stop All closes tabs opened by this app. External browser tabs are unaffected.",
                )
                .small()
                .weak(),
            );
        });
        if start {
            self.start_automation();
        }
        if stop {
            self.stop_automation();
        }
    }
}
