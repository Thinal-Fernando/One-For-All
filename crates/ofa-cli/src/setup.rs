//! `ofa setup`: adds OFA's hooks to Claude Code's user settings, or removes
//! them again with `--uninstall`.
//!
//! The settings file may already hold hooks and settings you rely on, so this
//! only ever adds or removes OFA's own entries, keeps everything else exactly
//! as it was (including key order), and takes a backup before writing.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

/// The Claude Code hook events `ofa hook` follows.
pub const EVENTS: [&str; 11] = [
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "PermissionDenied",
    "Notification",
    "Stop",
    "StopFailure",
    "SessionEnd",
];

/// Seconds Claude Code waits for a hook. `ofa hook` finishes in well under one.
const TIMEOUT_SECS: u64 = 5;

/// What `run` did, for the message printed afterwards.
pub struct Outcome {
    pub settings: PathBuf,
    pub backup: Option<PathBuf>,
    pub changed: bool,
}

/// `~/.claude/settings.json`, or the folder in `CLAUDE_CONFIG_DIR` if set.
pub fn settings_path() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        return Some(PathBuf::from(dir).join("settings.json"));
    }
    let home = std::env::var_os("USERPROFILE")?;
    Some(PathBuf::from(home).join(".claude").join("settings.json"))
}

/// Installs or removes the hooks in the settings file at `path`.
pub fn run(path: &Path, exe: &Path, uninstall: bool) -> Result<Outcome, String> {
    let original = match fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => return Err(format!("could not read {}: {err}", path.display())),
    };
    let settings: Value = match &original {
        // Some editors leave a byte order mark; Claude Code accepts it, so do we.
        Some(text) => serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|err| {
            format!(
                "{} isn't valid JSON, so it was left alone: {err}",
                path.display()
            )
        })?,
        None => json!({}),
    };
    if !settings.is_object() {
        return Err(format!("{} isn't a JSON object", path.display()));
    }

    let exe = exe.to_str().ok_or("the path to ofa.exe isn't valid text")?;
    let updated = if uninstall {
        remove_hooks(settings.clone())
    } else {
        add_hooks(settings.clone(), exe)
    };
    if updated == settings {
        return Ok(Outcome {
            settings: path.to_owned(),
            backup: None,
            changed: false,
        });
    }

    let backup = match &original {
        Some(text) => Some(write_backup(path, text)?),
        None => None,
    };
    let mut text = serde_json::to_string_pretty(&updated).map_err(|err| err.to_string())?;
    text.push('\n');
    write_atomically(path, &text)?;
    Ok(Outcome {
        settings: path.to_owned(),
        backup,
        changed: true,
    })
}

/// Adds one hook group per event that runs `<exe> hook`, replacing any OFA
/// entries already there so running setup twice changes nothing.
fn add_hooks(settings: Value, exe: &str) -> Value {
    let mut settings = remove_hooks(settings);
    let root = settings.as_object_mut().expect("checked to be an object");
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(hooks) = hooks.as_object_mut() else {
        // "hooks" exists but isn't an object; Claude Code would reject it too.
        return settings;
    };
    for event in EVENTS {
        let groups = hooks
            .entry(event)
            .or_insert_with(|| Value::Array(Vec::new()));
        if let Some(groups) = groups.as_array_mut() {
            groups.push(json!({
                "hooks": [{
                    "type": "command",
                    "command": exe,
                    "args": ["hook"],
                    "timeout": TIMEOUT_SECS
                }]
            }));
        }
    }
    settings
}

/// Removes every OFA hook, then any group, event list or `hooks` object that
/// is left empty because of it. Everything else stays as it was.
fn remove_hooks(mut settings: Value) -> Value {
    let Some(root) = settings.as_object_mut() else {
        return settings;
    };
    let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) else {
        return settings;
    };

    let mut emptied = Vec::new();
    for (event, groups) in hooks.iter_mut() {
        let Some(list) = groups.as_array_mut() else {
            continue;
        };
        let before = list.len();
        list.retain_mut(|group| {
            let Some(handlers) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
                return true;
            };
            let before = handlers.len();
            handlers.retain(|handler| !is_ofa_hook(handler));
            // Only drop a group that OFA emptied, never one that was empty already.
            !(handlers.is_empty() && before > 0)
        });
        if list.is_empty() && before > 0 {
            emptied.push(event.clone());
        }
    }
    for event in emptied {
        hooks.remove(&event);
    }
    if hooks.is_empty() {
        root.remove("hooks");
    }
    settings
}

/// An entry `ofa setup` wrote: it runs some `ofa.exe` with the single
/// argument `hook`, wherever that ofa.exe lives.
fn is_ofa_hook(handler: &Value) -> bool {
    let command = handler.get("command").and_then(Value::as_str);
    let is_ofa = command
        .and_then(|c| Path::new(c).file_name())
        .is_some_and(|name| name.eq_ignore_ascii_case("ofa.exe"));
    is_ofa && handler.get("args") == Some(&json!(["hook"]))
}

fn write_backup(path: &Path, text: &str) -> Result<PathBuf, String> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let name = format!(
        "{}.ofa-backup-{stamp}",
        path.file_name().unwrap_or_default().to_string_lossy()
    );
    let backup = path.with_file_name(name);
    fs::write(&backup, text)
        .map_err(|err| format!("could not write the backup {}: {err}", backup.display()))?;
    Ok(backup)
}

