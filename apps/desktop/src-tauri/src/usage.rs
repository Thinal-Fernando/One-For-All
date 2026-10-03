//! Works out plan usage and sends it to the UI.
//!
//! By default it is an estimate from Claude Code's local session logs:
//! Claude Code writes each reply to `~/.claude/projects/**/*.jsonl` with a
//! `usage` block. Only those token numbers and the time are used; message
//! text is never kept. Logs are read incrementally, once a minute.
//!
//! With the opt-in on, the exact percentages from Claude come along too
//! (see `plan`), and the estimate remains as the fallback.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ofa_core::usage::{self, UsageEntry, UsageSummary, WEEK_SECS};
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};

use crate::island::ISLAND;
use crate::plan::{self, PlanUsage};

/// Event sent to the UI whenever the estimate changes.
pub const USAGE_EVENT: &str = "usage";

const SCAN_EVERY: Duration = Duration::from_secs(60);

/// How often to ask Claude for exact usage, when that is switched on.
const PLAN_EVERY_SECS: i64 = 2 * 60;

/// The longest wait between tries while Claude keeps refusing.
const PLAN_MAX_BACKOFF_SECS: i64 = 16 * 60;

/// How long a last good answer from Claude is shown while new ones fail.
const PLAN_STALE_AFTER_SECS: i64 = 10 * 60;

/// The usage estimate as the UI sees it.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Default)]
pub struct UsageView {
    pub window_tokens: u64,
    /// Seconds since the Unix epoch, or `None` when no window is open.
    pub window_resets_at: Option<i64>,
    pub week_tokens: u64,
    /// Exact usage from Claude, when the opt-in is on and Claude answered.
    pub plan: Option<PlanUsage>,
}

impl UsageView {
    fn new(s: UsageSummary, plan: Option<PlanUsage>) -> Self {
        Self {
            window_tokens: s.window_tokens,
            window_resets_at: s.window_resets_at,
            week_tokens: s.week_tokens,
            plan,
        }
    }
}

#[derive(Default)]
pub struct Usage {
    latest: Mutex<UsageView>,
    /// Wakes the usage thread early, after a settings change.
    wake: Mutex<Option<Sender<()>>>,
}

/// Re-reads the usage now instead of at the next minute, so switching exact
/// usage on or off shows at once.
pub fn refresh(app: &AppHandle) {
    if let Some(wake) = &*app.state::<Usage>().wake.lock().unwrap() {
        let _ = wake.send(());
    }
}

/// When to next ask Claude for exact usage. Claude refuses (429) if asked
/// too often, so a failure waits twice as long as the one before, and the
/// last good answer is kept while the opt-in is off, so switching it off and
/// on again shows that answer instead of asking again.
#[derive(Debug, Default)]
struct PlanSchedule {
    next_at: i64,
    backoff: i64,
}

impl PlanSchedule {
    fn due(&self, now: i64) -> bool {
        now >= self.next_at
    }

    fn succeeded(&mut self, now: i64) {
        self.backoff = 0;
        self.next_at = now + PLAN_EVERY_SECS;
    }

    fn failed(&mut self, now: i64) {
        self.backoff = (self.backoff * 2).clamp(PLAN_EVERY_SECS, PLAN_MAX_BACKOFF_SECS);
        self.next_at = now + self.backoff;
    }
}

/// Lets the UI ask for the estimate when it starts.
#[tauri::command]
pub fn get_usage(usage: tauri::State<'_, Usage>) -> UsageView {
    *usage.latest.lock().unwrap()
}

/// Starts the thread that reads the logs once a minute.
pub fn start(app: &AppHandle) -> tauri::Result<()> {
    let Some(root) = projects_dir() else {
        eprintln!("usage: no Claude Code folder found, so no usage estimate");
        return Ok(());
    };
    let (wake, woken) = mpsc::channel();
    *app.state::<Usage>().wake.lock().unwrap() = Some(wake);
    let app = app.clone();
    thread::Builder::new().name("usage".into()).spawn(move || {
        let mut scanner = Scanner::default();
        let mut plan_usage: Option<(PlanUsage, i64)> = None;
        let mut schedule = PlanSchedule::default();
        loop {
            let now = unix_now();
            scanner.scan(&root, now);
            let enabled = plan::enabled();
            if enabled && schedule.due(now) {
                match plan::fetch() {
                    Ok(fresh) => {
                        plan_usage = Some((fresh, now));
                        schedule.succeeded(now);
                    }
                    Err(err) => {
                        eprintln!("usage: no exact usage from Claude: {err}");
                        schedule.failed(now);
                    }
                }
            }
            let plan = plan_usage
                .filter(|(_, at)| enabled && now - at < PLAN_STALE_AFTER_SECS)
                .map(|(p, _)| p);
            let view = UsageView::new(usage::summarize(&scanner.entries(), now), plan);
            let state = app.state::<Usage>();
            let changed = {
                let mut latest = state.latest.lock().unwrap();
                let changed = *latest != view;
                *latest = view;
                changed
            };
            if changed {
                if let Err(err) = app.emit_to(ISLAND, USAGE_EVENT, view) {
                    eprintln!("usage: could not send to the UI: {err}");
                }
            }
            // A settings change wakes this early, so the switch shows at once.
            let _ = woken.recv_timeout(SCAN_EVERY);
        }
    })?;
    Ok(())
}

