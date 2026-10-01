use tauri::{Manager, PhysicalPosition, WebviewWindow};

/// Label of the single island window, as declared in tauri.conf.json.
const ISLAND: &str = "island";

/// Places the island at the top centre of the primary monitor.
fn place_top_centre(window: &WebviewWindow) -> tauri::Result<()> {
    let Some(monitor) = window.primary_monitor()? else {
        return Ok(());
    };
    let area = monitor.position();
    let screen = monitor.size();
    let size = window.outer_size()?;
    let x = area.x + (screen.width as i32 - size.width as i32) / 2;
    window.set_position(PhysicalPosition::new(x, area.y))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let island = app
                .get_webview_window(ISLAND)
                .expect("island window is declared in tauri.conf.json");
            place_top_centre(&island)?;
            island.show()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running OFA");
}
