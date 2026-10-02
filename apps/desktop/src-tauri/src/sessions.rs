//! The live session store, and how it reaches the UI.
//!
//! Events from the local API go into one [`ofa_core::Store`]. A background
//! thread moves it on as time passes, watches busy sessions' transcripts for
//! a refused prompt or Esc, and checks every 10 seconds that each agent's
//! process is still alive. Whenever anything changes, the full list is sent
//! to the UI.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use ofa_core::{SessionKey, SessionState, Store};
use ofa_protocol::{Event, Source};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use windows::Win32::Foundation::{CloseHandle, E_ACCESSDENIED, STILL_ACTIVE};
use windows::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

use crate::focus;
use crate::island::ISLAND;
use crate::transcript;

/// Event sent to the UI with the full session list whenever it changes.
pub const SESSIONS_EVENT: &str = "sessions";

const EXPIRE_EVERY: Duration = Duration::from_secs(1);
const REAP_EVERY: Duration = Duration::from_secs(10);

#[derive(Default)]
pub struct Sessions {
    store: Mutex<Store>,
    /// How far into each session's transcript has been read.
    read_to: Mutex<HashMap<SessionKey, u64>>,
}

/// One session as the UI sees it.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SessionView {
    pub id: String,
    pub source: &'static str,
    pub title: String,
    pub state: &'static str,
    pub detail: String,
}

impl Sessions {
    /// Applies an event from the local API and tells the UI if it changed anything.
    pub fn apply(&self, app: &AppHandle, event: Event) {
        let key = SessionKey {
            source: event.source,
            id: event.session_id.clone(),
        };
        let mut store = self.store.lock().unwrap();
        let changed = store.apply(event, Instant::now());
        let transcript = store
            .sessions()
            .iter()
            .find(|s| s.key == key)
            .and_then(|s| s.transcript.clone());
        drop(store);

        // Whatever the transcript says from here on is newer than this event.
        let mut read_to = self.read_to.lock().unwrap();
        match transcript.and_then(|path| transcript::end_of(Path::new(&path))) {
            Some(end) => read_to.insert(key, end),
            None => read_to.remove(&key),
        };
        drop(read_to);

        if changed {
            self.publish(app);
        }
    }

    /// Reads what busy or waiting sessions appended to their transcripts and
    /// sends any that were stopped by you back to Idle. Returns whether
    /// anything changed.
    fn check_transcripts(&self, now: Instant) -> bool {
        let watched: Vec<(SessionKey, String)> = {
            let store = self.store.lock().unwrap();
            let mut read_to = self.read_to.lock().unwrap();
            read_to.retain(|key, _| store.sessions().iter().any(|s| &s.key == key));
            store
                .sessions()
                .iter()
                .filter(|s| matches!(s.state, SessionState::Working | SessionState::NeedsYou))
                .filter_map(|s| Some((s.key.clone(), s.transcript.clone()?)))
                .collect()
        };

        let mut stopped = Vec::new();
        for (key, path) in watched {
            let path = Path::new(&path);
            let Some(offset) = self.read_to.lock().unwrap().get(&key).copied() else {
                continue;
            };
            if transcript::end_of(path) == Some(offset) {
                continue;
            }
            match transcript::read_new(path, offset) {
                Ok((lines, end)) => {
                    self.read_to.lock().unwrap().insert(key.clone(), end);
                    if transcript::was_interrupted(&lines) {
                        stopped.push(key);
                    }
                }
                Err(err) => eprintln!("sessions: could not read {}: {err}", path.display()),
            }
        }

        let mut store = self.store.lock().unwrap();
        let mut changed = false;
        for key in &stopped {
            changed |= store.interrupt(key, now);
        }
        changed
    }

    pub fn views(&self) -> Vec<SessionView> {
        self.store
            .lock()
            .unwrap()
            .sessions()
            .iter()
            .map(|s| SessionView {
                id: format!("{}:{}", source_name(s.key.source), s.key.id),
                source: source_label(s.key.source),
                title: s.title.clone().unwrap_or_else(|| s.key.id.clone()),
                state: state_name(s.state),
                detail: s.detail.clone().unwrap_or_default(),
            })
            .collect()
    }

