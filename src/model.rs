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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PreferredQuality {
    #[default]
    #[serde(rename = "160p")]
    P160,
    #[serde(rename = "360p")]
    P360,
    #[serde(rename = "480p")]
    P480,
    #[serde(rename = "720p")]
    P720,
    #[serde(rename = "1080p")]
    P1080,
    #[serde(rename = "source")]
    Source,
    #[serde(rename = "auto")]
    Auto,
}

impl PreferredQuality {
    pub const ALL: [Self; 7] = [
        Self::P160,
        Self::P360,
        Self::P480,
        Self::P720,
        Self::P1080,
        Self::Source,
        Self::Auto,
    ];

    pub const fn value(self) -> &'static str {
        match self {
            Self::P160 => "160p",
            Self::P360 => "360p",
            Self::P480 => "480p",
            Self::P720 => "720p",
            Self::P1080 => "1080p",
            Self::Source => "source",
            Self::Auto => "auto",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Source => "Source",
            Self::Auto => "Twitch Auto",
            _ => self.value(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FollowedChannel {
    pub id: String,
    pub display_name: String,
    #[serde(default)]
    pub login: String,
    pub selection: ChannelSelection,
    #[serde(default)]
    pub unmuted: bool,
}

impl FollowedChannel {
    pub fn new(id: impl Into<String>, display_name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            display_name: display_name.into(),
            login: String::new(),
            selection: ChannelSelection::default(),
            unmuted: false,
        }
    }
}

pub fn merge_followed_channels(
    current: &[FollowedChannel],
    incoming: Vec<FollowedChannel>,
) -> Vec<FollowedChannel> {
    let preferences: std::collections::HashMap<_, _> = current
        .iter()
        .map(|channel| (channel.id.as_str(), (channel.selection, channel.unmuted)))
        .collect();
    incoming
        .into_iter()
        .map(|mut channel| {
            if let Some((selection, unmuted)) = preferences.get(channel.id.as_str()) {
                channel.selection = *selection;
                channel.unmuted = *unmuted;
            }
            channel
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserKind {
    Chrome,
    Brave,
    Edge,
    Helium,
}

impl BrowserKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Chrome => "Chrome",
            Self::Brave => "Brave",
            Self::Edge => "Edge",
            Self::Helium => "Helium",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BrowserLaunchTarget {
    pub browser: BrowserKind,
    pub profile_directory: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppTheme {
    #[default]
    System,
    Light,
    Dark,
    Amoled,
}

impl AppTheme {
    pub const ALL: [Self; 4] = [Self::System, Self::Light, Self::Dark, Self::Amoled];

    pub fn label(self) -> &'static str {
        match self {
            Self::System => "Follow system",
            Self::Light => "Light",
            Self::Dark => "Dark",
            Self::Amoled => "AMOLED black",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AppPreferences {
    pub auto_start_on_launch: bool,
    pub close_to_tray_on_close: bool,
    pub wake_browser_on_open: bool,
    pub close_on_raid_redirect: bool,
    pub adopt_existing_twitch_tabs: bool,
    pub hide_sensitive_info: bool,
    pub theme: AppTheme,
    pub preferred_quality: PreferredQuality,
    pub browser_launch_target: Option<BrowserLaunchTarget>,
    pub browser_profile_data_directories: Vec<(BrowserKind, String)>,
    pub channels: Vec<FollowedChannel>,
}

impl Default for AppPreferences {
    fn default() -> Self {
        Self {
            auto_start_on_launch: true,
            close_to_tray_on_close: false,
            wake_browser_on_open: false,
            close_on_raid_redirect: false,
            adopt_existing_twitch_tabs: false,
            hide_sensitive_info: false,
            theme: AppTheme::default(),
            browser_launch_target: None,
            preferred_quality: PreferredQuality::default(),
            browser_profile_data_directories: Vec::new(),
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
    use super::{AppPreferences, ChannelSelection, PreferredQuality};

    #[test]
    fn followed_channel_refresh_keeps_preferences_for_matching_ids() {
        let mut existing = super::FollowedChannel::new("channel-1", "Old name");
        existing.selection = ChannelSelection {
            streak: true,
            points: false,
        };
        existing.unmuted = true;
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
        assert!(refreshed[0].unmuted);
        assert!(!refreshed[1].unmuted);
    }
    #[test]
    fn preferences_round_trip_settings() {
        let mut preferences = AppPreferences::default();
        preferences.channels[0].selection = ChannelSelection {
            streak: true,
            points: true,
        };
        preferences.channels[0].unmuted = true;
        preferences.auto_start_on_launch = false;
        preferences.wake_browser_on_open = true;
        preferences.close_on_raid_redirect = true;
        preferences.adopt_existing_twitch_tabs = true;
        preferences.preferred_quality = PreferredQuality::P720;
        preferences.theme = super::AppTheme::Light;

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
        assert!(!preferences.close_to_tray_on_close);
        assert!(preferences.wake_browser_on_open);
        assert!(!preferences.close_on_raid_redirect);
        assert!(!preferences.adopt_existing_twitch_tabs);
        assert!(!preferences.hide_sensitive_info);
        assert!(preferences.channels.is_empty());
        assert_eq!(preferences.preferred_quality, PreferredQuality::P160);
        assert_eq!(preferences.theme, super::AppTheme::System);
    }

    #[test]
    fn legacy_followed_channel_defaults_to_muted() {
        let channel: super::FollowedChannel = ron::from_str(
            "(id: \"channel-1\", display_name: \"Streamer\", login: \"streamer\", selection: (streak: true, points: false))",
        )
        .unwrap();

        assert!(channel.selection.streak);
        assert!(!channel.selection.points);
        assert!(!channel.unmuted);
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

#[cfg(test)]
mod browser_launch_preferences_tests {
    use super::{AppPreferences, BrowserKind, BrowserLaunchTarget};

    #[test]
    fn selected_browser_profile_persists_in_preferences() {
        let target = BrowserLaunchTarget {
            browser: BrowserKind::Edge,
            profile_directory: "Profile 2".to_owned(),
        };
        let preferences = AppPreferences {
            browser_launch_target: Some(target.clone()),
            browser_profile_data_directories: vec![(
                BrowserKind::Brave,
                "/custom/brave-data".to_owned(),
            )],
            ..AppPreferences::default()
        };

        let serialized = ron::to_string(&preferences).unwrap();
        let restored: AppPreferences = ron::from_str(&serialized).unwrap();

        assert_eq!(restored.browser_launch_target, Some(target));
        assert_eq!(
            restored.browser_profile_data_directories,
            vec![(BrowserKind::Brave, "/custom/brave-data".to_owned())]
        );
    }
    #[test]
    fn existing_tab_adoption_preference_is_opt_in_and_roundtrips() {
        let mut preferences = AppPreferences::default();
        assert!(!preferences.adopt_existing_twitch_tabs);
        preferences.adopt_existing_twitch_tabs = true;

        let encoded = ron::to_string(&preferences).unwrap();
        let restored: AppPreferences = ron::from_str(&encoded).unwrap();
        assert!(restored.adopt_existing_twitch_tabs);
    }
}
