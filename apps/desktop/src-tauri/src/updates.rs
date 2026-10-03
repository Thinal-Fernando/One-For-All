//! Updates. OFA looks for a newer release shortly after it starts and then
//! every few hours, and says so in the settings window and the tray tooltip.
//! It only installs when you click the button: then the installer runs with
//! a small progress bar and OFA starts again on the new version.
//!
//! Releases are signed; the updater refuses any download whose signature
//! doesn't match the public key in `tauri.conf.json`.

use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::UpdaterExt;

/// Event sent to every window when what's known about updates changes.
pub const UPDATE_EVENT: &str = "update";

const FIRST_CHECK_AFTER: Duration = Duration::from_secs(30);
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// What the settings window shows about updates.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct UpdateStatus {
    pub current: String,
    /// A newer version that can be installed.
    pub available: Option<String>,
    pub checking: bool,
    pub installing: bool,
    /// Why the last check or install failed, briefly.
    pub error: Option<String>,
}

#[derive(Default)]
pub struct Updates {
    status: Mutex<UpdateStatus>,
}

fn change(app: &AppHandle, f: impl FnOnce(&mut UpdateStatus)) {
    let status = {
        let state = app.state::<Updates>();
        let mut status = state.status.lock().unwrap();
        f(&mut status);
        status.clone()
    };
    let tooltip = match &status.available {
        Some(version) => format!("OFA: version {version} is ready to install"),
        None => "OFA".to_owned(),
    };
    if let Some(tray) = app.tray_by_id(crate::tray::TRAY) {
        let _ = tray.set_tooltip(Some(&tooltip));
    }
    if let Err(err) = app.emit(UPDATE_EVENT, status) {
        eprintln!("updates: could not send to the UI: {err}");
    }
}

/// Looks for a newer release and records the answer.
async fn check(app: &AppHandle) -> Result<(), String> {
    change(app, |s| {
        s.checking = true;
        s.error = None;
    });
    let found = async {
        let update = app.updater().map_err(|e| e.to_string())?.check().await;
        update.map_err(|e| e.to_string())
    }
    .await;
    change(app, |s| {
        s.checking = false;
        match &found {
            Ok(update) => s.available = update.as_ref().map(|u| u.version.clone()),
            Err(err) => s.error = Some(format!("Couldn't check for updates: {err}")),
        }
    });
    found.map(|_| ())
}

/// Starts the background checks.
pub fn start(app: &AppHandle) {
    change(app, |s| s.current = app.package_info().version.to_string());
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK_AFTER).await;
        loop {
            if let Err(err) = check(&app).await {
                eprintln!("updates: {err}");
            }
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

#[tauri::command]
pub fn get_update_status(updates: tauri::State<'_, Updates>) -> UpdateStatus {
    updates.status.lock().unwrap().clone()
}

/// "Check now" in the settings window.
#[tauri::command]
pub async fn check_for_update(app: AppHandle) -> Result<(), String> {
    check(&app).await
}

/// "Install and restart" in the settings window. On Windows the installer
/// closes OFA, updates it and starts it again.
#[tauri::command]
pub async fn install_update(app: AppHandle) -> Result<(), String> {
    change(&app, |s| {
        s.installing = true;
        s.error = None;
    });
    let result = async {
        let update = app
            .updater()
            .map_err(|e| e.to_string())?
            .check()
            .await
            .map_err(|e| e.to_string())?
            .ok_or("OFA is already up to date")?;
        update
            .download_and_install(|_, _| {}, || {})
            .await
            .map_err(|e| e.to_string())
    }
    .await;
    if let Err(err) = &result {
        change(&app, |s| {
            s.installing = false;
            s.error = Some(format!("Couldn't install the update: {err}"));
        });
    }
    result
}
