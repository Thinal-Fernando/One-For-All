//! Turns an agent's hook input into OFA events.
//!
//! Claude Code and Codex both run `ofa hook` for each hook event and write
//! one JSON object to its stdin, with the same event and field names. The
//! names here were checked against Claude Code 2.1.193 and Codex's hook
//! documentation; anything unknown is ignored rather than treated as an error.

use std::path::Path;

use ofa_protocol::{Decision, Event, EventKind, Source};
use serde::Deserialize;
use serde_json::{json, Value};

/// The parts of a hook's input OFA uses. Every event carries `session_id`,
/// `cwd` and `hook_event_name`; the rest depend on the event.
#[derive(Debug, Deserialize)]
pub struct HookInput {
    pub session_id: String,
    pub hook_event_name: String,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub transcript_path: Option<String>,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub tool_input: Option<Value>,
    #[serde(default)]
    pub notification_type: Option<String>,
    #[serde(default)]
    pub error_type: Option<String>,
}

/// Longest detail line sent to the island.
const MAX_DETAIL: usize = 80;

/// The event to send, or `None` for hook events OFA doesn't follow.
pub fn to_event(input: HookInput, source: Source, pid: Option<u32>) -> Option<Event> {
    let working = |detail: &str| EventKind::Working {
        detail: Some(detail.to_owned()),
    };
    let kind = match input.hook_event_name.as_str() {
        "SessionStart" => EventKind::SessionStarted,
        "UserPromptSubmit" => working("Thinking"),
        "PreToolUse" => EventKind::Working {
            detail: Some(describe_tool(
                input.tool_name.as_deref().unwrap_or("a tool"),
                input.tool_input.as_ref(),
            )),
        },
        // The tool ran or was refused, so any prompt has been answered.
        "PostToolUse" | "PostToolUseFailure" | "PermissionDenied" => working("Thinking"),
        // Runs the moment the permission prompt appears. OFA never answers it
        // (no output), so the terminal shows its prompt as usual. It doesn't
        // say what is waiting, so the island keeps the command from the
        // PreToolUse event just before.
        "PermissionRequest" => EventKind::NeedsYou { detail: None },
        "Notification" => match input.notification_type.as_deref() {
            // Claude Code sends "permission_prompt" only after the prompt has
            // waited a few seconds, so PermissionRequest is used instead.
            Some("elicitation_dialog" | "elicitation_url_dialog") => EventKind::NeedsYou {
                detail: Some("Waiting for your answer".into()),
            },
            _ => return None,
        },
        "Stop" => EventKind::TurnFinished,
        // Codex reports Esc; Claude Code doesn't, so the app watches its
        // transcript instead.
        "Interrupt" => EventKind::Interrupted,
        "StopFailure" => EventKind::Failed {
            detail: Some(describe_error(input.error_type.as_deref())),
        },
        "SessionEnd" => EventKind::SessionEnded,
        _ => return None,
    };

    Some(Event {
        source,
        session_id: input.session_id,
        pid,
        title: input.cwd.as_deref().and_then(folder_name),
        transcript_path: input.transcript_path,
        kind,
    })
}

/// What `ofa hook` prints to answer a `PermissionRequest`, in the shape both
/// Claude Code and Codex read from a hook's stdout.
pub fn permission_reply(decision: Decision) -> String {
    let decision = match decision {
        Decision::Allow => json!({"behavior": "allow"}),
        Decision::Deny => json!({"behavior": "deny", "message": "Denied from the OFA island."}),
    };
    json!({
        "hookSpecificOutput": {
            "hookEventName": "PermissionRequest",
            "decision": decision
        }
    })
    .to_string()
}

/// "one-for-all" from "C:\Users\me\project\one-for-all".
fn folder_name(cwd: &str) -> Option<String> {
    Path::new(cwd)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
}

