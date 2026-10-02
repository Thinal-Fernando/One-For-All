//! Finds the agent process that ran `ofa hook`.
//!
//! Claude Code starts hooks directly, so the parent is normally claude.exe.
//! If something sits in between (a shell, a wrapper), walk up a few levels
//! to find it. The app watches this process to notice a closed terminal.

use std::collections::HashMap;

use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};

/// How far up the process tree to look for the agent.
const MAX_DEPTH: usize = 4;

/// The nearest ancestor named `agent_exe`, or the direct parent if none is.
pub fn agent_pid(agent_exe: &str) -> Option<u32> {
    let processes = snapshot()?;
    find_agent(&processes, std::process::id(), agent_exe)
}

/// pid -> (parent pid, exe name), for every running process.
type Processes = HashMap<u32, (u32, String)>;

fn find_agent(processes: &Processes, me: u32, agent_exe: &str) -> Option<u32> {
    let parent = processes.get(&me)?.0;
    let mut pid = parent;
    for _ in 0..MAX_DEPTH {
        let Some((next, name)) = processes.get(&pid) else {
            break;
        };
        if name.eq_ignore_ascii_case(agent_exe) {
            return Some(pid);
        }
        pid = *next;
    }
    Some(parent)
}

fn snapshot() -> Option<Processes> {
    // SAFETY: the snapshot handle is closed before returning, and each
    // PROCESSENTRY32W is sized as the API requires.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut processes = HashMap::new();
        let mut more = Process32FirstW(snap, &mut entry).is_ok();
        while more {
            let len = entry
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..len]);
            processes.insert(entry.th32ProcessID, (entry.th32ParentProcessID, name));
            more = Process32NextW(snap, &mut entry).is_ok();
        }
        let _ = CloseHandle(snap);
        Some(processes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(entries: &[(u32, u32, &str)]) -> Processes {
        entries
            .iter()
            .map(|&(pid, parent, name)| (pid, (parent, name.to_owned())))
            .collect()
    }

    #[test]
    fn the_direct_parent_is_the_agent() {
        let p = tree(&[
            (10, 5, "ofa.exe"),
            (5, 1, "claude.exe"),
            (1, 0, "explorer.exe"),
        ]);
        assert_eq!(find_agent(&p, 10, "claude.exe"), Some(5));
    }

    #[test]
    fn a_shell_in_between_is_skipped() {
        let p = tree(&[
            (10, 7, "ofa.exe"),
            (7, 5, "bash.exe"),
            (5, 1, "Claude.exe"),
            (1, 0, "explorer.exe"),
        ]);
        assert_eq!(find_agent(&p, 10, "claude.exe"), Some(5));
    }

    #[test]
    fn falls_back_to_the_parent() {
        let p = tree(&[
            (10, 7, "ofa.exe"),
            (7, 1, "node.exe"),
            (1, 0, "explorer.exe"),
        ]);
        assert_eq!(find_agent(&p, 10, "claude.exe"), Some(7));
    }

    #[test]
    fn this_process_has_a_parent() {
        assert!(agent_pid("cargo.exe").is_some());
    }
}
