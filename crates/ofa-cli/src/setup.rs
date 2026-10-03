//! `ofa setup`: adds OFA's hooks to Claude Code's user settings and to
//! Codex's hooks file, or removes them again with `--uninstall`.
//!
//! These files may already hold hooks and settings you rely on, so this only
//! ever adds or removes OFA's own entries, keeps everything else exactly as
//! it was (including key order), and takes a backup before writing.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

/// The agents whose hooks `ofa setup` manages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// `~/.claude/settings.json`, where hooks live next to other settings.
    Claude,
    /// `~/.codex/hooks.json`, which holds only hooks.
    Codex,
}

impl Target {
    pub fn name(self) -> &'static str {
        match self {
            Target::Claude => "Claude Code",
            Target::Codex => "Codex",
        }
    }

    /// The file to change: `CLAUDE_CONFIG_DIR` / `CODEX_HOME` if set, else
    /// the folder in your user profile.
    pub fn default_path(self) -> Option<PathBuf> {
        let (env, folder, file) = match self {
            Target::Claude => ("CLAUDE_CONFIG_DIR", ".claude", "settings.json"),
            Target::Codex => ("CODEX_HOME", ".codex", "hooks.json"),
        };
        if let Some(dir) = std::env::var_os(env) {
            return Some(PathBuf::from(dir).join(file));
        }
        let home = std::env::var_os("USERPROFILE")?;
        Some(PathBuf::from(home).join(folder).join(file))
    }

    /// The hook events `ofa hook` follows for this agent.
    fn events(self) -> &'static [&'static str] {
        match self {
            Target::Claude => &[
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
            ],
            Target::Codex => &[
                "SessionStart",
                "UserPromptSubmit",
                "PreToolUse",
                "PermissionRequest",
                "PostToolUse",
                "Stop",
                "Interrupt",
                "SessionEnd",
            ],
        }
    }

    /// One hook entry that runs `ofa.exe hook` for `event`. Claude Code takes
    /// the program and its arguments separately; Codex takes one command line.
    fn handler(self, exe: &str, event: &str) -> Value {
        let timeout = timeout_for(event);
        match self {
            Target::Claude => json!({
                "type": "command",
                "command": exe,
                "args": ["hook"],
                "timeout": timeout
            }),
            Target::Codex => {
                let program = if exe.contains(' ') {
                    format!("\"{exe}\"")
                } else {
                    exe.to_owned()
                };
                json!({
                    "type": "command",
                    "command": format!("{program} hook --agent codex"),
                    "timeout": timeout
                })
            }
        }
    }
}

/// Seconds an agent waits for a hook. `ofa hook` finishes in well under one,
/// except on a permission prompt, where it waits for your answer on the
/// island while the terminal shows its own prompt.
const TIMEOUT_SECS: u64 = 5;
const PERMISSION_TIMEOUT_SECS: u64 = 60 * 60;

fn timeout_for(event: &str) -> u64 {
    if event == "PermissionRequest" {
        PERMISSION_TIMEOUT_SECS
    } else {
        TIMEOUT_SECS
    }
}

/// What `run` did, for the message printed afterwards.
pub struct Outcome {
    pub backup: Option<PathBuf>,
    pub changed: bool,
}

/// Installs or removes `target`'s hooks in the file at `path`.
pub fn run(target: Target, path: &Path, exe: &Path, uninstall: bool) -> Result<Outcome, String> {
    let original = match fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => return Err(format!("could not read {}: {err}", path.display())),
    };
    let settings: Value = match &original {
        // Some editors leave a byte order mark; the agents accept it, so do we.
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
        add_hooks(target, settings.clone(), exe)
    };
    if updated == settings {
        return Ok(Outcome {
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
        backup,
        changed: true,
    })
}

