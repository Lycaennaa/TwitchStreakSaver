#[path = "ui/activity.rs"]
mod activity;
use activity::activity_log_matches;
#[path = "ui/pages.rs"]
mod pages;
#[path = "ui/privacy.rs"]
mod privacy;
#[path = "ui/settings.rs"]
mod settings;
use super::*;
use privacy::{
    activity_log_display_values, browser_profile_display_name, channel_selection_counts,
    daily_activity_summary_contents,
};
pub(super) fn section_heading(ui: &mut egui::Ui, title: &str) {
    ui.label(
        egui::RichText::new(title)
            .strong()
            .color(egui::Color32::from_rgb(185, 120, 255)),
    );
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DashboardTab {
    Queue,
    FollowedChannels,
    Activity,
    Settings,
}

impl Default for DashboardTab {
    fn default() -> Self {
        Self::Queue
    }
}

impl DashboardTab {
    const ALL: [Self; 4] = [
        Self::Queue,
        Self::FollowedChannels,
        Self::Activity,
        Self::Settings,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Queue => "Watch queue",
            Self::FollowedChannels => "Followed channels",
            Self::Activity => "Activity",
            Self::Settings => "Settings",
        }
    }

    fn subtitle(self) -> &'static str {
        match self {
            Self::Queue => "LIVE CHANNELS & STREAM AUTOMATION",
            Self::FollowedChannels => "FOLLOWED CHANNELS & SELECTION",
            Self::Activity => "RECENT EVENTS & STREAM HISTORY",
            Self::Settings => "PREFERENCES & BROWSER SETUP",
        }
    }
}

const DASHBOARD_TAB_ID: &str = "primary_navigation.active_tab";

pub(super) fn sidebar_tabs(ui: &mut egui::Ui, mut active_tab: DashboardTab) -> DashboardTab {
    for tab in DashboardTab::ALL {
        let _ = sidebar_tab(ui, &mut active_tab, tab);
        ui.add_space(5.0);
    }
    active_tab
}

pub(super) fn sidebar_tab(
    ui: &mut egui::Ui,
    active_tab: &mut DashboardTab,
    tab: DashboardTab,
) -> egui::Response {
    let selected = tab == *active_tab;
    let text_color = if selected {
        egui::Color32::from_rgb(210, 180, 255)
    } else {
        egui::Color32::from_rgb(190, 190, 198)
    };
    let response = ui.add_sized(
        egui::vec2(ui.available_width(), 44.0),
        egui::Button::new(egui::RichText::new(tab.label()).color(text_color).strong())
            .fill(egui::Color32::BLACK)
            .stroke(egui::Stroke::NONE),
    );
    if response.clicked() {
        *active_tab = tab;
    }
    response
}

