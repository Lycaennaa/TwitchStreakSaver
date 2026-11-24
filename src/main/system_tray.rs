use tray_icon::{
    menu::{Menu, MenuEvent, MenuId, MenuItem},
    Icon, TrayIcon, TrayIconBuilder,
};
use twitch_streak_saver::browser_protocol::AppStatusIndicator;

const OPEN_MENU_ID: &str = "open-window";
const QUIT_MENU_ID: &str = "quit-app";
const TRAY_ICON_SIZE: u32 = 32;
const TRAY_ACTIVE_RGBA: &[u8] = include_bytes!("../../assets/icons/tray-active-32.rgba");
const TRAY_PAUSED_RGBA: &[u8] = include_bytes!("../../assets/icons/tray-paused-32.rgba");
const TRAY_DISCONNECTED_RGBA: &[u8] =
    include_bytes!("../../assets/icons/tray-disconnected-32.rgba");

pub(super) enum TrayAction {
    Open,
    Quit,
}

pub(super) struct SystemTray {
    icon: TrayIcon,
    open_menu_id: MenuId,
    quit_menu_id: MenuId,
    status: AppStatusIndicator,
}

impl SystemTray {
    pub(super) fn new() -> Result<Self, String> {
        let open_item = MenuItem::with_id(OPEN_MENU_ID, "Open Twitch Streak Saver", true, None);
        let quit_item = MenuItem::with_id(QUIT_MENU_ID, "Quit", true, None);
        let menu = Menu::new();
        menu.append(&open_item)
            .map_err(|error| format!("Could not add the tray open action: {error}"))?;
        menu.append(&quit_item)
            .map_err(|error| format!("Could not add the tray quit action: {error}"))?;

        let initial_status = AppStatusIndicator::Disconnected;
        let icon = tray_icon_image(initial_status)?;
        let tray_icon = TrayIconBuilder::new()
            .with_icon(icon)
            .with_tooltip(status_tooltip(initial_status))
            .with_menu(Box::new(menu))
            .build()
            .map_err(|error| format!("Could not create the menu-bar/tray icon: {error}"))?;

        Ok(Self {
            icon: tray_icon,
            open_menu_id: open_item.id().clone(),
            quit_menu_id: quit_item.id().clone(),
            status: initial_status,
        })
    }

    pub(super) fn set_status(&mut self, status: AppStatusIndicator) {
        if self.status == status {
            return;
        }
        if let Ok(icon) = tray_icon_image(status) {
            let _ = self.icon.set_icon(Some(icon));
        }
        let _ = self.icon.set_tooltip(Some(status_tooltip(status)));
        self.status = status;
    }

    pub(super) fn drain_events(&self, mut handle: impl FnMut(TrayAction)) {
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id == self.open_menu_id {
                handle(TrayAction::Open);
            } else if event.id == self.quit_menu_id {
                handle(TrayAction::Quit);
            }
        }
    }
}

pub(super) fn should_hide_on_close(
    close_to_tray: bool,
    tray_available: bool,
    explicit_quit: bool,
) -> bool {
    close_to_tray && tray_available && !explicit_quit
}

fn status_tooltip(status: AppStatusIndicator) -> &'static str {
    match status {
        AppStatusIndicator::Active => "Twitch Streak Saver",
        AppStatusIndicator::Paused => "Twitch Streak Saver — Paused",
        AppStatusIndicator::Disconnected => "Twitch Streak Saver — Disconnected",
    }
}

fn tray_icon_image(status: AppStatusIndicator) -> Result<Icon, String> {
    let rgba = match status {
        AppStatusIndicator::Active => TRAY_ACTIVE_RGBA,
        AppStatusIndicator::Paused => TRAY_PAUSED_RGBA,
        AppStatusIndicator::Disconnected => TRAY_DISCONNECTED_RGBA,
    };
    Icon::from_rgba(rgba.to_vec(), TRAY_ICON_SIZE, TRAY_ICON_SIZE)
        .map_err(|error| format!("Could not create the menu-bar/tray icon image: {error}"))
}

#[cfg(test)]
mod tests {
    use super::{should_hide_on_close, status_tooltip, tray_icon_image};
    use twitch_streak_saver::browser_protocol::AppStatusIndicator;

    #[test]
    fn close_only_hides_when_enabled_and_tray_is_available() {
        assert!(should_hide_on_close(true, true, false));
        assert!(!should_hide_on_close(false, true, false));
        assert!(!should_hide_on_close(true, false, false));
        assert!(!should_hide_on_close(true, true, true));
    }

    #[test]
    fn tray_icon_builds_valid_images_for_all_status_states() {
        for status in [
            AppStatusIndicator::Active,
            AppStatusIndicator::Paused,
            AppStatusIndicator::Disconnected,
        ] {
            assert!(tray_icon_image(status).is_ok());
            assert!(status_tooltip(status).starts_with("Twitch Streak Saver"));
        }
    }
}