/// One short line for the island about what a tool is doing.
fn describe_tool(tool: &str, input: Option<&Value>) -> String {
    let value = |name: &str| input.and_then(|i| i.get(name));
    let field = |name: &str| value(name).and_then(Value::as_str);
    let file = || {
        field("file_path")
            .or_else(|| field("notebook_path"))
            .and_then(folder_name)
            .unwrap_or_else(|| "a file".into())
    };
    let line = match tool {
        // Codex can pass a command as a list of words.
        "Bash" | "PowerShell" | "shell" | "local_shell" | "exec_command" => value("command")
            .and_then(command_line)
            .unwrap_or(tool.into()),
        // Codex's file edits arrive as a patch, which names each file.
        "apply_patch" => field("command")
            .and_then(patched_file)
            .map(|file| format!("Editing {file}"))
            .unwrap_or_else(|| "Editing files".into()),
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => format!("Editing {}", file()),
        "Read" => format!("Reading {}", file()),
        "Grep" | "Glob" => "Searching the code".into(),
        "WebFetch" | "WebSearch" => "Searching the web".into(),
        "Task" | "Agent" => "Running a subagent".into(),
        other => other.into(),
    };
    shorten(&line)
}

fn describe_error(error_type: Option<&str>) -> String {
    match error_type {
        Some("rate_limit") => "Hit the rate limit".into(),
        Some("overloaded") => "Claude is overloaded".into(),
        Some("authentication_failed") => "Sign-in failed".into(),
        Some("billing_error") => "Billing problem".into(),
        Some("max_output_tokens") => "Reply was too long".into(),
        Some("server_error") => "Server error".into(),
        Some(other) => format!("Stopped: {}", other.replace('_', " ")),
        None => "Stopped with an error".into(),
    }
}

/// The first line of a command given as text or as a list of words.
fn command_line(command: &Value) -> Option<String> {
    match command {
        Value::String(text) => Some(first_line(text)),
        Value::Array(words) => {
            let words: Vec<&str> = words.iter().filter_map(Value::as_str).collect();
            Some(first_line(&words.join(" ")))
        }
        _ => None,
    }
}

/// The first file a patch touches: "*** Update File: src/main.rs" gives "main.rs".
fn patched_file(patch: &str) -> Option<String> {
    patch.lines().find_map(|line| {
        ["*** Update File: ", "*** Add File: ", "*** Delete File: "]
            .iter()
            .find_map(|prefix| line.strip_prefix(prefix))
            .and_then(|path| folder_name(path.trim()))
    })
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or("").trim().to_owned()
}

