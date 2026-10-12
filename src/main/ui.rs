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
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_space(8.0);
                ui.heading("Your watch list");
                ui.label("Choose Streaks, Channel Points, or both for each followed channel.");
                ui.add_space(12.0);

                ui.columns(2, |columns| {
                    self.channel_list(&mut columns[0]);
                    self.watch_settings(&mut columns[1]);
                });

                ui.add_space(8.0);
                self.activity_log_panel(ui);
                if let Some(error) = &self.connection_error {
                    ui.colored_label(egui::Color32::LIGHT_RED, format!("Twitch: {error}"));
                }
            });
        });
    }
}

impl Dashboard {
    fn activity_log_panel(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Activity log");
            ui.label(
                egui::RichText::new("Local time · newest first")
                    .small()
                    .weak(),
            );
        });
        if self.activity_log.is_empty() {
            ui.label("Point and Streak events will appear here.");
            return;
        }
        egui::ScrollArea::vertical()
            .max_height(76.0)
            .show(ui, |ui| {
                egui::Grid::new("activity_log_entries")
                    .num_columns(3)
                    .striped(true)
                    .show(ui, |ui| {
                        ui.strong("Time");
                        ui.strong("Streamer");
                        ui.strong("Event");
                        ui.end_row();
                        for entry in &self.activity_log {
                            ui.label(egui::RichText::new(&entry.time).monospace());
                            ui.label(&entry.streamer);
                            ui.label(&entry.event);
                            ui.end_row();
                        }
                    });
            });
    }

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
        ui.group(|ui| {
            ui.strong("Twitch tab opening");
            ui.label(
                "Each stream opens as a combined Twitch page in its own window. Its tab is selected without focusing the window.",
            );
            ui.checkbox(
                &mut self.wake_browser_on_open,
                "Restore and focus browser when opening",
            );
            ui.label(
                "Enabling this brings each stream window to the foreground as it opens and may interrupt other work. Twitch and the browser can still throttle or freeze background pages.",
            );
        });

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
