//! OFA's settings, kept in `%APPDATA%\OFA\settings.json`.
//!
//! The file is read each time a setting is needed, so a change applies
//! within a second or two without restarting. Anything missing or invalid
//! falls back to its default. The settings window edits the same file, and
//! keeps any keys it doesn't know about.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_autostart::ManagerExt;

use crate::{island, usage};

/// The settings window's label.
pub const SETTINGS: &str = "settings";

/// Smallest and largest orb, in CSS pixels.
pub const MIN_ORB: u32 = 16;
pub const MAX_ORB: u32 = 64;
const DEFAULT_ORB: u32 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Settings {
    /// Read exact plan usage from Claude with Claude Code's sign-in.
    pub exact_usage: bool,
    pub island: IslandSettings,
}

/// Where the orb sits and how big it is.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct IslandSettings {
    pub edge: Edge,
    pub size: u32,
}

impl Default for IslandSettings {
    fn default() -> Self {
        Self {
            edge: Edge::default(),
            size: DEFAULT_ORB,
        }
    }
}

/// The screen edge the orb sits on: halfway down the right or left edge, or
/// in the middle of the top edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Edge {
    #[default]
    Right,
    Left,
    Top,
}

/// The current settings. Never fails: a missing or broken file gives the
/// defaults.
pub fn load() -> Settings {
    path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|text| parse(&text))
        .unwrap_or_default()
}

/// Reads each setting on its own, so one bad value never resets the others.
fn parse(text: &str) -> Settings {
    let root: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).unwrap_or_default();
    let island = &root["island"];
    let edge = match island["edge"].as_str() {
        Some("left") => Edge::Left,
        Some("top") => Edge::Top,
        _ => Edge::Right,
    };
    let size = island["size"].as_u64().map_or(DEFAULT_ORB, |n| {
        n.clamp(MIN_ORB.into(), MAX_ORB.into()) as u32
    });
    Settings {
        exact_usage: root["exact_usage"].as_bool().unwrap_or(false),
        island: IslandSettings { edge, size },
    }
}

/// Writes `settings` over the file's copies of them, keeping everything else
/// in the file as it was.
fn save(settings: Settings) -> std::io::Result<()> {
    let path = path().ok_or_else(|| std::io::Error::other("no settings folder"))?;
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    let text = merge(&old, settings);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Write beside it and swap, so a reader never sees half a file.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, &path)
}

/// `old`'s JSON with the settings OFA knows about replaced by `settings`.
fn merge(old: &str, settings: Settings) -> String {
    let mut root = match serde_json::from_str(old.trim_start_matches('\u{feff}')) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    };
    let size = settings.island.size.clamp(MIN_ORB, MAX_ORB);
    root.insert("exact_usage".into(), json!(settings.exact_usage));
    root.insert(
        "island".into(),
        json!({ "edge": settings.island.edge, "size": size }),
    );
    let mut text = serde_json::to_string_pretty(&Value::Object(root)).unwrap_or_default();
    text.push('\n');
    text
}

/// Lets the settings window show what is set now.
#[tauri::command]
pub fn get_settings() -> Settings {
    load()
}

/// Saves the settings window's changes and applies them straight away.
#[tauri::command]
pub fn save_settings(app: AppHandle, settings: Settings) -> Result<(), String> {
    save(settings).map_err(|err| format!("could not save the settings: {err}"))?;
    if let Some(window) = app.get_webview_window(island::ISLAND) {
        island::place(&window).map_err(|err| err.to_string())?;
    }
    usage::refresh(&app);
    Ok(())
}

/// Whether OFA starts when you sign in to Windows. Kept by Windows itself
/// (the Run list in the registry), not in `settings.json`.
#[tauri::command]
pub fn get_autostart(app: AppHandle) -> Result<bool, String> {
    app.autolaunch().is_enabled().map_err(|err| err.to_string())
}