fn shorten(text: &str) -> String {
    if text.chars().count() <= MAX_DETAIL {
        return text.to_owned();
    }
    let cut: String = text.chars().take(MAX_DETAIL - 1).collect();
    format!("{cut}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds hook input shaped like what Claude Code 2.1.193 really sent.
    fn input(extra: Value) -> HookInput {
        let mut base = json!({
            "session_id": "a3b53514-721f-404a-9bd0-ce46a2fa0cd0",
            "transcript_path": "C:\\Users\\me\\.claude\\projects\\x\\a3b5.jsonl",
            "cwd": "C:\\Users\\me\\project\\one-for-all",
            "permission_mode": "default"
        });
        base.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        serde_json::from_value(base).unwrap()
    }

    fn kind(extra: Value) -> Option<EventKind> {
        to_event(input(extra), Source::ClaudeCode, None).map(|e| e.kind)
    }

    #[test]
    fn session_start_carries_id_pid_and_folder() {
        let event = to_event(
            input(json!({"hook_event_name": "SessionStart", "source": "startup"})),
            Source::ClaudeCode,
            Some(18516),
        )
        .unwrap();
        assert_eq!(event.source, Source::ClaudeCode);
        assert_eq!(event.session_id, "a3b53514-721f-404a-9bd0-ce46a2fa0cd0");
        assert_eq!(event.pid, Some(18516));
        assert_eq!(event.title.as_deref(), Some("one-for-all"));
        assert!(event
            .transcript_path
            .is_some_and(|p| p.ends_with("a3b5.jsonl")));
        assert_eq!(event.kind, EventKind::SessionStarted);
    }

    #[test]
    fn a_shell_command_shows_its_first_line() {
        let k = kind(json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": "npm test\nnpm run lint", "description": "Run tests"},
            "tool_use_id": "toolu_1"
        }));
        assert_eq!(
            k,
            Some(EventKind::Working {
                detail: Some("npm test".into())
            })
        );
    }

    #[test]
    fn file_tools_show_the_file_name() {
        let k = kind(json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Edit",
            "tool_input": {"file_path": "C:\\Users\\me\\project\\src\\island.rs"}
        }));
        assert_eq!(
            k,
            Some(EventKind::Working {
                detail: Some("Editing island.rs".into())
            })
        );
    }

    #[test]
    fn long_details_are_cut() {
        let long = "x".repeat(200);
        let k = kind(json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": long}
        }));
        let Some(EventKind::Working { detail: Some(d) }) = k else {
            panic!("expected working");
        };
        assert_eq!(d.chars().count(), MAX_DETAIL);
        assert!(d.ends_with('…'));
    }

    #[test]
    fn a_permission_prompt_needs_you() {
        let k = kind(json!({
            "hook_event_name": "PermissionRequest",
            "tool_name": "Write",
            "tool_input": {"file_path": "test2", "content": ""},
            "tool_use_id": "toolu_1"
        }));
        assert_eq!(k, Some(EventKind::NeedsYou { detail: None }));
    }

    #[test]
    fn replies_in_the_shape_claude_code_reads() {
        let allow: Value = serde_json::from_str(&permission_reply(Decision::Allow)).unwrap();
        assert_eq!(
            allow,
            json!({"hookSpecificOutput": {
                "hookEventName": "PermissionRequest",
                "decision": {"behavior": "allow"}
            }})
        );
        let deny: Value = serde_json::from_str(&permission_reply(Decision::Deny)).unwrap();
        assert_eq!(deny["hookSpecificOutput"]["decision"]["behavior"], "deny");
        assert!(deny["hookSpecificOutput"]["decision"]["message"].is_string());
    }

    #[test]
    fn idle_and_other_notifications_are_ignored() {
        for t in [
            "permission_prompt",
            "idle_prompt",
            "auth_success",
            "agent_completed",
        ] {
            let k = kind(json!({"hook_event_name": "Notification", "notification_type": t}));
            assert_eq!(k, None, "{t}");
        }
    }

    #[test]
    fn a_turn_moves_through_working_to_finished() {
        let prompt = kind(json!({"hook_event_name": "UserPromptSubmit", "prompt": "hi"}));
        assert!(matches!(prompt, Some(EventKind::Working { .. })));
        let after = kind(json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_response": {"stdout": "hi"},
            "duration_ms": 3015
        }));
        assert!(matches!(after, Some(EventKind::Working { .. })));
        let stop = kind(json!({"hook_event_name": "Stop", "stop_hook_active": false}));
        assert_eq!(stop, Some(EventKind::TurnFinished));
        let end = kind(json!({"hook_event_name": "SessionEnd", "reason": "other"}));
        assert_eq!(end, Some(EventKind::SessionEnded));
    }

    #[test]
    fn a_failed_turn_says_why() {
        let k = kind(json!({"hook_event_name": "StopFailure", "error_type": "rate_limit"}));
        assert_eq!(
            k,
            Some(EventKind::Failed {
                detail: Some("Hit the rate limit".into())
            })
        );
    }

    #[test]
    fn codex_events_carry_the_codex_source() {
        let event = to_event(
            input(json!({"hook_event_name": "UserPromptSubmit", "turn_id": "t1", "prompt": "hi"})),
            Source::Codex,
            Some(7),
        )
        .unwrap();
        assert_eq!(event.source, Source::Codex);
        assert_eq!(event.pid, Some(7));
    }

    #[test]
    fn esc_in_codex_is_an_interruption() {
        assert_eq!(
            kind(json!({"hook_event_name": "Interrupt"})),
            Some(EventKind::Interrupted)
        );
    }

    #[test]
    fn a_command_given_as_words_is_joined() {
        let k = kind(json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "shell",
            "tool_input": {"command": ["powershell.exe", "-Command", "npm test"]}
        }));
        assert_eq!(
            k,
            Some(EventKind::Working {
                detail: Some("powershell.exe -Command npm test".into())
            })
        );
    }

    #[test]
    fn a_patch_shows_the_file_it_edits() {
        let k = kind(json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "apply_patch",
            "tool_input": {"command": "*** Begin Patch\n*** Update File: src/island.rs\n@@\n-a\n+b\n*** End Patch"}
        }));
        assert_eq!(
            k,
            Some(EventKind::Working {
                detail: Some("Editing island.rs".into())
            })
        );
    }

    #[test]
    fn unfollowed_events_send_nothing() {
        assert_eq!(kind(json!({"hook_event_name": "PreCompact"})), None);
        assert_eq!(kind(json!({"hook_event_name": "SomethingNew"})), None);
    }
}
