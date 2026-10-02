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

/// Answers with [`Health`]. Needs no token, so the helper can tell "app not
/// running" apart from "wrong token".
pub const HEALTH_PATH: &str = "/health";

/// Largest request body the app accepts. Events are a few hundred bytes.
pub const MAX_BODY_BYTES: usize = 64 * 1024;

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
    Codex,
    Terminal,
}

/// Something that happened in one agent session or terminal job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub source: Source,
    /// The tool's own id for the session, or a fresh id per terminal job.
    pub session_id: String,
    /// The agent's or job's process, so the app can tell when it disappears
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
    },
    /// The agent finished its turn and is waiting for your next prompt.
    TurnFinished,
    /// The agent reported an error that stopped its turn.
    Failed {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    /// A terminal job ended.
    JobFinished { exit_code: i32, duration_ms: u64 },
    /// The session closed normally.
    SessionEnded,
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
                "detail": "npm test"
            })
        );
        assert_eq!(serde_json::from_value::<Event>(value).unwrap(), event);
    }

    #[test]
    fn optional_fields_can_be_left_out() {
        let event: Event = serde_json::from_value(json!({
            "source": "codex",
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
    fn job_finished_carries_exit_code_and_duration() {
        let event: Event = serde_json::from_value(json!({
            "source": "terminal",
            "session_id": "job-7",
            "event": "job-finished",
            "exit_code": 101,
            "duration_ms": 42000
        }))
        .unwrap();
        assert_eq!(
            event.kind,
            EventKind::JobFinished {
                exit_code: 101,
                duration_ms: 42_000
            }
        );
    }

    #[test]
    fn unknown_events_and_sources_are_rejected() {
        let bad_event = json!({"source": "codex", "session_id": "s", "event": "exploded"});
        assert!(serde_json::from_value::<Event>(bad_event).is_err());
        let bad_source = json!({"source": "vim", "session_id": "s", "event": "working"});
        assert!(serde_json::from_value::<Event>(bad_source).is_err());
    }
}
