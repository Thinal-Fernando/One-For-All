//! Ctrl+Alt+Y allows and Ctrl+Alt+N denies the prompt that has waited
//! longest, from any app, without moving focus to the island or the terminal.
//!
//! Windows' own hotkey registration does the work: the island only learns
//! that the key combination was pressed, and never sees other keystrokes.

use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut, ShortcutState};

use crate::sessions::Sessions;

fn allow() -> Shortcut {
    Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyY)
}

fn deny() -> Shortcut {
    Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyN)
}

/// Registers both shortcuts. If another app already owns one, the island
/// works without it and says so on stderr.
pub fn start(app: &AppHandle) -> tauri::Result<()> {
    let plugin = tauri_plugin_global_shortcut::Builder::new()
        .with_handler(|app, shortcut, event| {
            if event.state != ShortcutState::Pressed {
                return;
            }
            let allowed = *shortcut == allow();
            if !allowed && *shortcut != deny() {
                return;
            }
            if let Err(err) = app.state::<Sessions>().answer_longest_waiting(app, allowed) {
                eprintln!("shortcuts: {err}");
            }
        })
        .build();
    app.plugin(plugin)?;

    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    for shortcut in [allow(), deny()] {
        if let Err(err) = app.global_shortcut().register(shortcut) {
            eprintln!("shortcuts: could not register {shortcut}: {err}");
        }
    }
    Ok(())
}