/// `~/.claude/projects`, or under `CLAUDE_CONFIG_DIR` if set.
fn projects_dir() -> Option<PathBuf> {
    let base = match std::env::var_os("CLAUDE_CONFIG_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(std::env::var_os("USERPROFILE")?).join(".claude"),
    };
    Some(base.join("projects"))
}

/// Remembers how far each log has been read and every reply counted, so a
/// reply logged more than once (Claude Code does that while streaming) is
/// only counted once.
#[derive(Default)]
struct Scanner {
    read_to: HashMap<PathBuf, u64>,
    seen: HashSet<String>,
    entries: Vec<(String, UsageEntry)>,
}

impl Scanner {
    fn entries(&self) -> Vec<UsageEntry> {
        self.entries.iter().map(|(_, e)| *e).collect()
    }

    fn scan(&mut self, root: &Path, now: i64) {
        let cutoff = now - WEEK_SECS;
        for path in logs_changed_since(root, cutoff) {
            let offset = self.read_to.get(&path).copied().unwrap_or(0);
            match read_from(&path, offset) {
                Ok((text, end)) => {
                    self.read_to.insert(path, end);
                    for line in text.lines().filter(|l| l.contains("\"usage\"")) {
                        if let Some((key, entry)) = parse_line(line) {
                            if entry.at > cutoff && self.seen.insert(key.clone()) {
                                self.entries.push((key, entry));
                            }
                        }
                    }
                }
                Err(err) => eprintln!("usage: could not read {}: {err}", path.display()),
            }
        }
        // Forget what is too old to matter.
        let seen = &mut self.seen;
        self.entries.retain(|(key, e)| {
            let keep = e.at > cutoff;
            if !keep {
                seen.remove(key);
            }
            keep
        });
    }
}

/// Every `.jsonl` under `root` written to since `cutoff`.
fn logs_changed_since(root: &Path, cutoff: i64) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut folders = vec![root.to_path_buf()];
    while let Some(folder) = folders.pop() {
        let Ok(items) = fs::read_dir(&folder) else {
            continue;
        };
        for item in items.flatten() {
            let path = item.path();
            let Ok(meta) = item.metadata() else { continue };
            if meta.is_dir() {
                folders.push(path);
            } else if path.extension().is_some_and(|e| e == "jsonl") {
                let modified = meta
                    .modified()
                    .ok()
                    .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_secs() as i64);
                if modified > cutoff {
                    found.push(path);
                }
            }
        }
    }
    found
}

/// Complete lines from `offset` to the end of the file, and where they end.
fn read_from(path: &Path, offset: u64) -> std::io::Result<(String, u64)> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let start = if offset > len { 0 } else { offset };
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(len - start).read_to_end(&mut bytes)?;
    let complete = bytes.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
    bytes.truncate(complete);
    Ok((
        String::from_utf8_lossy(&bytes).into_owned(),
        start + complete as u64,
    ))
}

/// A reply's identity and its usage, from one log line. Tokens are the input,
/// output and cache-write tokens; cache reads are left out, as they are a
/// tiny fraction of the cost and would dwarf everything else.
fn parse_line(line: &str) -> Option<(String, UsageEntry)> {
    let entry: Value = serde_json::from_str(line).ok()?;
    if entry["type"] != "assistant" {
        return None;
    }
    let message = &entry["message"];
    let usage = &message["usage"];
    let count = |name: &str| usage[name].as_u64().unwrap_or(0);
    let tokens =
        count("input_tokens") + count("output_tokens") + count("cache_creation_input_tokens");
    let key = format!(
        "{}:{}",
        message["id"].as_str()?,
        entry["requestId"].as_str().unwrap_or("")
    );
    let at = parse_timestamp(entry["timestamp"].as_str()?)?;
    Some((key, UsageEntry { at, tokens }))
}