    /// The process of the session the UI calls `view_id`.
    fn pid_of(&self, view_id: &str) -> Option<u32> {
        let store = self.store.lock().unwrap();
        store
            .sessions()
            .iter()
            .find(|s| format!("{}:{}", source_name(s.key.source), s.key.id) == view_id)
            .and_then(|s| s.pid)
    }

    fn publish(&self, app: &AppHandle) {
        if let Err(err) = app.emit_to(ISLAND, SESSIONS_EVENT, self.views()) {
            eprintln!("sessions: could not send to the UI: {err}");
        }
    }
}

/// Lets the UI ask for the current list when it starts, since events sent
/// before it was listening are lost.
#[tauri::command]
pub fn get_sessions(sessions: tauri::State<'_, Sessions>) -> Vec<SessionView> {
    sessions.views()
}

/// Brings the clicked session's terminal to the front. Returns whether a
/// window was found.
#[tauri::command]
pub fn focus_session(sessions: tauri::State<'_, Sessions>, id: String) -> bool {
    sessions.pid_of(&id).is_some_and(focus::bring_to_front)
}

/// Starts the thread that runs the timers, the transcript watch and the
/// process check.
pub fn start(app: &AppHandle) -> tauri::Result<()> {
    let app = app.clone();
    thread::Builder::new()
        .name("sessions".into())
        .spawn(move || {
            let sessions = app.state::<Sessions>();
            let mut last_reap = Instant::now();
            loop {
                thread::sleep(EXPIRE_EVERY);
                let now = Instant::now();
                let mut changed = sessions.check_transcripts(now);
                let mut store = sessions.store.lock().unwrap();
                changed |= store.expire(now);
                if now.duration_since(last_reap) >= REAP_EVERY {
                    last_reap = now;
                    changed |= store.reap(now, is_alive);
                }
                drop(store);
                if changed {
                    sessions.publish(&app);
                }
            }
        })?;
    Ok(())
}

/// Whether a process is still running. A process we aren't allowed to query
/// counts as alive, so a permissions problem never marks a session lost.
fn is_alive(pid: u32) -> bool {
    // SAFETY: the handle is closed before returning, and GetExitCodeProcess
    // only writes to the u32 we pass in.
    unsafe {
        let handle = match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            Ok(handle) => handle,
            // Usually the process no longer exists. Access denied means it
            // does, but belongs to someone we can't inspect.
            Err(err) => return err.code() == E_ACCESSDENIED,
        };
        let mut code = 0u32;
        let alive = GetExitCodeProcess(handle, &mut code).is_ok() && code == STILL_ACTIVE.0 as u32;
        let _ = CloseHandle(handle);
        alive
    }
}

fn source_name(source: Source) -> &'static str {
    match source {
        Source::ClaudeCode => "claude-code",
        Source::Codex => "codex",
        Source::Terminal => "terminal",
    }
}

fn source_label(source: Source) -> &'static str {
    match source {
        Source::ClaudeCode => "Claude Code",
        Source::Codex => "Codex",
        Source::Terminal => "Terminal",
    }
}

/// Matches the `SessionState` type in the UI's sessions.ts.
fn state_name(state: SessionState) -> &'static str {
    match state {
        SessionState::Idle => "idle",
        SessionState::Working => "working",
        SessionState::NeedsYou => "needs-you",
        SessionState::Done => "done",
        SessionState::Failed => "failed",
        SessionState::Lost => "lost",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_is_alive() {
        assert!(is_alive(std::process::id()));
    }

    #[test]
    fn a_finished_process_is_not() {
        let mut child = std::process::Command::new("cmd")
            .args(["/C", "exit 0"])
            .spawn()
            .unwrap();
        let pid = child.id();
        child.wait().unwrap();
        // The child's handle is still open until `child` drops, so the
        // process object exists but has exited.
        assert!(!is_alive(pid));
    }

    #[test]
    fn a_pid_that_was_never_used_is_not() {
        // Windows pids are multiples of 4, so this one can't exist.
        assert!(!is_alive(4_000_001));
    }
}
