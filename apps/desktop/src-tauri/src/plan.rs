//! Exact plan usage, read from the same service Claude Code's `/usage`
//! screen uses. Opt-in: it signs in with Claude Code's saved sign-in, so it
//! only runs when `exact_usage` is on in OFA's settings.
//!
//! OFA only ever reads that sign-in. It never refreshes or rewrites it,
//! because a refresh would sign Claude Code out. When the saved sign-in has
//! expired, OFA waits for Claude Code to renew it and shows the estimate.

use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

use crate::usage::parse_timestamp;

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const TIMEOUT: Duration = Duration::from_secs(10);

/// One limit as Claude reports it.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct Limit {
    /// 0 to 100.
    pub percent: f64,
    /// Seconds since the Unix epoch.
    pub resets_at: Option<i64>,
}

/// The 5-hour and weekly limits.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct PlanUsage {
    pub session: Limit,
    pub weekly: Option<Limit>,
}

/// Whether the opt-in is on: `"exact_usage": true` in
/// `%APPDATA%\OFA\settings.json`. Read each time, so a change applies
/// without a restart.
pub fn enabled() -> bool {
    #[derive(Deserialize, Default)]
    struct Settings {
        #[serde(default)]
        exact_usage: bool,
    }
    let Some(path) = settings_path() else {
        return false;
    };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Settings>(text.trim_start_matches('\u{feff}')).ok())
        .is_some_and(|s| s.exact_usage)
}

fn settings_path() -> Option<PathBuf> {
    let token = ofa_protocol::token_path()?;
    Some(token.with_file_name("settings.json"))
}

/// Asks Claude for the current limits. `Err` explains why not, briefly.
pub fn fetch() -> Result<PlanUsage, String> {
    let token = access_token()?;
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .http_status_as_error(false)
        .build()
        .new_agent();
    let mut response = agent
        .get(USAGE_URL)
        .header("Authorization", &format!("Bearer {token}"))
        .header("anthropic-beta", "oauth-2025-04-20")
        .header("User-Agent", concat!("OFA/", env!("CARGO_PKG_VERSION")))
        .call()
        .map_err(|err| format!("couldn't reach Claude: {err}"))?;
    match response.status().as_u16() {
        200 => {}
        401 | 403 => return Err("Claude Code's sign-in was refused".into()),
        status => return Err(format!("Claude answered {status}")),
    }
    let body = response
        .body_mut()
        .read_to_string()
        .map_err(|err| format!("couldn't read Claude's answer: {err}"))?;
    parse_usage(&body).ok_or_else(|| "Claude's answer had no 5-hour limit".into())
}

/// The access token from Claude Code's saved sign-in, if it hasn't expired.
fn access_token() -> Result<String, String> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct SignIn {
        access_token: String,
        expires_at: i64,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct File {
        claude_ai_oauth: Option<SignIn>,
    }

    let base = match std::env::var_os("CLAUDE_CONFIG_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => {
            PathBuf::from(std::env::var_os("USERPROFILE").ok_or("no user profile")?).join(".claude")
        }
    };
    let text = std::fs::read_to_string(base.join(".credentials.json"))
        .map_err(|_| "Claude Code isn't signed in on this PC".to_owned())?;
    let file: File =
        serde_json::from_str(&text).map_err(|_| "unexpected sign-in file".to_owned())?;
    let sign_in = file
        .claude_ai_oauth
        .ok_or("Claude Code isn't signed in with a Claude plan")?;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    if sign_in.expires_at <= now_ms {
        return Err(
            "Claude Code's sign-in has expired; it renews when you next use Claude Code".into(),
        );
    }
    Ok(sign_in.access_token)
}

/// Reads `five_hour` and `seven_day` from the service's answer.
fn parse_usage(body: &str) -> Option<PlanUsage> {
    #[derive(Deserialize)]
    struct RawLimit {
        utilization: Option<f64>,
        resets_at: Option<String>,
    }
    #[derive(Deserialize)]
    struct Raw {
        five_hour: Option<RawLimit>,
        seven_day: Option<RawLimit>,
    }
    let raw: Raw = serde_json::from_str(body).ok()?;
    let limit = |raw: RawLimit| {
        Some(Limit {
            percent: raw.utilization?,
            resets_at: raw.resets_at.as_deref().and_then(parse_timestamp),
        })
    };
    Some(PlanUsage {
        session: raw.five_hour.and_then(limit)?,
        weekly: raw.seven_day.and_then(limit),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Shape of a real answer, trimmed.
    const ANSWER: &str = r#"{"five_hour":{"utilization":89.0,"resets_at":"2026-10-03T15:20:00.361993+00:00","limit_dollars":null},"seven_day":{"utilization":35.0,"resets_at":"2026-10-08T06:00:00.362013+00:00"},"seven_day_opus":null,"extra_usage":{"is_enabled":false}}"#;

    #[test]
    fn reads_both_limits() {
        let usage = parse_usage(ANSWER).unwrap();
        assert_eq!(usage.session.percent, 89.0);
        assert_eq!(usage.session.resets_at, Some(1_791_040_800));
        let weekly = usage.weekly.unwrap();
        assert_eq!(weekly.percent, 35.0);
        assert_eq!(weekly.resets_at, Some(1_791_439_200));
    }

    #[test]
    fn a_missing_weekly_limit_is_fine() {
        let usage = parse_usage(r#"{"five_hour":{"utilization":5,"resets_at":null}}"#).unwrap();
        assert_eq!(usage.session.percent, 5.0);
        assert_eq!(usage.session.resets_at, None);
        assert!(usage.weekly.is_none());
    }

    #[test]
    fn no_five_hour_limit_is_no_answer() {
        assert!(parse_usage(r#"{"five_hour":null}"#).is_none());
        assert!(parse_usage("not json").is_none());
    }
}
