//! Session state machine for OFA. Pure logic with no I/O, so it can be
//! unit-tested against recorded events: the caller passes in the time and
//! says which processes are still alive.

use std::time::{Duration, Instant};

use ofa_protocol::{Event, EventKind, Source};

/// How long a finished session shows as Done before going back to Idle.
pub const DONE_FOR: Duration = Duration::from_secs(6);

/// How long a Lost session stays listed before it is dropped.
pub const LOST_FOR: Duration = Duration::from_secs(60);

/// What one session is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Idle,
    Working,
    NeedsYou,
    Done,
    Failed,
    /// Its process disappeared without a stop or end event, for example after
    /// Ctrl+C or a closed terminal. Without this it would spin forever.
    Lost,
}

/// What the island shows, most urgent first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum IslandState {
    NeedsYou,
    Failed,
    Working,
    Done,
    Idle,
}

impl IslandState {
    /// The state the island shows for a set of sessions: the most urgent one.
    pub fn most_urgent(states: impl IntoIterator<Item = IslandState>) -> IslandState {
        states.into_iter().min().unwrap_or(IslandState::Idle)
    }
}

impl From<SessionState> for IslandState {
    fn from(state: SessionState) -> Self {
        match state {
            SessionState::NeedsYou => IslandState::NeedsYou,
            SessionState::Failed => IslandState::Failed,
            SessionState::Working => IslandState::Working,
            SessionState::Done => IslandState::Done,
            // A lost session is listed in the panel but doesn't light up the island.
            SessionState::Idle | SessionState::Lost => IslandState::Idle,
        }
    }
}

/// Identifies a session across events. Ids are only unique per tool.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionKey {
    pub source: Source,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub key: SessionKey,
    pub title: Option<String>,
    pub pid: Option<u32>,
    /// The agent's session log, used to notice a refused prompt or Esc.
    pub transcript: Option<String>,
    pub state: SessionState,
    /// One short line about what is happening, such as the waiting command.
    pub detail: Option<String>,
    /// When `state` last changed.
    pub since: Instant,
}

/// Every known session, in the order they first appeared.
#[derive(Debug, Default)]
pub struct Store {
    sessions: Vec<Session>,
}

impl Store {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn sessions(&self) -> &[Session] {
        &self.sessions
    }

    pub fn island_state(&self) -> IslandState {
        IslandState::most_urgent(self.sessions.iter().map(|s| s.state.into()))
    }

    /// Applies one event. Returns whether anything changed.
    pub fn apply(&mut self, event: Event, now: Instant) -> bool {
        let key = SessionKey {
            source: event.source,
            id: event.session_id,
        };
        let index = self.sessions.iter().position(|s| s.key == key);

        if event.kind == EventKind::SessionEnded {
            return match index {
                Some(i) => {
                    self.sessions.remove(i);
                    true
                }
                None => false,
            };
        }

        let session = match index {
            Some(i) => &mut self.sessions[i],
            None => {
                self.sessions.push(Session {
                    key,
                    title: None,
                    pid: None,
                    transcript: None,
                    state: SessionState::Idle,
                    detail: None,
                    since: now,
                });
                self.sessions.last_mut().expect("just pushed")
            }
        };
        let before = session.clone();

        if event.pid.is_some() {
            session.pid = event.pid;
        }
        if event.title.is_some() {
            session.title = event.title;
        }
        if event.transcript_path.is_some() {
            session.transcript = event.transcript_path;
        }

        let next = match event.kind {
            // Only creates the session; a late start must not reset its state.
            EventKind::SessionStarted => None,
            EventKind::Working { detail } => Some((SessionState::Working, detail)),
            // Claude Code's permission notice doesn't say what is waiting, but
            // the tool event just before it did, so keep that line.
            EventKind::NeedsYou { detail } => Some((
                SessionState::NeedsYou,
                detail.or_else(|| session.detail.clone()),
            )),
            EventKind::TurnFinished => Some((SessionState::Done, None)),
            EventKind::Failed { detail } => Some((SessionState::Failed, detail)),
            EventKind::JobFinished {
                exit_code: 0,
                duration_ms,
            } => Some((
                SessionState::Done,
                Some(format!("Finished in {}", duration(duration_ms))),
            )),
            EventKind::JobFinished {
                exit_code,
                duration_ms,
            } => Some((
                SessionState::Failed,
                Some(format!(
                    "Exited with code {exit_code} after {}",
                    duration(duration_ms)
                )),
            )),
            EventKind::SessionEnded => unreachable!("handled above"),
        };

        if let Some((state, detail)) = next {
            if state != session.state {
                session.since = now;
            }
            session.state = state;
            session.detail = detail;
        }

        index.is_none() || *session != before
    }

