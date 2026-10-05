//! The live session store, and how it reaches the UI.
//!
//! Events from the local API go into one [`ofa_core::Store`]. A background
//! thread moves it on as time passes, watches busy sessions' transcripts for
//! a refused prompt or Esc, and checks every 10 seconds that each agent's
//! process is still alive. Whenever anything changes, the full list is sent
//! to the UI.
//!
//! A permission prompt can also wait here for your answer: `ofa hook` holds
//! the request open, and a click on the island (or a shortcut) answers it.
//! If the prompt is answered in the terminal instead, the waiting hook is
//! told to step aside.

use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::{self, Sender};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use ofa_core::{SessionKey, SessionState, Store};
use ofa_protocol::{Decision, Event, EventKind, Request, Source};
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

/// Longest a hook waits for an answer from the island. The terminal prompt
/// stays usable all along, and Claude Code gives up on the hook after an hour.
const MAX_WAIT: Duration = Duration::from_secs(55 * 60);

/// A hook waiting for an answer: the prompt it is for, and where to send it.
/// `None` tells it to step aside.
type Waiter = (u64, Sender<Option<Decision>>);

#[derive(Default)]
pub struct Sessions {
    store: Mutex<Store>,
    /// How far into each session's transcript has been read.
    read_to: Mutex<HashMap<SessionKey, u64>>,
    /// Hooks waiting for an answer, by session.
    waiting: Mutex<HashMap<SessionKey, Waiter>>,
}

