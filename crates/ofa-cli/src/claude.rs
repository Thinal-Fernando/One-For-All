//! Turns Claude Code's hook input into OFA events.
//!
//! Claude Code runs `ofa hook` for each hook event and writes one JSON object
//! to its stdin. The field names here were checked against Claude Code
//! 2.1.193; anything unknown is ignored rather than treated as an error.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use ofa_protocol::{Decision, Event, EventKind, Request, RequestFormat, Source, MAX_REQUEST_BYTES};
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

/// Longest reason sent with a permission request, in characters.
const MAX_REASON: usize = 1500;

/// How much of the end of the transcript is searched for Claude's reason.
const REASON_SEARCH_BYTES: u64 = 256 * 1024;

/// The event to send, or `None` for hook events OFA doesn't follow.
pub fn to_event(input: HookInput, pid: Option<u32>) -> Option<Event> {
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
        // Runs the moment the permission prompt appears, and `ofa hook`
        // waits for an answer from the island while the terminal shows its
        // own prompt as usual. The short line stays the command from the
        // PreToolUse event just before; the whole request goes along so the
        // island can show it in full.
        "PermissionRequest" => EventKind::NeedsYou {
            detail: None,
            request: Some(describe_request(
                input.tool_name.as_deref().unwrap_or("a tool"),
                input.tool_input.as_ref(),
                input.transcript_path.as_deref().map(Path::new),
            )),
        },
        "Notification" => match input.notification_type.as_deref() {
            // Claude Code sends "permission_prompt" only after the prompt has
            // waited a few seconds, so PermissionRequest is used instead.
            Some("elicitation_dialog" | "elicitation_url_dialog") => EventKind::NeedsYou {
                detail: Some("Waiting for your answer".into()),
                request: None,
            },
            _ => return None,
        },
        "Stop" => EventKind::TurnFinished,
        "StopFailure" => EventKind::Failed {
            detail: Some(describe_error(input.error_type.as_deref())),
        },
        "SessionEnd" => EventKind::SessionEnded,
        _ => return None,
    };

    Some(Event {
        source: Source::ClaudeCode,
        session_id: input.session_id,
        pid,
        title: input.cwd.as_deref().and_then(folder_name),
        transcript_path: input.transcript_path,
        kind,
    })
}

/// What `ofa hook` prints to answer a `PermissionRequest`, in the shape
/// Claude Code reads from a hook's stdout.
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
    let field = |name: &str| input.and_then(|i| i.get(name)).and_then(Value::as_str);
    let file = || {
        field("file_path")
            .or_else(|| field("notebook_path"))
            .and_then(folder_name)
            .unwrap_or_else(|| "a file".into())
    };
    let line = match tool {
        "Bash" | "PowerShell" => field("command").map(first_line).unwrap_or(tool.into()),
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => format!("Editing {}", file()),
        "Read" => format!("Reading {}", file()),
        "Grep" | "Glob" => "Searching the code".into(),
        "WebFetch" | "WebSearch" => "Searching the web".into(),
        "Task" | "Agent" => "Running a subagent".into(),
        other => other.into(),
    };
    shorten(&line)
}

/// Everything a permission prompt asks for: the whole command, or the
/// change to a file as diff lines, and Claude's reason.
fn describe_request(tool: &str, input: Option<&Value>, transcript: Option<&Path>) -> Request {
    let field = |name: &str| input.and_then(|i| i.get(name)).and_then(Value::as_str);
    let (format, body) = match tool {
        "Bash" | "PowerShell" => (
            RequestFormat::Command,
            field("command").unwrap_or_default().to_owned(),
        ),
        "Edit" => (
            RequestFormat::Diff,
            diff(
                field("old_string").unwrap_or_default(),
                field("new_string").unwrap_or_default(),
            ),
        ),
        "MultiEdit" => {
            let edits = input
                .and_then(|i| i.get("edits"))
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let parts: Vec<String> = edits
                .iter()
                .map(|edit| {
                    let text = |name: &str| edit.get(name).and_then(Value::as_str).unwrap_or("");
                    diff(text("old_string"), text("new_string"))
                })
                .collect();
            (RequestFormat::Diff, parts.join("\n@@\n"))
        }
        "Write" => (
            RequestFormat::Diff,
            diff("", field("content").unwrap_or_default()),
        ),
        "NotebookEdit" => (
            RequestFormat::Diff,
            diff("", field("new_source").unwrap_or_default()),
        ),
        _ => (
            RequestFormat::Text,
            input
                .and_then(|i| serde_json::to_string_pretty(i).ok())
                .unwrap_or_default(),
        ),
    };
    let (body, truncated) = cut_to(body, MAX_REQUEST_BYTES);
    // What Claude wrote just before the request says why; a shell command
    // also carries a short description of its own.
    let reason = transcript
        .and_then(last_words)
        .or_else(|| field("description").map(str::to_owned))
        .map(|text| cut_chars(text.trim(), MAX_REASON))
        .filter(|text| !text.is_empty());
    Request {
        tool: tool.to_owned(),
        file: field("file_path")
            .or_else(|| field("notebook_path"))
            .map(str::to_owned),
        format,
        body,
        truncated,
        reason,
    }
}

/// `old` as removed lines followed by `new` as added ones.
fn diff(old: &str, new: &str) -> String {
    let removed = old.lines().map(|line| format!("-{line}"));
    let added = new.lines().map(|line| format!("+{line}"));
    removed.chain(added).collect::<Vec<_>>().join("\n")
}

