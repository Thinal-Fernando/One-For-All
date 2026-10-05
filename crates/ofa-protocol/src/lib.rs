//! Types shared by the OFA app and the `ofa` helper, so the two can't drift apart.
//!
//! The helper talks to the app over HTTP on loopback. Every request except
//! `GET /health` carries the secret token from [`token_path`] as
//! `Authorization: Bearer <token>`. Events are JSON, one per `POST /events`.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Default port for the local API. Picked from the dynamic range to stay clear
/// of common dev servers.
pub const API_PORT: u16 = 47_821;

/// Version of the wire protocol between `ofa.exe` and the app.
pub const PROTOCOL_VERSION: u32 = 1;

/// Takes one [`Event`] as JSON. Needs the token.
pub const EVENTS_PATH: &str = "/events";

/// Takes one [`EventKind::NeedsYou`] event and holds the request open until
/// you answer that prompt on the island, then replies with a
/// [`PermissionAnswer`]. If the prompt is answered elsewhere first, the reply
/// carries no decision. Needs the token.
pub const PERMISSION_PATH: &str = "/permission";

/// Answers with [`Health`]. Needs no token, so the helper can tell "app not
/// running" apart from "wrong token".
pub const HEALTH_PATH: &str = "/health";

/// Largest request body the app accepts. Most events are a few hundred
/// bytes; a permission prompt can carry up to [`MAX_REQUEST_BYTES`] of the
/// request itself, which JSON escaping can grow.
pub const MAX_BODY_BYTES: usize = 256 * 1024;

/// Most of a permission request's [`Request::body`] the helper sends. Longer
/// ones are cut, and the terminal still shows the whole thing.
pub const MAX_REQUEST_BYTES: usize = 48 * 1024;

/// Address of the local API. It only ever binds to loopback, so nothing
/// outside this machine can reach it.
pub fn api_addr() -> SocketAddr {
    SocketAddr::from((Ipv4Addr::LOCALHOST, API_PORT))
}

/// Where the app keeps its secret token: `%APPDATA%\OFA\token`. Only your
/// Windows user can read that folder. `None` if `APPDATA` isn't set.
pub fn token_path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(appdata).join("OFA").join("token"))
}

/// Which tool a session belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    ClaudeCode,
}

/// Something that happened in one agent session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub source: Source,
    /// The tool's own id for the session.
    pub session_id: String,
    /// The agent's process, so the app can tell when it disappears
    /// without saying goodbye (Ctrl+C, a closed terminal).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// Short name for the island, usually the project folder or command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The agent's session log, if it keeps one. Claude Code sends no event
    /// when you refuse a prompt or press Esc, but it writes a line here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<String>,
    #[serde(flatten)]
    pub kind: EventKind,
}

/// What happened. Serialised as an `"event"` field next to the [`Event`]'s
/// own fields, for example `{"event": "needs-you", "detail": "npm test", ...}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "kebab-case")]
pub enum EventKind {
    /// The session exists but isn't doing anything yet.
    SessionStarted,
    /// The agent is busy: a prompt was sent, a tool started or finished.
    /// Also means any waiting prompt was answered.
    Working {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    /// The agent is waiting for you to allow or deny something.
    NeedsYou {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
        /// The whole request, for the island to show in full.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request: Option<Request>,
    },
    /// The agent finished its turn and is waiting for your next prompt.
    TurnFinished,
    /// The agent reported an error that stopped its turn.
    Failed {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    /// The session closed normally.
    SessionEnded,
}

/// Everything a permission prompt asks for, so you can read it on the island
/// before answering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    /// The tool that wants to run, such as "Bash" or "Edit".
    pub tool: String,
    /// The file it would change, for tools that change files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    pub format: RequestFormat,
    /// The command, the change as diff lines, or the tool's input.
    pub body: String,
    /// Whether `body` was cut to [`MAX_REQUEST_BYTES`].
    #[serde(default)]
    pub truncated: bool,
    /// Why Claude asked, in its own words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// How to show a [`Request::body`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RequestFormat {
    /// A shell command.
    Command,
    /// Lines starting with "+" are added and "-" removed.
    Diff,
    /// Anything else, such as a tool's input as JSON.
    Text,
}

/// Your answer to a permission prompt, given on the island.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Decision {
    Allow,
    Deny,
}

/// Reply to `POST /permission`. `decision` is `None` when the prompt was
/// answered somewhere else, so the hook should step aside.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionAnswer {
    pub decision: Option<Decision>,
}

/// Body of `GET /health`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    pub app_version: String,
    pub protocol: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn api_is_loopback_only() {
        let addr = api_addr();
        assert!(addr.ip().is_loopback());
        assert_eq!(addr.to_string(), "127.0.0.1:47821");
    }

    #[test]
    fn token_lives_in_appdata() {
        if let Some(path) = token_path() {
            assert!(path.ends_with("OFA/token") || path.ends_with("OFA\\token"));
        }
    }

    // The JSON shape is the contract with ofa.exe, so pin it exactly.
    #[test]
    fn event_wire_format() {
        let event = Event {
            source: Source::ClaudeCode,
            session_id: "abc".into(),
            pid: Some(4242),
            title: Some("one-for-all".into()),
            transcript_path: None,
            kind: EventKind::NeedsYou {
                detail: Some("npm test".into()),
                request: Some(Request {
                    tool: "Bash".into(),
                    file: None,
                    format: RequestFormat::Command,
                    body: "npm test".into(),
                    truncated: false,
                    reason: Some("Run the tests".into()),
                }),
            },
        };
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(
            value,
            json!({
                "source": "claude-code",
                "session_id": "abc",
                "pid": 4242,
                "title": "one-for-all",
                "event": "needs-you",
                "detail": "npm test",
                "request": {
                    "tool": "Bash",
                    "format": "command",
                    "body": "npm test",
                    "truncated": false,
                    "reason": "Run the tests"
                }
            })
        );
        assert_eq!(serde_json::from_value::<Event>(value).unwrap(), event);
    }

    #[test]
    fn optional_fields_can_be_left_out() {
        let event: Event = serde_json::from_value(json!({
            "source": "claude-code",
            "session_id": "s1",
            "event": "working"
        }))
        .unwrap();
        assert_eq!(event.pid, None);
        assert_eq!(event.title, None);
        assert_eq!(event.transcript_path, None);
        assert_eq!(event.kind, EventKind::Working { detail: None });
    }

    #[test]
    fn permission_answer_wire_format() {
        let allow = PermissionAnswer {
            decision: Some(Decision::Allow),
        };
        assert_eq!(
            serde_json::to_value(allow).unwrap(),
            json!({"decision": "allow"})
        );
        let none: PermissionAnswer = serde_json::from_value(json!({"decision": null})).unwrap();
        assert_eq!(none.decision, None);
    }

    #[test]
    fn unknown_events_and_sources_are_rejected() {
        let bad_event = json!({"source": "claude-code", "session_id": "s", "event": "exploded"});
        assert!(serde_json::from_value::<Event>(bad_event).is_err());
        let bad_source = json!({"source": "vim", "session_id": "s", "event": "working"});
        assert!(serde_json::from_value::<Event>(bad_source).is_err());
    }
}
