mod api;
mod focus;
mod fullscreen;
mod island;
mod plan;
mod sessions;
mod settings;
mod shortcuts;
mod topmost;
mod transcript;
mod usage;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(island::IslandState::default())
        .manage(sessions::Sessions::default())
        .manage(usage::Usage::default())
        .invoke_handler(tauri::generate_handler![
            island::set_hit_area,
            island::get_island_layout,
            sessions::get_sessions,
            sessions::focus_session,
            sessions::answer_prompt,
            settings::get_settings,
            settings::save_settings,
            settings::open_settings,
            usage::get_usage
        ])
        .setup(|app| {
            let window = app
                .get_webview_window(island::ISLAND)
                .expect("island window is declared in tauri.conf.json");
            island::place(&window)?;
            island::start(app.handle(), window.clone())?;
            sessions::start(app.handle())?;
            shortcuts::start(app.handle())?;
            usage::start(app.handle())?;
            // Without the API the island still runs, it just hears nothing.
            if let Err(err) = api::start(app.handle()) {
                eprintln!("api: not started: {err}");
            }
            window.show()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running OFA");
}