/// The text Claude wrote alongside the tool call that is now asking, read
/// from the end of the transcript: whatever it said after the last thing you
/// or a tool sent it. `None` if it went straight to the tool.
fn last_words(transcript: &Path) -> Option<String> {
    let mut file = File::open(transcript).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(REASON_SEARCH_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    // Starting mid-file means the first line is only part of one.
    let lines: Vec<&str> = text.lines().skip(usize::from(start > 0)).collect();

    let mut said = Vec::new();
    for entry in lines
        .into_iter()
        .rev()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|entry| entry["isSidechain"] != true)
    {
        match entry["type"].as_str() {
            Some("user") => break,
            Some("assistant") => {
                let texts: Vec<&str> = entry["message"]["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|part| part["type"] == "text")
                    .filter_map(|part| part["text"].as_str())
                    .collect();
                if !texts.is_empty() {
                    said.push(texts.join("\n"));
                }
            }
            _ => {}
        }
    }
    said.reverse();
    let said = said.join("\n\n");
    (!said.trim().is_empty()).then_some(said)
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

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or("").trim().to_owned()
}

fn shorten(text: &str) -> String {
    cut_chars(text, MAX_DETAIL)
}

fn cut_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let cut: String = text.chars().take(max - 1).collect();
    format!("{cut}…")
}

/// `text` cut to at most `max` bytes on a character boundary, and whether
/// anything was cut.
fn cut_to(mut text: String, max: usize) -> (String, bool) {
    if text.len() <= max {
        return (text, false);
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    (text, true)
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
        to_event(input(extra), None).map(|e| e.kind)
    }

    #[test]
    fn session_start_carries_id_pid_and_folder() {
        let event = to_event(
            input(json!({"hook_event_name": "SessionStart", "source": "startup"})),
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

    fn request(extra: Value) -> Request {
        let mut extra = extra;
        // The test transcript path doesn't exist, so leave it out entirely.
        extra["transcript_path"] = Value::Null;
        match kind(extra) {
            Some(EventKind::NeedsYou {
                detail: None,
                request: Some(request),
            }) => request,
            other => panic!("expected a permission request, got {other:?}"),
        }
    }

    #[test]
    fn a_permission_prompt_carries_the_whole_command() {
        let r = request(json!({
            "hook_event_name": "PermissionRequest",
            "tool_name": "Bash",
            "tool_input": {"command": "npm test\nnpm run lint", "description": "Run the checks"}
        }));
        assert_eq!(r.tool, "Bash");
        assert_eq!(r.format, RequestFormat::Command);
        assert_eq!(r.body, "npm test\nnpm run lint");
        assert!(!r.truncated);
        assert_eq!(r.reason.as_deref(), Some("Run the checks"));
    }

    #[test]
    fn file_changes_show_as_diff_lines() {
        let r = request(json!({
            "hook_event_name": "PermissionRequest",
            "tool_name": "Edit",
            "tool_input": {
                "file_path": "C:\\code\\island.rs",
                "old_string": "let a = 1;\nlet b = 2;",
                "new_string": "let a = 3;"
            }
        }));
        assert_eq!(r.format, RequestFormat::Diff);
        assert_eq!(r.file.as_deref(), Some("C:\\code\\island.rs"));
        assert_eq!(r.body, "-let a = 1;\n-let b = 2;\n+let a = 3;");
        assert_eq!(r.reason, None);

        let w = request(json!({
            "hook_event_name": "PermissionRequest",
            "tool_name": "Write",
            "tool_input": {"file_path": "test2", "content": "one\ntwo"}
        }));
        assert_eq!(w.body, "+one\n+two");
    }

    #[test]
    fn other_tools_show_their_input() {
        let r = request(json!({
            "hook_event_name": "PermissionRequest",
            "tool_name": "WebFetch",
            "tool_input": {"url": "https://example.com"}
        }));
        assert_eq!(r.format, RequestFormat::Text);
        assert!(r.body.contains("https://example.com"));
    }

    #[test]
    fn a_huge_request_is_cut_on_a_character_boundary() {
        let r = request(json!({
            "hook_event_name": "PermissionRequest",
            "tool_name": "Bash",
            "tool_input": {"command": "é".repeat(MAX_REQUEST_BYTES)}
        }));
        assert!(r.truncated);
        assert!(r.body.len() <= MAX_REQUEST_BYTES);
        assert!(r.body.chars().all(|c| c == 'é'));
    }

    #[test]
    fn the_reason_is_what_claude_said_since_the_last_tool_result() {
        let path = std::env::temp_dir().join(format!("ofa-reason-{}.jsonl", std::process::id()));
        let lines = [
            r#"{"type":"user","message":{"role":"user","content":"tidy up"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Old reason."}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"ok"}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Removing build frees space."}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{}}]}}"#,
        ];
        let input = json!({"command": "rm -r build", "description": "Remove build"});

        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        let r = describe_request("Bash", Some(&input), Some(&path));
        assert_eq!(r.reason.as_deref(), Some("Removing build frees space."));

        // Straight to the tool: the command's own description is used.
        let straight = [lines[0], lines[1], lines[2], lines[4]];
        std::fs::write(&path, straight.join("\n") + "\n").unwrap();
        let r = describe_request("Bash", Some(&input), Some(&path));
        assert_eq!(r.reason.as_deref(), Some("Remove build"));
        std::fs::remove_file(&path).unwrap();
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
    fn unfollowed_events_send_nothing() {
        assert_eq!(kind(json!({"hook_event_name": "PreCompact"})), None);
        assert_eq!(kind(json!({"hook_event_name": "SomethingNew"})), None);
    }
}
