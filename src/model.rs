#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChannelSelection {
    pub streak: bool,
    pub points: bool,
}

impl ChannelSelection {
    pub fn is_selected(self) -> bool {
        self.streak || self.points
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FollowedChannel {
    pub id: String,
    pub display_name: String,
    #[serde(default)]
    pub login: String,
    pub selection: ChannelSelection,
}

impl FollowedChannel {
    pub fn new(id: impl Into<String>, display_name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            display_name: display_name.into(),
            login: String::new(),
            selection: ChannelSelection::default(),
        }
    }
}

pub fn merge_followed_channels(
    current: &[FollowedChannel],
    incoming: Vec<FollowedChannel>,
) -> Vec<FollowedChannel> {
    let selections: std::collections::HashMap<_, _> = current
        .iter()
        .map(|channel| (channel.id.as_str(), channel.selection))
        .collect();
    incoming
        .into_iter()
        .map(|mut channel| {
            if let Some(selection) = selections.get(channel.id.as_str()) {
                channel.selection = *selection;
            }
            channel
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AppPreferences {
    pub auto_start_on_launch: bool,
    pub wake_browser_on_open: bool,
    pub close_on_raid_redirect: bool,
    pub channels: Vec<FollowedChannel>,
}

impl Default for AppPreferences {
    fn default() -> Self {
        Self {
            auto_start_on_launch: true,
            wake_browser_on_open: false,
            close_on_raid_redirect: false,
            channels: vec![
                FollowedChannel::new("sample-01", "Sample channel one"),
                FollowedChannel::new("sample-02", "Sample channel two"),
                FollowedChannel::new("sample-03", "Sample channel three"),
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AppPreferences, ChannelSelection};

    #[test]
    fn followed_channel_refresh_keeps_selections_for_matching_ids() {
        let mut existing = super::FollowedChannel::new("channel-1", "Old name");
        existing.selection = ChannelSelection {
            streak: true,
            points: false,
        };
        let removed = super::FollowedChannel::new("unfollowed", "No longer followed");
        let incoming = vec![
            super::FollowedChannel::new("channel-1", "Updated name"),
            super::FollowedChannel::new("channel-2", "New channel"),
        ];

        let refreshed = super::merge_followed_channels(&[existing, removed], incoming);

        assert_eq!(refreshed.len(), 2);
        assert_eq!(refreshed[0].display_name, "Updated name");
        assert_eq!(
            refreshed[0].selection,
            ChannelSelection {
                streak: true,
                points: false,
            }
        );
        assert_eq!(refreshed[1].selection, ChannelSelection::default());
    }
    #[test]
    fn preferences_round_trip_settings() {
        let mut preferences = AppPreferences::default();
        preferences.channels[0].selection = ChannelSelection {
            streak: true,
            points: true,
        };
        preferences.auto_start_on_launch = false;
        preferences.wake_browser_on_open = true;
        preferences.close_on_raid_redirect = true;

        let encoded = ron::to_string(&preferences).unwrap();
        let decoded: AppPreferences = ron::from_str(&encoded).unwrap();

        assert_eq!(decoded, preferences);
    }

    #[test]
    fn legacy_preferences_ignore_removed_fields() {
        let preferences: AppPreferences = ron::from_str(
            "(mode: open_all, auto_start_on_launch: false, wake_browser_on_open: true, tab_layout: separate_player_chat, channels: [], legacy_setting: false)",
        )
        .unwrap();

        assert!(!preferences.auto_start_on_launch);
        assert!(preferences.wake_browser_on_open);
        assert!(!preferences.close_on_raid_redirect);
        assert!(preferences.channels.is_empty());
    }

    #[test]
    fn channel_can_be_selected_for_streaks_and_points_independently() {
        let mut selection = ChannelSelection::default();
        assert!(!selection.is_selected());

        selection.streak = true;
        selection.points = true;
        assert!(selection.is_selected());

        selection.streak = false;
        assert!(selection.is_selected());
        assert!(selection.points);
    }
}