    /// Moves sessions on as time passes: Done becomes Idle after
    /// [`DONE_FOR`], and Lost sessions are dropped after [`LOST_FOR`].
    /// Returns whether anything changed.
    pub fn expire(&mut self, now: Instant) -> bool {
        let mut changed = false;
        self.sessions.retain_mut(|s| {
            let age = now.saturating_duration_since(s.since);
            match s.state {
                SessionState::Done if age >= DONE_FOR => {
                    s.state = SessionState::Idle;
                    s.detail = None;
                    s.since = now;
                    changed = true;
                    true
                }
                SessionState::Lost if age >= LOST_FOR => {
                    changed = true;
                    false
                }
                _ => true,
            }
        });
        changed
    }

    /// Handles sessions whose process has gone. A busy or waiting session
    /// becomes Lost, an idle one is dropped. Done, Failed and Lost keep their
    /// state, since a finished job's process is expected to be gone.
    /// Returns whether anything changed.
    pub fn reap(&mut self, now: Instant, is_alive: impl Fn(u32) -> bool) -> bool {
        let mut changed = false;
        self.sessions.retain_mut(|s| {
            let Some(pid) = s.pid else { return true };
            match s.state {
                SessionState::Working | SessionState::NeedsYou if !is_alive(pid) => {
                    s.state = SessionState::Lost;
                    s.detail = Some("Process ended without saying goodbye".into());
                    s.since = now;
                    changed = true;
                    true
                }
                SessionState::Idle if !is_alive(pid) => {
                    changed = true;
                    false
                }
                _ => true,
            }
        });
        changed
    }

    /// The agent stopped mid-turn because you refused a prompt or pressed
    /// Esc. It sends no event for that, so the caller spots it another way.
    /// A busy or waiting session goes back to Idle; others are left alone.
    /// Returns whether anything changed.
    pub fn interrupt(&mut self, key: &SessionKey, now: Instant) -> bool {
        let Some(s) = self.sessions.iter_mut().find(|s| &s.key == key) else {
            return false;
        };
        if !matches!(s.state, SessionState::Working | SessionState::NeedsYou) {
            return false;
        }
        s.state = SessionState::Idle;
        s.detail = None;
        s.since = now;
        true
    }

    /// Clears a Failed or Lost session you have seen. Returns whether it existed.
    pub fn dismiss(&mut self, key: &SessionKey) -> bool {
        let before = self.sessions.len();
        self.sessions.retain(|s| &s.key != key);
        self.sessions.len() != before
    }
}