/// Turns starting with Windows on or off.
#[tauri::command]
pub fn set_autostart(app: AppHandle, enabled: bool) -> Result<(), String> {
    let autolaunch = app.autolaunch();
    let result = if enabled {
        autolaunch.enable()
    } else {
        autolaunch.disable()
    };
    result.map_err(|err| format!("could not change starting with Windows: {err}"))
}

/// Opens the settings window, or brings it forward if it is already open.
///
/// Async on purpose: on Windows, creating a window from a sync command
/// deadlocks the webview and the window stays blank.
#[tauri::command]
pub async fn open_settings(app: AppHandle) -> Result<(), String> {
    show_window(&app).map_err(|err| err.to_string())
}

/// Opens the settings window from anywhere outside a command, such as the
/// tray menu or a second launch of OFA. Done off the main thread for the
/// same reason `open_settings` is async.
pub fn open(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(err) = show_window(&app) {
            eprintln!("settings: could not open the window: {err}");
        }
    });
}

fn show_window(app: &AppHandle) -> tauri::Result<()> {
    let window = match app.get_webview_window(SETTINGS) {
        Some(window) => window,
        None => build_window(app)?,
    };
    window.unminimize()?;
    window.show()?;
    // The click comes from the island, which never takes focus, so Windows
    // won't let the new window come forward on its own and it would open
    // behind whatever you were using. Pinning it on top for a moment does.
    window.set_always_on_top(true)?;
    window.set_focus()?;
    window.set_always_on_top(false)
}

fn build_window(app: &AppHandle) -> tauri::Result<tauri::WebviewWindow> {
    WebviewWindowBuilder::new(app, SETTINGS, WebviewUrl::App("index.html".into()))
        .title("OFA Settings")
        .inner_size(420.0, 600.0)
        .resizable(false)
        .maximizable(false)
        .theme(Some(tauri::Theme::Dark))
        .center()
        .focused(true)
        .build()
}

fn path() -> Option<PathBuf> {
    Some(ofa_protocol::token_path()?.with_file_name("settings.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_file_gives_the_defaults() {
        let s = parse("{}");
        assert!(!s.exact_usage);
        assert_eq!(
            s.island,
            IslandSettings {
                edge: Edge::Right,
                size: 20
            }
        );
        assert_eq!(parse("not json"), Settings::default());
    }

    #[test]
    fn reads_edge_size_and_usage() {
        let s = parse(r#"{"exact_usage": true, "island": {"edge": "top", "size": 32}}"#);
        assert!(s.exact_usage);
        assert_eq!(
            s.island,
            IslandSettings {
                edge: Edge::Top,
                size: 32
            }
        );
    }

    #[test]
    fn a_size_out_of_range_is_clamped() {
        assert_eq!(parse(r#"{"island": {"size": 2}}"#).island.size, MIN_ORB);
        assert_eq!(parse(r#"{"island": {"size": 500}}"#).island.size, MAX_ORB);
    }

    #[test]
    fn saving_keeps_unknown_keys_and_round_trips() {
        let new = Settings {
            exact_usage: true,
            island: IslandSettings {
                edge: Edge::Left,
                size: 99,
            },
        };
        let text = merge(r#"{"later": 1, "exact_usage": false}"#, new);
        let root: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(root["later"], 1);
        let back = parse(&text);
        assert!(back.exact_usage);
        assert_eq!(back.island.edge, Edge::Left);
        assert_eq!(back.island.size, MAX_ORB);
        // A broken file is replaced rather than kept.
        assert_eq!(parse(&merge("not json", new)), back);
    }

    #[test]
    fn a_bad_value_only_resets_itself() {
        let s = parse(r#"{"exact_usage": true, "island": {"edge": "bottom", "size": 30}}"#);
        assert!(s.exact_usage);
        assert_eq!(
            s.island,
            IslandSettings {
                edge: Edge::Right,
                size: 30
            }
        );
    }
}