/// One session as the UI sees it.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SessionView {
    pub id: String,
    pub source: &'static str,
    pub title: String,
    pub state: &'static str,
    pub detail: String,
    /// Which prompt is showing, sent back with an answer.
    pub prompt: u64,
    /// Whether an answer from the island can reach this session's prompt.
    pub answerable: bool,
    /// The whole permission request, while one is waiting.
    pub request: Option<Request>,
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

    /// Records a permission prompt and waits until you answer it on the
    /// island. Returns `None` if it was answered elsewhere first, or if the
    /// wait ran out.
    pub fn wait_for_answer(&self, app: &AppHandle, event: Event) -> Option<Decision> {
        if !matches!(event.kind, EventKind::NeedsYou { .. }) {
            return None;
        }
        let key = SessionKey {
            source: event.source,
            id: event.session_id.clone(),
        };
        self.apply(app, event);

        let (sender, answer) = mpsc::channel();
        let prompt = {
            let store = self.store.lock().unwrap();
            let prompt = store.sessions().iter().find(|s| s.key == key)?.prompt;
            if !store.is_waiting_on(&key, prompt) {
                return None;
            }
            let mut waiting = self.waiting.lock().unwrap();
            if let Some((_, older)) = waiting.insert(key.clone(), (prompt, sender)) {
                let _ = older.send(None);
            }
            prompt
        };
        // Now that a hook is waiting, the UI can offer Allow and Deny.
        self.publish(app);

        let decision = answer.recv_timeout(MAX_WAIT).ok().flatten();
        // A newer prompt for the same session has a higher number; leave it.
        let mut waiting = self.waiting.lock().unwrap();
        if waiting.get(&key).is_some_and(|(p, _)| *p == prompt) {
            waiting.remove(&key);
        }
        decision
    }

    /// Answers prompt number `prompt` of the session the UI calls `view_id`.
    /// Fails if that prompt was already answered, or no hook is waiting on it.
    pub fn answer(
        &self,
        app: &AppHandle,
        view_id: &str,
        prompt: u64,
        allow: bool,
    ) -> Result<(), String> {
        let key = self.key_of(view_id).ok_or("that session has ended")?;
        {
            let mut store = self.store.lock().unwrap();
            let mut waiting = self.waiting.lock().unwrap();
            let Some((waiting_on, _)) = waiting.get(&key) else {
                return Err("that prompt can't be answered from here".into());
            };
            if *waiting_on != prompt || !store.answered(&key, prompt, allow, Instant::now()) {
                return Err("that prompt was already answered".into());
            }
            let (_, sender) = waiting.remove(&key).expect("checked above");
            let decision = if allow {
                Decision::Allow
            } else {
                Decision::Deny
            };
            let _ = sender.send(Some(decision));
        }
        self.publish(app);
        Ok(())
    }

    /// Answers the prompt that has waited longest, for the keyboard shortcuts.
    pub fn answer_longest_waiting(&self, app: &AppHandle, allow: bool) -> Result<(), String> {
        let target = {
            let store = self.store.lock().unwrap();
            let waiting = self.waiting.lock().unwrap();
            store
                .sessions()
                .iter()
                .filter(|s| waiting.get(&s.key).is_some_and(|(p, _)| *p == s.prompt))
                .filter(|s| store.is_waiting_on(&s.key, s.prompt))
                .min_by_key(|s| s.since)
                .map(|s| (view_id(&s.key), s.prompt))
        };
        let (id, prompt) = target.ok_or("nothing is waiting for an answer")?;
        self.answer(app, &id, prompt, allow)
    }

    /// Tells hooks whose prompt is no longer waiting to step aside.
    fn release_answered(&self) {
        let store = self.store.lock().unwrap();
        self.waiting
            .lock()
            .unwrap()
            .retain(|key, (prompt, sender)| {
                let still_waiting = store.is_waiting_on(key, *prompt);
                if !still_waiting {
                    let _ = sender.send(None);
                }
                still_waiting
            });
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
        let store = self.store.lock().unwrap();
        let waiting = self.waiting.lock().unwrap();
        store
            .sessions()
            .iter()
            .map(|s| SessionView {
                id: view_id(&s.key),
                source: source_label(s.key.source),
                title: s.title.clone().unwrap_or_else(|| s.key.id.clone()),
                state: state_name(s.state),
                detail: s.detail.clone().unwrap_or_default(),
                prompt: s.prompt,
                answerable: s.state == SessionState::NeedsYou
                    && waiting.get(&s.key).is_some_and(|(p, _)| *p == s.prompt),
                request: s.request.clone(),
            })
            .collect()
    }

    /// The session the UI calls `id`.
    fn key_of(&self, id: &str) -> Option<SessionKey> {
        let store = self.store.lock().unwrap();
        store
            .sessions()
            .iter()
            .find(|s| view_id(&s.key) == id)
            .map(|s| s.key.clone())
    }

    /// The process of the session the UI calls `id`.
    fn pid_of(&self, id: &str) -> Option<u32> {
        let store = self.store.lock().unwrap();
        store
            .sessions()
            .iter()
            .find(|s| view_id(&s.key) == id)
            .and_then(|s| s.pid)
    }

    fn publish(&self, app: &AppHandle) {
        self.release_answered();
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

/// Answers a permission prompt from the island.
#[tauri::command]
pub fn answer_prompt(
    app: AppHandle,
    sessions: tauri::State<'_, Sessions>,
    id: String,
    prompt: u64,
    allow: bool,
) -> Result<(), String> {
    sessions.answer(&app, &id, prompt, allow)
}

/// Brings the clicked session's terminal to the front. Returns whether a
/// window was found.
#[tauri::command]
pub fn focus_session(sessions: tauri::State<'_, Sessions>, id: String) -> bool {
    sessions.pid_of(&id).is_some_and(focus::bring_to_front)
}

/// Clears a Failed or Lost session from the island once you have seen it.
/// Other sessions are left alone. Returns whether one was cleared.
#[tauri::command]
pub fn dismiss_session(app: AppHandle, sessions: tauri::State<'_, Sessions>, id: String) -> bool {
    let dismissed = {
        let mut store = sessions.store.lock().unwrap();
        let key = store
            .sessions()
            .iter()
            .find(|s| view_id(&s.key) == id)
            .filter(|s| matches!(s.state, SessionState::Failed | SessionState::Lost))
            .map(|s| s.key.clone());
        key.is_some_and(|key| store.dismiss(&key))
    };
    if dismissed {
        sessions.publish(&app);
    }
    dismissed
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

/// How the UI names a session: "claude-code:<id>".
fn view_id(key: &SessionKey) -> String {
    format!("{}:{}", source_name(key.source), key.id)
}

fn source_name(source: Source) -> &'static str {
    match source {
        Source::ClaudeCode => "claude-code",
    }
}

fn source_label(source: Source) -> &'static str {
    match source {
        Source::ClaudeCode => "Claude Code",
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
