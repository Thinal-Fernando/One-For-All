//! Estimates plan usage from the token counts Claude Code writes to its
//! local session logs.
//!
//! Claude plans limit use over rolling 5-hour windows. A window opens with
//! the first message after the previous one has ended, starting at the top
//! of that hour, and resets 5 hours later. This only estimates the window
//! from your own logs: messages from another computer, or from claude.ai in
//! a browser, aren't in them.

/// Length of one usage window, in seconds.
pub const WINDOW_SECS: i64 = 5 * 60 * 60;

/// How far back the weekly figure looks, in seconds.
pub const WEEK_SECS: i64 = 7 * 24 * 60 * 60;

/// One reply from Claude: when it was written and the tokens it used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsageEntry {
    /// Seconds since the Unix epoch.
    pub at: i64,
    pub tokens: u64,
}

/// What the island shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UsageSummary {
    /// Tokens used in the window that is open now; 0 if none is open.
    pub window_tokens: u64,
    /// When the open window resets, in seconds since the Unix epoch.
    pub window_resets_at: Option<i64>,
    /// Tokens used over the past 7 days.
    pub week_tokens: u64,
}

/// Works out the open window and the weekly total. `entries` may be in any
/// order.
pub fn summarize(entries: &[UsageEntry], now: i64) -> UsageSummary {
    let mut sorted: Vec<UsageEntry> = entries.iter().copied().filter(|e| e.at <= now).collect();
    sorted.sort_by_key(|e| e.at);

    let week_tokens = sorted
        .iter()
        .filter(|e| e.at > now - WEEK_SECS)
        .map(|e| e.tokens)
        .sum();

    // Walk the windows in order; only the last one can still be open.
    let mut window_start = None;
    let mut window_tokens = 0;
    for entry in &sorted {
        let open = window_start.is_some_and(|start| entry.at < start + WINDOW_SECS);
        if !open {
            window_start = Some(top_of_hour(entry.at));
            window_tokens = 0;
        }
        window_tokens += entry.tokens;
    }

    match window_start.map(|start| start + WINDOW_SECS) {
        Some(resets_at) if now < resets_at => UsageSummary {
            window_tokens,
            window_resets_at: Some(resets_at),
            week_tokens,
        },
        _ => UsageSummary {
            week_tokens,
            ..UsageSummary::default()
        },
    }
}

fn top_of_hour(at: i64) -> i64 {
    at - at.rem_euclid(3600)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: i64 = 3600;
    /// 2026-10-03 09:00:00 UTC.
    const NINE: i64 = 1_791_018_000;

    fn at(at: i64, tokens: u64) -> UsageEntry {
        UsageEntry { at, tokens }
    }

    #[test]
    fn nothing_used_means_no_window() {
        assert_eq!(summarize(&[], NINE), UsageSummary::default());
    }

    #[test]
    fn a_window_opens_at_the_top_of_the_hour() {
        let first = NINE + 25 * 60; // 09:25
        let s = summarize(&[at(first, 100), at(first + 60, 50)], first + 120);
        assert_eq!(s.window_tokens, 150);
        assert_eq!(s.window_resets_at, Some(NINE + 5 * HOUR)); // 14:00
        assert_eq!(s.week_tokens, 150);
    }

    #[test]
    fn a_message_after_the_reset_opens_a_new_window() {
        let entries = [at(NINE + 10, 100), at(NINE + 5 * HOUR + 30 * 60, 7)];
        let s = summarize(&entries, NINE + 6 * HOUR);
        assert_eq!(s.window_tokens, 7);
        assert_eq!(s.window_resets_at, Some(NINE + 10 * HOUR));
        assert_eq!(s.week_tokens, 107);
    }

    #[test]
    fn an_expired_window_shows_nothing_open() {
        let s = summarize(&[at(NINE, 100)], NINE + 5 * HOUR);
        assert_eq!(s.window_tokens, 0);
        assert_eq!(s.window_resets_at, None);
        assert_eq!(s.week_tokens, 100);
    }

    #[test]
    fn the_week_only_counts_the_last_seven_days() {
        let now = NINE;
        let entries = [at(now - WEEK_SECS - 1, 1000), at(now - WEEK_SECS + 1, 5)];
        assert_eq!(summarize(&entries, now).week_tokens, 5);
    }

    #[test]
    fn order_doesnt_matter_and_the_future_is_ignored() {
        let entries = [
            at(NINE + 3 * HOUR, 2),
            at(NINE, 1),
            at(NINE + 100 * HOUR, 99),
        ];
        let s = summarize(&entries, NINE + 4 * HOUR);
        assert_eq!(s.window_tokens, 3);
        assert_eq!(s.week_tokens, 3);
    }
}