/// Writes next to the file and renames over it, so Claude Code never reads
/// a half-written settings file.
fn write_atomically(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|err| err.to_string())?;
    }
    let temp = path.with_extension("json.ofa-tmp");
    fs::write(&temp, text).map_err(|err| format!("could not write {}: {err}", temp.display()))?;
    fs::rename(&temp, path).map_err(|err| {
        let _ = fs::remove_file(&temp);
        format!("could not replace {}: {err}", path.display())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = "C:\\Users\\me\\AppData\\Local\\OFA\\ofa.exe";

    fn ofa_handler() -> Value {
        json!({"type": "command", "command": EXE, "args": ["hook"], "timeout": TIMEOUT_SECS})
    }

    #[test]
    fn adds_one_hook_per_event_to_empty_settings() {
        let out = add_hooks(json!({}), EXE);
        let hooks = out["hooks"].as_object().unwrap();
        assert_eq!(hooks.len(), EVENTS.len());
        for event in EVENTS {
            assert_eq!(out["hooks"][event], json!([{"hooks": [ofa_handler()]}]));
        }
    }

    #[test]
    fn keeps_your_settings_and_hooks_in_their_order() {
        let mine = json!({"matcher": "Bash", "hooks": [{"type": "command", "command": "lint.sh"}]});
        let before = json!({
            "model": "opus",
            "hooks": {"PreToolUse": [mine.clone()], "PreCompact": [mine.clone()]},
            "theme": "dark"
        });
        let out = add_hooks(before, EXE);
        let keys: Vec<_> = out.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, ["model", "hooks", "theme"]);
        assert_eq!(out["hooks"]["PreToolUse"][0], mine);
        assert_eq!(
            out["hooks"]["PreToolUse"][1],
            json!({"hooks": [ofa_handler()]})
        );
        assert_eq!(out["hooks"]["PreCompact"], json!([mine]));
    }

    #[test]
    fn running_setup_twice_changes_nothing() {
        let once = add_hooks(json!({"model": "opus"}), EXE);
        assert_eq!(add_hooks(once.clone(), EXE), once);
    }

    #[test]
    fn a_moved_ofa_exe_replaces_the_old_entry() {
        let old = add_hooks(json!({}), "C:\\old\\place\\ofa.exe");
        let new = add_hooks(old, EXE);
        assert_eq!(new["hooks"]["Stop"], json!([{"hooks": [ofa_handler()]}]));
    }

    #[test]
    fn uninstall_restores_the_original() {
        let original = json!({
            "model": "opus",
            "hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "lint.sh"}]}]}
        });
        let installed = add_hooks(original.clone(), EXE);
        assert_eq!(remove_hooks(installed), original);
        assert_eq!(
            remove_hooks(add_hooks(json!({"a": 1}), EXE)),
            json!({"a": 1})
        );
    }

    #[test]
    fn uninstall_leaves_other_handlers_in_a_shared_group() {
        let shared = json!({"hooks": {"Stop": [{"hooks": [
            {"type": "command", "command": "notify.exe"},
            ofa_handler()
        ]}]}});
        let out = remove_hooks(shared);
        assert_eq!(
            out,
            json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "notify.exe"}]}]}})
        );
    }

    #[test]
    fn empty_lists_ofa_never_touched_are_kept() {
        let mine = json!({"model": "opus", "hooks": {"PreCompact": []}});
        assert_eq!(remove_hooks(add_hooks(mine.clone(), EXE)), mine);
    }

    #[test]
    fn other_programs_named_like_ofa_are_left_alone() {
        let other = json!({"hooks": {"Stop": [{"hooks": [
            {"type": "command", "command": EXE, "args": ["run", "x"]},
            {"type": "command", "command": "C:\\tools\\sofa.exe", "args": ["hook"]}
        ]}]}});
        assert_eq!(remove_hooks(other.clone()), other);
    }

    #[test]
    fn run_backs_up_writes_and_reverts_a_real_file() {
        let dir = std::env::temp_dir().join(format!("ofa-setup-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        let original = "{\n  \"model\": \"opus\"\n}\n";
        fs::write(&path, original).unwrap();

        let done = run(&path, Path::new(EXE), false).unwrap();
        assert!(done.changed);
        assert_eq!(fs::read_to_string(done.backup.unwrap()).unwrap(), original);
        let written: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            written["hooks"]["SessionStart"][0]["hooks"][0],
            ofa_handler()
        );

        let again = run(&path, Path::new(EXE), false).unwrap();
        assert!(!again.changed);
        assert!(again.backup.is_none());

        run(&path, Path::new(EXE), true).unwrap();
        let reverted: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(reverted, json!({"model": "opus"}));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn invalid_json_is_never_overwritten() {
        let dir = std::env::temp_dir().join(format!("ofa-setup-bad-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        fs::write(&path, "{ not json").unwrap();
        assert!(run(&path, Path::new(EXE), false).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "{ not json");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_file_is_created_without_a_backup() {
        let dir = std::env::temp_dir().join(format!("ofa-setup-new-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("settings.json");
        let done = run(&path, Path::new(EXE), false).unwrap();
        assert!(done.changed && done.backup.is_none());
        assert!(path.exists());
        fs::remove_dir_all(&dir).unwrap();
    }
}