impl eframe::App for Dashboard {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, PREFERENCES_KEY, &self.preferences());
        eframe::set_value(storage, super::ACTIVITY_LOG_KEY, &self.activity_log);
    }

    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            if self.background_launch {
                if self.system_tray.is_none() {
                    context.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                }
                self.background_launch = false;
            }

            let mut open_requested = false;
            let mut quit_requested = false;
            if let Some(tray) = &self.system_tray {
                tray.drain_events(|action| match action {
                    super::system_tray::TrayAction::Open => open_requested = true,
                    super::system_tray::TrayAction::Quit => quit_requested = true,
                });
            }
            if quit_requested {
                self.exit_requested = true;
                context.send_viewport_cmd(egui::ViewportCommand::Close);
            } else if open_requested {
                self.exit_requested = false;
                context.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                context.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                context.send_viewport_cmd(egui::ViewportCommand::Focus);
            }

            if context.input(|input| input.viewport().close_requested())
                && super::system_tray::should_hide_on_close(
                    self.close_to_tray_on_close,
                    self.system_tray.is_some(),
                    self.exit_requested,
                )
            {
                context.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                context.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            }
        }
        self.poll_twitch_updates();
        self.poll_browser_port_check();
        self.poll_browser_bridge();
        self.service_network_recovery();
        self.service_pending_manual_open();
        let now = self.started_at.elapsed();
        let poll_interval = Duration::from_secs(1);
        self.reconcile_automatic_tabs(now);
        context.request_repaint_after(poll_interval);
        let mut visuals = egui::Visuals::dark();
        let text = egui::Color32::from_rgb(235, 235, 235);
        visuals.panel_fill = egui::Color32::BLACK;
        visuals.window_fill = egui::Color32::BLACK;
        visuals.faint_bg_color = egui::Color32::BLACK;
        visuals.extreme_bg_color = egui::Color32::BLACK;
        visuals.code_bg_color = egui::Color32::BLACK;
        visuals.hyperlink_color = egui::Color32::from_rgb(192, 151, 255);
        visuals.selection.bg_fill = egui::Color32::from_rgb(82, 44, 128);
        visuals.selection.stroke =
            egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(205, 173, 255));
        visuals.window_stroke = egui::Stroke::NONE;
        visuals.widgets.noninteractive.bg_fill = egui::Color32::BLACK;
        visuals.widgets.noninteractive.weak_bg_fill = egui::Color32::BLACK;
        visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0_f32, text);
        visuals.widgets.noninteractive.bg_stroke = egui::Stroke::NONE;
        visuals.widgets.inactive.bg_fill = egui::Color32::BLACK;
        visuals.widgets.inactive.weak_bg_fill = egui::Color32::BLACK;
        visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0_f32, text);
        visuals.widgets.inactive.bg_stroke = egui::Stroke::NONE;
        visuals.widgets.hovered.bg_fill = egui::Color32::BLACK;
        visuals.widgets.hovered.weak_bg_fill = egui::Color32::BLACK;
        visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0_f32, egui::Color32::WHITE);
        visuals.widgets.hovered.bg_stroke = egui::Stroke::NONE;
        visuals.widgets.active.bg_fill = egui::Color32::BLACK;
        visuals.widgets.active.weak_bg_fill = egui::Color32::BLACK;
        visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0_f32, egui::Color32::WHITE);
        visuals.widgets.active.bg_stroke = egui::Stroke::NONE;
        visuals.widgets.open.bg_fill = egui::Color32::BLACK;
        visuals.widgets.open.weak_bg_fill = egui::Color32::BLACK;
        visuals.widgets.open.fg_stroke = egui::Stroke::new(1.0_f32, text);
        visuals.widgets.open.bg_stroke = egui::Stroke::NONE;
        context.set_visuals(visuals);
        context.style_mut(|style| {
            style.spacing.item_spacing = egui::vec2(12.0, 10.0);
            style.spacing.button_padding = egui::vec2(12.0, 8.0);
            style.spacing.interact_size = egui::vec2(44.0, 34.0);
            if let Some(font) = style.text_styles.get_mut(&egui::TextStyle::Body) {
                font.size = 19.0;
            }
            if let Some(font) = style.text_styles.get_mut(&egui::TextStyle::Small) {
                font.size = 16.0;
            }
            if let Some(font) = style.text_styles.get_mut(&egui::TextStyle::Heading) {
                font.size = 28.0;
            }
        });

        let tab_id = egui::Id::new(DASHBOARD_TAB_ID);
        let mut active_tab = context
            .data_mut(|data| data.get_temp::<DashboardTab>(tab_id))
            .unwrap_or_default();
        egui::TopBottomPanel::top("app_header")
            .show_separator_line(false)
            .show(context, |ui| {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.heading("Twitch Streak Saver");
                        ui.label(egui::RichText::new(active_tab.subtitle()).small().weak());
                    });
                    let status = match &self.connection_status {
                        TwitchConnectionStatus::Disconnected => "NOT CONNECTED".to_owned(),
                        TwitchConnectionStatus::Connecting => "CONNECTING TO TWITCH…".to_owned(),
                        TwitchConnectionStatus::Connected(_) if self.hide_sensitive_info => {
                            "CONNECTED".to_owned()
                        }
                        TwitchConnectionStatus::Connected(login) => format!("CONNECTED · {login}"),
                    };
                    let connecting =
                        matches!(&self.connection_status, TwitchConnectionStatus::Connecting);
                    let connected = matches!(
                        &self.connection_status,
                        TwitchConnectionStatus::Connected(_)
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(egui::RichText::new(status).strong().color(if connected {
                            egui::Color32::from_rgb(80, 210, 140)
                        } else if connecting {
                            egui::Color32::YELLOW
                        } else {
                            egui::Color32::from_rgb(255, 120, 120)
                        }));
                        if self.twitch_network_unavailable {
                            ui.colored_label(egui::Color32::YELLOW, "NETWORK RECOVERING");
                        }
                        if connecting {
                            if ui.button("Cancel").clicked() {
                                self.cancel_twitch_connection();
                            }
                        } else if connected {
                            if ui.button("Disconnect").clicked() {
                                self.disconnect_twitch();
                            }
                        } else if ui
                            .add(
                                egui::Button::new("Connect Twitch")
                                    .fill(egui::Color32::from_rgb(145, 70, 255)),
                            )
                            .clicked()
                        {
                            self.start_twitch_connection();
                        }
                    });
                });
                ui.add_space(8.0);
            });

        egui::SidePanel::left("primary_navigation")
            .resizable(false)
            .exact_width(220.0)
            .frame(
                egui::Frame::new()
                    .fill(egui::Color32::BLACK)
                    .stroke(egui::Stroke::NONE)
                    .inner_margin(egui::Margin::same(14)),
            )
            .show(context, |ui| {
                ui.add_space(12.0);
                ui.label(egui::RichText::new("NAVIGATION").small().weak());
                ui.add_space(12.0);
                active_tab = sidebar_tabs(ui, active_tab);
            });
        context.data_mut(|data| data.insert_temp(tab_id, active_tab));

        egui::CentralPanel::default().show(context, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    self.show_dashboard_content(ui, active_tab);
                });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_log_filters_by_kind_and_fuzzy_text_across_fields() {
        let entry = ActivityLogEntry {
            time: "2026-10-19 21:15:09".to_owned(),
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
            time: "2026-10-19 21:15:10".to_owned(),
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
}
fn draw_automation_pause_controls(
    ui: &mut egui::Ui,
    automation_delays: &mut automation::AutomationDelays,
    now: Duration,
    automation_enabled: bool,
) {
    let remaining = automation_delays.global_pause_remaining(now);
    let mut resume = false;
    let mut pause_for = None;

    if let Some(remaining) = remaining {
        ui.horizontal(|ui| {
            ui.label(format!(
                "Automation paused — resumes in {}.",
                automation::format_remaining_duration(remaining)
            ));
            resume = ui.button("Resume now").clicked();
            if automation_enabled {
                ui.menu_button("Extend pause", |ui| {
                    for (duration, label) in automation::TEMPORARY_AUTOMATION_DURATIONS {
                        if ui.button(label).clicked() {
                            pause_for = Some(duration);
                            ui.close_menu();
                        }
                    }
                });
            }
        });
    } else if automation_enabled {
        ui.menu_button("Pause automatic opening", |ui| {
            for (duration, label) in automation::TEMPORARY_AUTOMATION_DURATIONS {
                if ui.button(label).clicked() {
                    pause_for = Some(duration);
                    ui.close_menu();
                }
            }
        });
    }

    if resume {
        automation_delays.resume();
    }
    if let Some(duration) = pause_for {
        automation_delays.pause_for(now, duration);
    }
}
