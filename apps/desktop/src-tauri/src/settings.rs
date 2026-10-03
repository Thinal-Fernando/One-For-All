//! OFA's settings, kept in `%APPDATA%\OFA\settings.json`.
//!
//! The file is read each time a setting is needed, so a change applies
//! within a second or two without restarting. Anything missing or invalid
//! falls back to its default. A settings window replaces hand editing later.

use std::path::PathBuf;

use serde::Serialize;
use serde_json::Value;

/// Smallest and largest orb, in CSS pixels.
pub const MIN_ORB: u32 = 16;
pub const MAX_ORB: u32 = 64;
const DEFAULT_ORB: u32 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Settings {
    /// Read exact plan usage from Claude with Claude Code's sign-in.
    pub exact_usage: bool,
    pub island: IslandSettings,
}

/// Where the orb sits and how big it is.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
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
