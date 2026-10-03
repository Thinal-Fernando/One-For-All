//! OFA's icon by the clock. Clicking it opens the settings; its menu also
//! hides the orb for a while and quits OFA.

use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::AppHandle;

use crate::{island, settings};

const SETTINGS: &str = "settings";
const HIDE: &str = "hide";
const QUIT: &str = "quit";

/// Puts the icon in the tray.
pub fn start(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, SETTINGS, "Settings", true, None::<&str>)?;
    let hide = CheckMenuItem::with_id(app, HIDE, "Hide the orb", true, false, None::<&str>)?;
    let quit = MenuItem::with_id(app, QUIT, "Quit OFA", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&open, &hide, &separator, &quit])?;

    let mut tray = TrayIconBuilder::with_id("ofa")
        .tooltip("OFA")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            SETTINGS => settings::open(app),
            // The menu ticks and unticks the item itself.
            HIDE => island::set_hidden_by_user(app, hide.is_checked().unwrap_or(false)),
            QUIT => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                settings::open(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}
