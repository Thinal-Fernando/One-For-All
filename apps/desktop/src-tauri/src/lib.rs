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
mod tray;
mod updates;
mod usage;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Must come first. Starting OFA again while it runs opens the
        // settings instead of a second orb.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            settings::open(app);
        }))
        .plugin(tauri_plugin_autostart::Builder::new().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(island::IslandState::default())
        .manage(sessions::Sessions::default())
        .manage(usage::Usage::default())
        .manage(updates::Updates::default())
        .invoke_handler(tauri::generate_handler![
            island::set_hit_area,
            island::get_island_layout,
            sessions::get_sessions,
            sessions::focus_session,
            sessions::answer_prompt,
            sessions::dismiss_session,
            settings::get_settings,
            settings::save_settings,
            settings::open_settings,
            settings::get_autostart,
            settings::set_autostart,
            updates::get_update_status,
            updates::check_for_update,
            updates::install_update,
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
            tray::start(app.handle())?;
            updates::start(app.handle());
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