/// Seconds since the Unix epoch from "2026-10-02T04:22:40.292Z" or
/// "2026-10-03T15:20:00.361993+00:00".
pub fn parse_timestamp(text: &str) -> Option<i64> {
    let number = |range: std::ops::Range<usize>| text.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    if text.get(4..5) != Some("-") || text.get(10..11) != Some("T") {
        return None;
    }
    // Skip any fraction of a second, then read the zone: "Z" or "+05:30".
    let zone = text
        .get(19..)?
        .trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let offset = match zone {
        "Z" => 0,
        _ => {
            let sign = match zone.get(0..1)? {
                "+" => 1,
                "-" => -1,
                _ => return None,
            };
            let hours: i64 = zone.get(1..3)?.parse().ok()?;
            let minutes: i64 = zone.get(4..6)?.parse().ok()?;
            sign * (hours * 3600 + minutes * 60)
        }
    };
    let local = days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second;
    Some(local - offset)
}

/// Days since 1970-01-01 for a date in the proleptic Gregorian calendar.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_usage_backs_off_while_refused_and_resets_on_success() {
        let mut s = PlanSchedule::default();
        assert!(s.due(0));
        s.failed(0);
        assert!(!s.due(PLAN_EVERY_SECS - 1));
        assert!(s.due(PLAN_EVERY_SECS));
        s.failed(120);
        assert_eq!(s.next_at, 120 + 2 * PLAN_EVERY_SECS);
        for _ in 0..10 {
            s.failed(1000);
        }
        assert_eq!(s.next_at, 1000 + PLAN_MAX_BACKOFF_SECS);
        s.succeeded(5000);
        assert_eq!(s.next_at, 5000 + PLAN_EVERY_SECS);
        s.failed(5200);
        assert_eq!(s.next_at, 5200 + PLAN_EVERY_SECS);
    }

    // Trimmed from a real Claude Code 2.1 log line.
    const LINE: &str = r#"{"type":"assistant","timestamp":"2026-10-02T04:22:40.292Z","requestId":"req_011","message":{"id":"msg_011","model":"claude-opus-5-5","usage":{"input_tokens":2,"cache_creation_input_tokens":31192,"cache_read_input_tokens":52127,"output_tokens":491}}}"#;

    #[test]
    fn reads_tokens_and_time_from_a_reply() {
        let (key, entry) = parse_line(LINE).unwrap();
        assert_eq!(key, "msg_011:req_011");
        assert_eq!(entry.tokens, 2 + 491 + 31_192);
        assert_eq!(entry.at, 1_790_914_960);
    }

    #[test]
    fn other_lines_are_skipped() {
        assert!(parse_line(r#"{"type":"user","message":{"content":"hi"}}"#).is_none());
        assert!(parse_line("not json").is_none());
    }

    #[test]
    fn timestamps_convert_to_unix_seconds() {
        assert_eq!(parse_timestamp("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_timestamp("2000-03-01T00:00:00.000Z"),
            Some(951_868_800)
        );
        assert_eq!(
            parse_timestamp("2026-10-03T09:00:00.5Z"),
            Some(1_791_018_000)
        );
        assert_eq!(
            parse_timestamp("2026-10-03T15:20:00.361993+00:00"),
            Some(1_791_040_800)
        );
        assert_eq!(
            parse_timestamp("2026-10-03T20:50:00+05:30"),
            Some(1_791_040_800)
        );
        assert_eq!(parse_timestamp("2026-10-03T15:20:00"), None);
        assert_eq!(parse_timestamp("yesterday"), None);
    }

    #[test]
    fn a_reply_logged_twice_counts_once() {
        let dir = std::env::temp_dir().join(format!("ofa-usage-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("project")).unwrap();
        let log = dir.join("project").join("a.jsonl");
        fs::write(&log, format!("{LINE}\n{LINE}\n")).unwrap();

        let mut scanner = Scanner::default();
        let now = 1_790_915_000;
        scanner.scan(&dir, now);
        assert_eq!(scanner.entries().len(), 1);

        // A later scan only reads what was added.
        let second = LINE.replace("msg_011", "msg_012");
        fs::write(&log, format!("{LINE}\n{LINE}\n{second}\n")).unwrap();
        scanner.scan(&dir, now);
        assert_eq!(scanner.entries().len(), 2);

        fs::remove_dir_all(&dir).unwrap();
    }
}