/// "850 ms", "42 s", "3 min 5 s", "2 h 10 min".
fn duration(ms: u64) -> String {
    let secs = ms / 1000;
    match secs {
        0 => format!("{ms} ms"),
        1..=59 => format!("{secs} s"),
        60..=3599 if secs.is_multiple_of(60) => format!("{} min", secs / 60),
        60..=3599 => format!("{} min {} s", secs / 60, secs % 60),
        _ if (secs % 3600) / 60 == 0 => format!("{} h", secs / 3600),
        _ => format!("{} h {} min", secs / 3600, secs % 3600 / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::IslandState::*;
    use super::*;

    fn event(id: &str, kind: EventKind) -> Event {
        Event {
            source: Source::ClaudeCode,
            session_id: id.into(),
            pid: None,
            title: None,
            transcript_path: None,
            kind,
        }
    }

    fn working(detail: &str) -> EventKind {
        EventKind::Working {
            detail: Some(detail.into()),
        }
    }

    fn needs_you(detail: &str) -> EventKind {
        EventKind::NeedsYou {
            detail: Some(detail.into()),
        }
    }

    fn state_of(store: &Store, id: &str) -> Option<SessionState> {
        store
            .sessions()
            .iter()
            .find(|s| s.key.id == id)
            .map(|s| s.state)
    }

    fn key(id: &str) -> SessionKey {
        SessionKey {
            source: Source::ClaudeCode,
            id: id.into(),
        }
    }

    #[test]
    fn empty_is_idle() {
        assert_eq!(IslandState::most_urgent([]), Idle);
        assert_eq!(Store::new().island_state(), Idle);
    }

    #[test]
    fn needs_you_beats_everything() {
        assert_eq!(
            IslandState::most_urgent([Done, Working, NeedsYou, Failed]),
            NeedsYou
        );
        assert_eq!(IslandState::most_urgent([Done, Working, Failed]), Failed);
        assert_eq!(IslandState::most_urgent([Idle, Done, Working]), Working);
    }

    #[test]
    fn a_full_turn_with_a_permission_prompt() {
        let t = Instant::now();
        let mut store = Store::new();

        assert!(store.apply(event("a", EventKind::SessionStarted), t));
        assert_eq!(state_of(&store, "a"), Some(SessionState::Idle));
        assert_eq!(store.island_state(), Idle);

        store.apply(event("a", working("Reading src/lib.rs")), t);
        assert_eq!(store.island_state(), Working);

        store.apply(event("a", needs_you("npm test")), t);
        assert_eq!(store.island_state(), NeedsYou);
        assert_eq!(store.sessions()[0].detail.as_deref(), Some("npm test"));

        // Answered, in the terminal or the pill: the tool runs.
        store.apply(event("a", working("npm test")), t);
        assert_eq!(store.island_state(), Working);

        store.apply(event("a", EventKind::TurnFinished), t);
        assert_eq!(store.island_state(), Done);
        assert_eq!(store.sessions()[0].detail, None);

        store.apply(event("a", EventKind::SessionEnded), t);
        assert!(store.sessions().is_empty());
    }

    #[test]
    fn a_prompt_without_detail_keeps_the_waiting_command() {
        let t = Instant::now();
        let mut store = Store::new();
        store.apply(event("a", working("npm test")), t);
        store.apply(event("a", EventKind::NeedsYou { detail: None }), t);
        assert_eq!(store.sessions()[0].state, SessionState::NeedsYou);
        assert_eq!(store.sessions()[0].detail.as_deref(), Some("npm test"));
    }

    #[test]
    fn the_first_event_can_be_anything() {
        let mut store = Store::new();
        assert!(store.apply(event("a", needs_you("rm -rf build")), Instant::now()));
        assert_eq!(state_of(&store, "a"), Some(SessionState::NeedsYou));
    }

    #[test]
    fn a_late_session_start_does_not_reset_a_busy_session() {
        let t = Instant::now();
        let mut store = Store::new();
        store.apply(event("a", working("x")), t);
        assert!(!store.apply(event("a", EventKind::SessionStarted), t));
        assert_eq!(state_of(&store, "a"), Some(SessionState::Working));
    }

    #[test]
    fn apply_reports_whether_anything_changed() {
        let t = Instant::now();
        let mut store = Store::new();
        assert!(store.apply(event("a", working("x")), t));
        assert!(!store.apply(event("a", working("x")), t));
        assert!(store.apply(event("a", working("y")), t));
        assert!(!store.apply(event("gone", EventKind::SessionEnded), t));
    }

    #[test]
    fn pid_and_title_stick_until_replaced() {
        let t = Instant::now();
        let mut store = Store::new();
        let mut first = event("a", EventKind::SessionStarted);
        first.pid = Some(10);
        first.title = Some("one-for-all".into());
        store.apply(first, t);
        store.apply(event("a", working("x")), t);
        let s = &store.sessions()[0];
        assert_eq!((s.pid, s.title.as_deref()), (Some(10), Some("one-for-all")));
    }

    #[test]
    fn sessions_from_different_tools_never_mix() {
        let t = Instant::now();
        let mut store = Store::new();
        store.apply(event("same-id", working("x")), t);
        let mut codex = event("same-id", needs_you("y"));
        codex.source = Source::Codex;
        store.apply(codex, t);
        assert_eq!(store.sessions().len(), 2);
        assert_eq!(store.sessions()[0].state, SessionState::Working);
        assert_eq!(store.sessions()[1].state, SessionState::NeedsYou);
    }

    #[test]
    fn island_shows_the_most_urgent_session() {
        let t = Instant::now();
        let mut store = Store::new();
        store.apply(event("a", working("x")), t);
        store.apply(event("b", EventKind::TurnFinished), t);
        assert_eq!(store.island_state(), Working);
        store.apply(event("c", needs_you("y")), t);
        assert_eq!(store.island_state(), NeedsYou);
        store.apply(event("c", EventKind::SessionEnded), t);
        assert_eq!(store.island_state(), Working);
    }

    #[test]
    fn done_turns_idle_after_six_seconds() {
        let t = Instant::now();
        let mut store = Store::new();
        store.apply(event("a", EventKind::TurnFinished), t);
        assert!(!store.expire(t + Duration::from_millis(5_999)));
        assert_eq!(store.island_state(), Done);
        assert!(store.expire(t + DONE_FOR));
        assert_eq!(state_of(&store, "a"), Some(SessionState::Idle));
        assert_eq!(store.island_state(), Idle);
    }

    #[test]
    fn done_timer_restarts_with_each_finish() {
        let t = Instant::now();
        let mut store = Store::new();
        store.apply(event("a", EventKind::TurnFinished), t);
        store.apply(event("a", working("x")), t + Duration::from_secs(4));
        store.apply(
            event("a", EventKind::TurnFinished),
            t + Duration::from_secs(5),
        );
        store.expire(t + Duration::from_secs(10));
        assert_eq!(state_of(&store, "a"), Some(SessionState::Done));
    }

    #[test]
    fn refusing_a_prompt_or_pressing_esc_goes_idle() {
        let t = Instant::now();
        let mut store = Store::new();
        store.apply(event("a", needs_you("rm build")), t);
        assert!(store.interrupt(&key("a"), t));
        assert_eq!(state_of(&store, "a"), Some(SessionState::Idle));
        assert_eq!(store.sessions()[0].detail, None);
        assert_eq!(store.island_state(), Idle);

        store.apply(event("a", working("x")), t);
        assert!(store.interrupt(&key("a"), t));
        assert_eq!(state_of(&store, "a"), Some(SessionState::Idle));
    }

    #[test]
    fn an_interrupt_leaves_finished_sessions_alone() {
        let t = Instant::now();
        let mut store = Store::new();
        store.apply(event("a", EventKind::TurnFinished), t);
        assert!(!store.interrupt(&key("a"), t));
        assert_eq!(state_of(&store, "a"), Some(SessionState::Done));
        assert!(!store.interrupt(&key("missing"), t));
    }

    #[test]
    fn the_transcript_path_sticks() {
        let t = Instant::now();
        let mut store = Store::new();
        let mut first = event("a", EventKind::SessionStarted);
        first.transcript_path = Some("C:/t/a.jsonl".into());
        store.apply(first, t);
        store.apply(event("a", working("x")), t);
        assert_eq!(
            store.sessions()[0].transcript.as_deref(),
            Some("C:/t/a.jsonl")
        );
    }

    #[test]
    fn failed_stays_until_dismissed() {
        let t = Instant::now();
        let mut store = Store::new();
        store.apply(
            event(
                "a",
                EventKind::Failed {
                    detail: Some("API error".into()),
                },
            ),
            t,
        );
        assert!(!store.expire(t + Duration::from_secs(3600)));
        assert!(!store.reap(t, |_| false));
        assert_eq!(store.island_state(), Failed);
        assert!(store.dismiss(&key("a")));
        assert!(!store.dismiss(&key("a")));
        assert_eq!(store.island_state(), Idle);
    }

    #[test]
    fn a_vanished_busy_session_is_lost_then_dropped() {
        let t = Instant::now();
        let mut store = Store::new();
        let mut e = event("a", needs_you("x"));
        e.pid = Some(42);
        store.apply(e, t);

        assert!(!store.reap(t, |_| true));
        assert!(store.reap(t, |pid| pid != 42));
        assert_eq!(state_of(&store, "a"), Some(SessionState::Lost));
        // Lost is listed but doesn't light up the island.
        assert_eq!(store.island_state(), Idle);
        // Reaping again changes nothing.
        assert!(!store.reap(t, |_| false));

        assert!(!store.expire(t + LOST_FOR - Duration::from_millis(1)));
        assert!(store.expire(t + LOST_FOR));
        assert!(store.sessions().is_empty());
    }

    #[test]
    fn a_vanished_idle_session_is_dropped_at_once() {
        let t = Instant::now();
        let mut store = Store::new();
        let mut e = event("a", EventKind::SessionStarted);
        e.pid = Some(7);
        store.apply(e, t);
        assert!(store.reap(t, |_| false));
        assert!(store.sessions().is_empty());
    }

    #[test]
    fn sessions_without_a_pid_are_never_reaped() {
        let t = Instant::now();
        let mut store = Store::new();
        store.apply(event("a", working("x")), t);
        assert!(!store.reap(t, |_| false));
        assert_eq!(state_of(&store, "a"), Some(SessionState::Working));
    }

    #[test]
    fn a_lost_session_comes_back_if_it_speaks_again() {
        let t = Instant::now();
        let mut store = Store::new();
        let mut e = event("a", working("x"));
        e.pid = Some(1);
        store.apply(e, t);
        store.reap(t, |_| false);
        store.apply(event("a", working("y")), t);
        assert_eq!(state_of(&store, "a"), Some(SessionState::Working));
    }

    #[test]
    fn terminal_jobs_finish_done_or_failed_by_exit_code() {
        let t = Instant::now();
        let mut store = Store::new();
        let job = |id: &str, exit_code, duration_ms| Event {
            source: Source::Terminal,
            session_id: id.into(),
            pid: Some(99),
            title: Some("cargo test".into()),
            transcript_path: None,
            kind: EventKind::JobFinished {
                exit_code,
                duration_ms,
            },
        };
        store.apply(job("ok", 0, 185_000), t);
        store.apply(job("bad", 101, 42_000), t);
        let detail = |i: usize| store.sessions()[i].detail.as_deref();
        assert_eq!(store.sessions()[0].state, SessionState::Done);
        assert_eq!(detail(0), Some("Finished in 3 min 5 s"));
        assert_eq!(store.sessions()[1].state, SessionState::Failed);
        assert_eq!(detail(1), Some("Exited with code 101 after 42 s"));
        // The job's process is gone by now, and that's expected.
        assert!(!store.reap(t, |_| false));
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(duration(850), "850 ms");
        assert_eq!(duration(42_000), "42 s");
        assert_eq!(duration(120_000), "2 min");
        assert_eq!(duration(185_000), "3 min 5 s");
        assert_eq!(duration(3_600_000), "1 h");
        assert_eq!(duration(7_800_000), "2 h 10 min");
    }
}
