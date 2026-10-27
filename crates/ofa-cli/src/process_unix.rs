//! Finds Claude Code's process in the ancestor chain of an OFA hook.
use std::process::Command;

pub fn agent_pid(_agent_exe: &str) -> Option<u32> {
    let output = Command::new("ps").args(["-axo", "pid=,ppid=,comm="]).output().ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    let mut processes = std::collections::HashMap::new();
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let (Some(pid), Some(parent), Some(name)) = (fields.next(), fields.next(), fields.next()) else { continue };
        if let (Ok(pid), Ok(parent)) = (pid.parse::<u32>(), parent.parse::<u32>()) {
            processes.insert(pid, (parent, name.rsplit('/').next().unwrap_or(name).to_owned()));
        }
    }
    let mut pid = processes.get(&std::process::id())?.0;
    let parent = *pid;
    for _ in 0..4 {
        let Some((next, name)) = processes.get(&pid) else { break };
        if name == "claude" || name == "Claude" { return Some(pid); }
        pid = *next;
    }
    Some(parent)
}
