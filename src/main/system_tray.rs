use tray_icon::{
    menu::{Menu, MenuEvent, MenuId, MenuItem},
    Icon, TrayIcon, TrayIconBuilder,
};

const OPEN_MENU_ID: &str = "open-window";
const QUIT_MENU_ID: &str = "quit-app";

pub(super) enum TrayAction {
    Open,
    Quit,
}

pub(super) struct SystemTray {
    _icon: TrayIcon,
    open_menu_id: MenuId,
    quit_menu_id: MenuId,
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

        let icon = tray_icon_image()?;
        let tray_icon = TrayIconBuilder::new()
            .with_icon(icon)
            .with_tooltip("Twitch Streak Saver")
            .with_menu(Box::new(menu))
            .build()
            .map_err(|error| format!("Could not create the menu-bar/tray icon: {error}"))?;

        Ok(Self {
            _icon: tray_icon,
            open_menu_id: open_item.id().clone(),
            quit_menu_id: quit_item.id().clone(),
        })
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

fn tray_icon_image() -> Result<Icon, String> {
    const SIZE: u32 = 16;
    const PURPLE: [u8; 4] = [112, 52, 191, 255];
    const WHITE: [u8; 4] = [255, 255, 255, 255];

    let mut rgba = vec![0; (SIZE * SIZE * 4) as usize];
    let set_pixel = |rgba: &mut [u8], x: u32, y: u32, color: [u8; 4]| {
        let offset = ((y * SIZE + x) * 4) as usize;
        rgba[offset..offset + 4].copy_from_slice(&color);
    };

    for y in 2..14 {
        for x in 2..14 {
            set_pixel(&mut rgba, x, y, PURPLE);
        }
    }
    for y in 4..10 {
        for x in 4..12 {
            set_pixel(&mut rgba, x, y, WHITE);
        }
    }
    for y in 10..12 {
        for x in 4..6 {
            set_pixel(&mut rgba, x, y, WHITE);
        }
    }
    for y in 6..9 {
        set_pixel(&mut rgba, 6, y, PURPLE);
        set_pixel(&mut rgba, 7, y, PURPLE);
        set_pixel(&mut rgba, 10, y, PURPLE);
        set_pixel(&mut rgba, 11, y, PURPLE);
    }

    Icon::from_rgba(rgba, SIZE, SIZE)
        .map_err(|error| format!("Could not create the menu-bar/tray icon image: {error}"))
}

#[cfg(test)]
mod tests {
    use super::should_hide_on_close;

    #[test]
    fn close_only_hides_when_enabled_and_tray_is_available() {
        assert!(should_hide_on_close(true, true, false));
        assert!(!should_hide_on_close(false, true, false));
        assert!(!should_hide_on_close(true, false, false));
        assert!(!should_hide_on_close(true, true, true));
    }
}
