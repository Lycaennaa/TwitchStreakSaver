use super::*;
impl Dashboard {
    pub(crate) fn show_dashboard_content(&mut self, ui: &mut egui::Ui, active_tab: DashboardTab) {
        if let Some(error) = &self.connection_error {
            ui.colored_label(egui::Color32::LIGHT_RED, format!("Twitch: {error}"));
        }
        if let Some(error) = &self.browser_error {
            ui.colored_label(egui::Color32::LIGHT_RED, format!("Browser: {error}"));
        }

        match active_tab {
            DashboardTab::Queue => {
                let _ = self.show_dashboard_content_with_bounds(ui);
            }
            DashboardTab::FollowedChannels => self.show_followed_channels_page(ui),
            DashboardTab::Activity => {
                ui.add_space(8.0);
                Self::activity_log_panel(
                    ui,
                    self.hide_sensitive_info,
                    &self.activity_log,
                    &mut self.activity_log_kind_filter,
                    &mut self.activity_log_search,
                );
            }
            DashboardTab::Settings => {
                ui.add_space(8.0);
                self.watch_settings(ui);
            }
        }
    }
    pub(crate) fn show_dashboard_content_with_bounds(
        &mut self,
        ui: &mut egui::Ui,
    ) -> (egui::Rect, egui::Rect, egui::Rect, egui::Rect) {
        ui.add_space(8.0);
        let queue_rect = self.show_queue_visibility_card(ui);
        ui.add_space(8.0);
        let automation_rect = self.automation_quick_controls(ui);
        ui.add_space(8.0);
        let summary_rect = self.selection_summary(ui);
        let page_rect = ui.min_rect();
        (queue_rect, automation_rect, summary_rect, page_rect)
    }

    fn show_followed_channels_page(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        let list_bottom_y = ui.clip_rect().max.y;
        self.channel_list(ui, list_bottom_y);
    }

    fn automation_quick_controls(&mut self, ui: &mut egui::Ui) -> egui::Rect {
        let automation_enabled = self.automation_enabled;
        let now = self.started_at.elapsed();
        let mut start = false;
        let mut stop = false;
        let response = ui.group(|ui| {
            ui.set_min_width(ui.available_width());
            section_heading(ui, "Automation");
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(if automation_enabled {
                        "Running"
                    } else {
                        "Stopped"
                    })
                    .strong()
                    .color(if automation_enabled {
                        egui::Color32::from_rgb(80, 210, 140)
                    } else {
                        egui::Color32::YELLOW
                    }),
                );
                draw_automation_pause_controls(
                    ui,
                    &mut self.automation_delays,
                    now,
                    automation_enabled,
                );
                if automation_enabled {
                    stop = ui.button("Stop All").clicked();
                } else {
                    start = ui
                        .add(egui::Button::new("Start").fill(egui::Color32::from_rgb(145, 70, 255)))
                        .clicked();
                }
            });
        });
        if start {
            self.start_automation();
        }
        if stop {
            self.stop_automation();
        }
        response.response.rect
    }

    fn selection_summary(&self, ui: &mut egui::Ui) -> egui::Rect {
        let selection_counts = channel_selection_counts(&self.channels, self.hide_sensitive_info);
        let response = ui.group(|ui| {
            ui.set_min_width(ui.available_width());
            section_heading(ui, "Selection summary");
            if let Some((selected_channels, selected_streaks, selected_points)) = selection_counts {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new(format!("{selected_channels}"))
                                .size(22.0)
                                .strong()
                                .color(egui::Color32::from_rgb(145, 70, 255)),
                        );
                        ui.label(egui::RichText::new("Selected channels").small().weak());
                    });
                    ui.separator();
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new(format!("{selected_streaks}"))
                                .size(22.0)
                                .strong()
                                .color(egui::Color32::from_rgb(80, 210, 140)),
                        );
                        ui.label(egui::RichText::new("Streak channels").small().weak());
                    });
                    ui.separator();
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new(format!("{selected_points}"))
                                .size(22.0)
                                .strong()
                                .color(egui::Color32::from_rgb(192, 151, 255)),
                        );
                        ui.label(egui::RichText::new("Points channels").small().weak());
                    });
                });
            } else {
                ui.label("Channel counts hidden");
            }
        });
        response.response.rect
    }
}
