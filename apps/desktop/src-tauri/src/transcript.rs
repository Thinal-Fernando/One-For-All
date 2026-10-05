//! Reads what an agent appends to its session log.
//!
//! Claude Code sends no hook event when you refuse a permission prompt or
//! press Esc mid-turn, so the island would stay on "Needs you" or "Working".
//! It does append a line to the session's transcript straight away, which is
//! what this looks for. Only new lines are read, never the whole file.
//!
//! The same lines say how many tokens a turn used, shown once it finishes.

use std::collections::HashSet;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use serde_json::Value;

/// Most new text read in one go. Lines this far apart are never related.
const MAX_READ: u64 = 4 * 1024 * 1024;

/// The file's current length, which is where the next read starts.
pub fn end_of(path: &Path) -> Option<u64> {
    std::fs::metadata(path).ok().map(|m| m.len())
}

/// Complete lines appended since `offset`, and the offset after them. A line
/// still being written is left for the next read.
pub fn read_new(path: &Path, offset: u64) -> io::Result<(String, u64)> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    if len < offset {
        // Rewritten or truncated: start again from its new end.
        return Ok((String::new(), len));
    }
    let start = offset.max(len.saturating_sub(MAX_READ));
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

/// Whether the new lines record you stopping the agent: refusing a prompt
/// ("[Request interrupted by user for tool use]") or pressing Esc
/// ("[Request interrupted by user]").
pub fn was_interrupted(lines: &str) -> bool {
    lines
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|entry| entry["type"] == "user")
        .any(|entry| match &entry["message"]["content"] {
            Value::String(text) => is_interruption(text),
            Value::Array(parts) => parts
                .iter()
                .filter_map(|part| part["text"].as_str())
                .any(is_interruption),
            _ => false,
        })
}

fn is_interruption(text: &str) -> bool {
    text.starts_with("[Request interrupted by user")
}

/// Tokens Claude's replies in these lines used, counted the same way as the
/// plan usage estimate: input, output and cache writes, but not cache reads.
/// A reply written as several lines counts once.
pub fn tokens_used(lines: &str) -> u64 {
    let mut seen = HashSet::new();
    lines
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|entry| entry["type"] == "assistant")
        .filter(|entry| {
            let id = entry["message"]["id"].as_str().unwrap_or_default();
            id.is_empty() || seen.insert(id.to_owned())
        })
        .map(|entry| {
            let usage = &entry["message"]["usage"];
            let count = |name: &str| usage[name].as_u64().unwrap_or(0);
            count("input_tokens") + count("output_tokens") + count("cache_creation_input_tokens")
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    // Lines as Claude Code 2.1.193 wrote them, trimmed to the fields that matter.
    const REFUSED: &str = r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"[Request interrupted by user for tool use]"}]},"isSidechain":false}"#;
    const ESC: &str = r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"[Request interrupted by user]"}]},"isSidechain":false}"#;
    const REPLY: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Hi! What would you like to do?"}]}}"#;
    const PROMPT: &str = r#"{"type":"user","message":{"role":"user","content":"hello"}}"#;

    #[test]
    fn spots_a_refused_prompt_and_esc() {
        assert!(was_interrupted(&format!("{REPLY}\n{REFUSED}\n")));
        assert!(was_interrupted(&format!("{ESC}\n")));
    }

    #[test]
    fn ordinary_lines_are_not_interruptions() {
        assert!(!was_interrupted(&format!("{PROMPT}\n{REPLY}\n")));
        assert!(!was_interrupted(""));
        assert!(!was_interrupted("not json\n"));
    }

    #[test]
    fn quoting_the_marker_in_a_reply_does_not_count() {
        let quoted = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"[Request interrupted by user]"}]}}"#;
        assert!(!was_interrupted(quoted));
    }

    #[test]
    fn counts_each_reply_once_and_leaves_out_cache_reads() {
        let text = r#"{"type":"assistant","message":{"id":"msg_1","content":[{"type":"text","text":"Hi"}],"usage":{"input_tokens":2,"cache_creation_input_tokens":300,"cache_read_input_tokens":50000,"output_tokens":40}}}"#;
        let tool = r#"{"type":"assistant","message":{"id":"msg_1","content":[{"type":"tool_use"}],"usage":{"input_tokens":2,"cache_creation_input_tokens":300,"cache_read_input_tokens":50000,"output_tokens":40}}}"#;
        let next = r#"{"type":"assistant","message":{"id":"msg_2","usage":{"input_tokens":1,"output_tokens":9}}}"#;
        let lines = format!("{PROMPT}\n{text}\n{tool}\n{next}\n{REFUSED}\n");
        assert_eq!(tokens_used(&lines), 2 + 300 + 40 + 1 + 9);
        assert_eq!(tokens_used(""), 0);
    }

    #[test]
    fn reads_only_new_complete_lines() {
        let path =
            std::env::temp_dir().join(format!("ofa-transcript-{}.jsonl", std::process::id()));
        let mut file = File::create(&path).unwrap();
        writeln!(file, "{PROMPT}").unwrap();
        let start = end_of(&path).unwrap();

        // Nothing new yet.
        assert_eq!(read_new(&path, start).unwrap(), (String::new(), start));

        // A full line, then half of the next one.
        write!(file, "{REFUSED}\n{{\"type\":\"us").unwrap();
        let (text, offset) = read_new(&path, start).unwrap();
        assert_eq!(text, format!("{REFUSED}\n"));
        assert!(was_interrupted(&text));

        // The rest of the half-written line arrives later.
        writeln!(file, "er\"}}").unwrap();
        let (text, _) = read_new(&path, offset).unwrap();
        assert_eq!(text, "{\"type\":\"user\"}\n");

        drop(file);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_rewritten_file_starts_over_at_its_end() {
        let path =
            std::env::temp_dir().join(format!("ofa-transcript-short-{}.jsonl", std::process::id()));
        std::fs::write(&path, "x\n").unwrap();
        assert_eq!(read_new(&path, 1000).unwrap(), (String::new(), 2));
        std::fs::remove_file(&path).unwrap();
    }
}
