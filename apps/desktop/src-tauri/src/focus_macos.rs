//! Brings the application hosting a Claude Code session to the front on macOS.
use std::process::Command;

pub fn bring_to_front(pid: u32) -> bool {
    let script = format!(
        r#"tell application "System Events" to set frontmost of (first process whose unix id is {pid}) to true"#
    );
    Command::new("osascript")
        .args(["-e", &script])
        .status()
        .is_ok_and(|status| status.success())
}