/// Adds one hook group per event that runs `<exe> hook`, replacing any OFA
/// entries already there so running setup twice changes nothing.
fn add_hooks(target: Target, settings: Value, exe: &str) -> Value {
    let mut settings = remove_hooks(settings);
    let root = settings.as_object_mut().expect("checked to be an object");
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(hooks) = hooks.as_object_mut() else {
        // "hooks" exists but isn't an object; the agent would reject it too.
        return settings;
    };
    for &event in target.events() {
        let groups = hooks
            .entry(event)
            .or_insert_with(|| Value::Array(Vec::new()));
        if let Some(groups) = groups.as_array_mut() {
            groups.push(json!({ "hooks": [target.handler(exe, event)] }));
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

/// An entry `ofa setup` wrote: it runs some `ofa.exe hook`, wherever that
/// ofa.exe lives, either as a program plus `args` (Claude Code) or as one
/// command line (Codex).
fn is_ofa_hook(handler: &Value) -> bool {
    let Some(command) = handler.get("command").and_then(Value::as_str) else {
        return false;
    };
    let (program, rest) = split_command(command);
    let is_ofa = Path::new(program)
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("ofa.exe"));
    let runs_hook = match handler.get("args") {
        Some(args) => args == &json!(["hook"]),
        None => rest == "hook" || rest.starts_with("hook "),
    };
    is_ofa && runs_hook
}

/// Splits a command line into its program (quoted or not) and the rest.
fn split_command(command: &str) -> (&str, &str) {
    let command = command.trim();
    if let Some(quoted) = command.strip_prefix('"') {
        if let Some(end) = quoted.find('"') {
            return (&quoted[..end], quoted[end + 1..].trim_start());
        }
    }
    match command.split_once(' ') {
        Some((program, rest)) => (program, rest.trim_start()),
        None => (command, ""),
    }
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

/// Writes next to the file and renames over it, so an agent never reads a
/// half-written file.
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

    fn ofa_handler_for(event: &str) -> Value {
        json!({"type": "command", "command": EXE, "args": ["hook"], "timeout": timeout_for(event)})
    }

    #[test]
    fn adds_one_hook_per_event_to_empty_settings() {
        let out = add_hooks(Target::Claude, json!({}), EXE);
        let hooks = out["hooks"].as_object().unwrap();
        assert_eq!(hooks.len(), Target::Claude.events().len());
        for &event in Target::Claude.events() {
            assert_eq!(
                out["hooks"][event],
                json!([{"hooks": [ofa_handler_for(event)]}])
            );
        }
        assert_eq!(
            out["hooks"]["PermissionRequest"][0]["hooks"][0]["timeout"],
            PERMISSION_TIMEOUT_SECS
        );
    }

    #[test]
    fn keeps_your_settings_and_hooks_in_their_order() {
        let mine = json!({"matcher": "Bash", "hooks": [{"type": "command", "command": "lint.sh"}]});
        let before = json!({
            "model": "opus",
            "hooks": {"PreToolUse": [mine.clone()], "PreCompact": [mine.clone()]},
            "theme": "dark"
        });
        let out = add_hooks(Target::Claude, before, EXE);
        let keys: Vec<_> = out.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, ["model", "hooks", "theme"]);
        assert_eq!(out["hooks"]["PreToolUse"][0], mine);
        assert_eq!(
            out["hooks"]["PreToolUse"][1],
            json!({"hooks": [ofa_handler_for("PreToolUse")]})
        );
        assert_eq!(out["hooks"]["PreCompact"], json!([mine]));
    }

    #[test]
    fn running_setup_twice_changes_nothing() {
        let once = add_hooks(Target::Claude, json!({"model": "opus"}), EXE);
        assert_eq!(add_hooks(Target::Claude, once.clone(), EXE), once);
    }

    #[test]
    fn a_moved_ofa_exe_replaces_the_old_entry() {
        let old = add_hooks(Target::Claude, json!({}), "C:\\old\\place\\ofa.exe");
        let new = add_hooks(Target::Claude, old, EXE);
        assert_eq!(new["hooks"]["Stop"], json!([{"hooks": [ofa_handler()]}]));
    }

    #[test]
    fn uninstall_restores_the_original() {
        let original = json!({
            "model": "opus",
            "hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "lint.sh"}]}]}
        });
        let installed = add_hooks(Target::Claude, original.clone(), EXE);
        assert_eq!(remove_hooks(installed), original);
        assert_eq!(
            remove_hooks(add_hooks(Target::Claude, json!({"a": 1}), EXE)),
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
        assert_eq!(
            remove_hooks(add_hooks(Target::Claude, mine.clone(), EXE)),
            mine
        );
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
    fn codex_gets_one_command_line_per_event() {
        let out = add_hooks(Target::Codex, json!({}), EXE);
        assert_eq!(
            out["hooks"]["Interrupt"],
            json!([{"hooks": [{
                "type": "command",
                "command": format!("{EXE} hook --agent codex"),
                "timeout": TIMEOUT_SECS
            }]}])
        );
        assert_eq!(
            out["hooks"]["PermissionRequest"][0]["hooks"][0]["timeout"],
            PERMISSION_TIMEOUT_SECS
        );
        assert!(out["hooks"].get("Notification").is_none());
    }

    #[test]
    fn a_path_with_spaces_is_quoted_for_codex() {
        let out = add_hooks(Target::Codex, json!({}), "C:\\Program Files\\OFA\\ofa.exe");
        assert_eq!(
            out["hooks"]["Stop"][0]["hooks"][0]["command"],
            "\"C:\\Program Files\\OFA\\ofa.exe\" hook --agent codex"
        );
        assert_eq!(remove_hooks(out), json!({}));
    }

    #[test]
    fn codex_uninstall_restores_the_original() {
        let original =
            json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "notify.exe"}]}]}});
        let installed = add_hooks(Target::Codex, original.clone(), EXE);
        assert_eq!(add_hooks(Target::Codex, installed.clone(), EXE), installed);
        assert_eq!(remove_hooks(installed), original);
    }

    #[test]
    fn command_lines_split_into_program_and_rest() {
        assert_eq!(
            split_command("C:\\ofa.exe hook --agent codex"),
            ("C:\\ofa.exe", "hook --agent codex")
        );
        assert_eq!(
            split_command("\"C:\\a b\\ofa.exe\" hook"),
            ("C:\\a b\\ofa.exe", "hook")
        );
        assert_eq!(split_command("ofa.exe"), ("ofa.exe", ""));
    }

    #[test]
    fn run_backs_up_writes_and_reverts_a_real_file() {
        let dir = std::env::temp_dir().join(format!("ofa-setup-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        let original = "{\n  \"model\": \"opus\"\n}\n";
        fs::write(&path, original).unwrap();

        let done = run(Target::Claude, &path, Path::new(EXE), false).unwrap();
        assert!(done.changed);
        assert_eq!(fs::read_to_string(done.backup.unwrap()).unwrap(), original);
        let written: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            written["hooks"]["SessionStart"][0]["hooks"][0],
            ofa_handler()
        );

        let again = run(Target::Claude, &path, Path::new(EXE), false).unwrap();
        assert!(!again.changed);
        assert!(again.backup.is_none());

        run(Target::Claude, &path, Path::new(EXE), true).unwrap();
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
        assert!(run(Target::Claude, &path, Path::new(EXE), false).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "{ not json");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_file_is_created_without_a_backup() {
        let dir = std::env::temp_dir().join(format!("ofa-setup-new-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("settings.json");
        let done = run(Target::Claude, &path, Path::new(EXE), false).unwrap();
        assert!(done.changed && done.backup.is_none());
        assert!(path.exists());
        fs::remove_dir_all(&dir).unwrap();
    }
}
