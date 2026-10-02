mod fullscreen;
mod island;
mod topmost;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(island::IslandState::default())
        .invoke_handler(tauri::generate_handler![island::set_hit_area])
        .setup(|app| {
            let window = app
                .get_webview_window(island::ISLAND)
                .expect("island window is declared in tauri.conf.json");
            island::place_top_centre(&window)?;
            island::start(app.handle(), window.clone())?;
            window.show()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running OFA");
}
