use super::*;
pub(super) fn channel_selection_controls(
    ui: &mut egui::Ui,
    channel: &mut FollowedChannel,
) -> egui::Response {
    let unmuted = ui
        .checkbox(&mut channel.unmuted, "Unmuted")
        .on_hover_text("Start this player's tab with audio enabled.");
    ui.checkbox(&mut channel.selection.points, "Points");
    ui.checkbox(&mut channel.selection.streak, "Streak");
    unmuted
}

pub(super) fn selection_bulk_controls(
    ui: &mut egui::Ui,
    channels: &mut [FollowedChannel],
    matching_indices: &[usize],
) -> [egui::Response; 4] {
    let all_channels_enabled = !channels.is_empty();
    let matching_channels_enabled = !matching_indices.is_empty();
    let all_points_selected =
        all_channels_enabled && channels.iter().all(|channel| channel.selection.points);
    let all_streaks_selected =
        all_channels_enabled && channels.iter().all(|channel| channel.selection.streak);
    let all_matching_points_selected = matching_channels_enabled
        && matching_indices
            .iter()
            .all(|&index| channels[index].selection.points);
    let all_matching_streaks_selected = matching_channels_enabled
        && matching_indices
            .iter()
            .all(|&index| channels[index].selection.streak);

    ui.horizontal_wrapped(|ui| {
        let all_points_label = if all_points_selected {
            "Unselect all Points"
        } else {
            "Select all Points"
        };
        let all_points = ui.add_enabled(all_channels_enabled, egui::Button::new(all_points_label));
        let select_all_points = all_points.clicked();
        let all_points = all_points.on_hover_text(if all_points_selected {
            "Clear Points selection for all followed channels, regardless of the current search."
        } else {
            "Select Points for all followed channels, regardless of the current search."
        });
        if select_all_points {
            for channel in channels.iter_mut() {
                channel.selection.points = !all_points_selected;
            }
        }

        let all_streaks_label = if all_streaks_selected {
            "Unselect all Streaks"
        } else {
            "Select all Streaks"
        };
        let all_streaks =
            ui.add_enabled(all_channels_enabled, egui::Button::new(all_streaks_label));
        let select_all_streaks = all_streaks.clicked();
        let all_streaks = all_streaks.on_hover_text(if all_streaks_selected {
            "Clear Streak selection for all followed channels, regardless of the current search."
        } else {
            "Select Streaks for all followed channels, regardless of the current search."
        });
        if select_all_streaks {
            for channel in channels.iter_mut() {
                channel.selection.streak = !all_streaks_selected;
            }
        }

        let matching_points_label = if all_matching_points_selected {
            "Unselect matching Points"
        } else {
            "Select matching Points"
        };
        let matching_points = ui.add_enabled(
            matching_channels_enabled,
            egui::Button::new(matching_points_label),
        );
        let select_matching_points = matching_points.clicked();
        let matching_points = matching_points.on_hover_text(if all_matching_points_selected {
            "Clear Points selection only for followed channels matching the current search."
        } else {
            "Select Points only for followed channels matching the current search."
        });
        if select_matching_points {
            for &index in matching_indices {
                channels[index].selection.points = !all_matching_points_selected;
            }
        }

        let matching_streaks_label = if all_matching_streaks_selected {
            "Unselect matching Streaks"
        } else {
            "Select matching Streaks"
        };
        let matching_streaks = ui.add_enabled(
            matching_channels_enabled,
            egui::Button::new(matching_streaks_label),
        );
        let select_matching_streaks = matching_streaks.clicked();
        let matching_streaks = matching_streaks.on_hover_text(if all_matching_streaks_selected {
            "Clear Streaks selection only for followed channels matching the current search."
        } else {
            "Select Streaks only for followed channels matching the current search."
        });
        if select_matching_streaks {
            for &index in matching_indices {
                channels[index].selection.streak = !all_matching_streaks_selected;
            }
        }

        [all_points, all_streaks, matching_points, matching_streaks]
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::*;
    fn show_selection_controls(
        context: &egui::Context,
        channels: &mut [FollowedChannel],
        matching_indices: &[usize],
        events: Vec<egui::Event>,
    ) -> [egui::Rect; 4] {
        let mut button_rects = None;
        let _ = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600.0, 300.0),
                )),
                events,
                ..Default::default()
            },
            |context| {
                egui::CentralPanel::default().show(context, |ui| {
                    let [all_points, all_streaks, matching_points, matching_streaks] =
                        selection_bulk_controls(ui, channels, matching_indices);
                    button_rects = Some([
                        all_points.rect,
                        all_streaks.rect,
                        matching_points.rect,
                        matching_streaks.rect,
                    ]);
                });
            },
        );
        button_rects.expect("selection controls were not rendered")
    }
    fn click_button(
        context: &egui::Context,
        channels: &mut [FollowedChannel],
        matching_indices: &[usize],
        index: usize,
    ) {
        let rect = show_selection_controls(context, channels, matching_indices, Vec::new())[index];
        let position = rect.center();
        for pressed in [true, false] {
            let _ = show_selection_controls(
                context,
                channels,
                matching_indices,
                vec![
                    egui::Event::PointerMoved(position),
                    egui::Event::PointerButton {
                        pos: position,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
    }

    fn show_channel_selection_controls(
        context: &egui::Context,
        channel: &mut FollowedChannel,
        events: Vec<egui::Event>,
    ) -> egui::Rect {
        let mut unmuted_rect = None;
        let _ = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600.0, 300.0),
                )),
                events,
                ..Default::default()
            },
            |context| {
                egui::CentralPanel::default().show(context, |ui| {
                    unmuted_rect = Some(channel_selection_controls(ui, channel).rect);
                });
            },
        );
        unmuted_rect.expect("channel selection controls were not rendered")
    }

    fn click_unmuted(context: &egui::Context, channel: &mut FollowedChannel) {
        let position = show_channel_selection_controls(context, channel, Vec::new()).center();
        for pressed in [true, false] {
            let _ = show_channel_selection_controls(
                context,
                channel,
                vec![
                    egui::Event::PointerMoved(position),
                    egui::Event::PointerButton {
                        pos: position,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
    }

    #[test]
    fn unmuted_checkbox_only_changes_audio_preference() {
        let mut channel = FollowedChannel::new("channel", "Channel");
        let context = egui::Context::default();

        click_unmuted(&context, &mut channel);

        assert!(channel.unmuted);
        assert!(!channel.selection.streak);
        assert!(!channel.selection.points);

        click_unmuted(&context, &mut channel);

        assert!(!channel.unmuted);
        assert!(!channel.selection.streak);
        assert!(!channel.selection.points);
    }

    #[test]
    fn all_channel_bulk_buttons_ignore_the_current_search() {
        let mut first = FollowedChannel::new("first", "First");
        first.selection.streak = true;
        let hidden = FollowedChannel::new("hidden", "Hidden");
        let mut third = FollowedChannel::new("third", "Third");
        third.selection.points = true;
        let mut channels = vec![first, hidden, third];
        let matching_indices = followed_channel_search_indices(&channels, "t");
        assert_eq!(matching_indices, vec![0, 2]);
        let context = egui::Context::default();

        click_button(&context, &mut channels, &matching_indices, 0);

        assert!(channels.iter().all(|channel| channel.selection.points));

        click_button(&context, &mut channels, &matching_indices, 1);

        assert!(channels.iter().all(|channel| channel.selection.streak));

        click_button(&context, &mut channels, &matching_indices, 0);

        assert!(channels.iter().all(|channel| !channel.selection.points));

        click_button(&context, &mut channels, &matching_indices, 1);

        assert!(channels.iter().all(|channel| !channel.selection.streak));
    }

    #[test]
    fn matching_bulk_buttons_only_toggle_channels_matching_the_search() {
        let mut first = FollowedChannel::new("first", "First");
        first.selection.streak = true;
        let hidden = FollowedChannel::new("hidden", "Hidden");
        let third = FollowedChannel::new("third", "Third");
        let mut channels = vec![first, hidden, third];
        let matching_indices = followed_channel_search_indices(&channels, "t");
        assert_eq!(matching_indices, vec![0, 2]);
        let context = egui::Context::default();

        click_button(&context, &mut channels, &matching_indices, 2);

        assert!(channels[0].selection.points);
        assert!(!channels[1].selection.points);
        assert!(channels[2].selection.points);

        channels[1].selection.points = true;
        click_button(&context, &mut channels, &matching_indices, 2);

        assert!(!channels[0].selection.points);
        assert!(channels[1].selection.points);
        assert!(!channels[2].selection.points);

        click_button(&context, &mut channels, &matching_indices, 3);

        assert!(channels[0].selection.streak);
        assert!(!channels[1].selection.streak);
        assert!(channels[2].selection.streak);

        channels[1].selection.streak = true;
        click_button(&context, &mut channels, &matching_indices, 3);

        assert!(!channels[0].selection.streak);
        assert!(channels[1].selection.streak);
        assert!(!channels[2].selection.streak);
    }
}
