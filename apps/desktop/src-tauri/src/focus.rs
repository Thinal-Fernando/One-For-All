//! Brings a session's terminal window to the front.
//!
//! The agent's process has no window of its own; the window belongs to
//! whatever hosts it. Walking up from the agent finds that host: Windows
//! Terminal, VS Code, or the Claude desktop app. A classic console window
//! belongs to the console host, so each step up also checks for that.
//! Picking the exact tab inside the host isn't possible from outside.

use std::collections::HashMap;

use windows::core::BOOL;
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindow, GetWindowTextLengthW, GetWindowThreadProcessId, IsIconic,
    IsWindowVisible, SetForegroundWindow, ShowWindow, GW_OWNER, SW_RESTORE,
};

/// How far up from the agent to look for a window.
const MAX_DEPTH: usize = 6;

/// Brings the window hosting `pid` to the front. Returns whether one was found.
pub fn bring_to_front(pid: u32) -> bool {
    let Some(processes) = snapshot() else {
        return false;
    };
    let windows = top_level_windows();
    let Some(window) = host_window(&processes, &windows, pid) else {
        return false;
    };
    // SAFETY: the handle came from EnumWindows just now; both calls fail
    // harmlessly if the window has closed since.
    unsafe {
        if IsIconic(window).as_bool() {
            let _ = ShowWindow(window, SW_RESTORE);
        }
        SetForegroundWindow(window).as_bool()
    }
}

/// pid -> (parent pid, exe name).
type Processes = HashMap<u32, (u32, String)>;

/// The first window found walking up from `pid`: the process's own window,
/// or a console host's window if the process started one.
fn host_window(processes: &Processes, windows: &[(HWND, u32)], pid: u32) -> Option<HWND> {
    let window_of = |owner: u32| windows.iter().find(|(_, p)| *p == owner).map(|(w, _)| *w);
    let mut current = pid;
    for _ in 0..MAX_DEPTH {
        if let Some(window) = window_of(current) {
            return Some(window);
        }
        let console = processes.iter().find(|(_, (parent, name))| {
            *parent == current
                && (name.eq_ignore_ascii_case("conhost.exe")
                    || name.eq_ignore_ascii_case("OpenConsole.exe"))
        });
        if let Some(window) = console.and_then(|(host, _)| window_of(*host)) {
            return Some(window);
        }
        match processes.get(&current) {
            // Past the shell is the desktop itself, whose window isn't a host.
            Some((parent, _)) if processes.get(parent).is_some_and(|(_, n)| is_shell(n)) => break,
            Some((parent, _)) if *parent != 0 && *parent != current => current = *parent,
            _ => break,
        }
    }
    None
}

fn is_shell(name: &str) -> bool {
    name.eq_ignore_ascii_case("explorer.exe")
}

/// Visible, titled, unowned windows: the ones with a taskbar button.
fn top_level_windows() -> Vec<(HWND, u32)> {
    unsafe extern "system" fn collect(window: HWND, list: LPARAM) -> BOOL {
        // SAFETY: `list` is the Vec passed to EnumWindows below, alive for
        // the whole call.
        let list = unsafe { &mut *(list.0 as *mut Vec<(HWND, u32)>) };
        // SAFETY: plain queries on a window handle EnumWindows just gave us.
        unsafe {
            let unowned = GetWindow(window, GW_OWNER).is_err();
            if IsWindowVisible(window).as_bool() && unowned && GetWindowTextLengthW(window) > 0 {
                let mut pid = 0;
                GetWindowThreadProcessId(window, Some(&mut pid));
                list.push((window, pid));
            }
        }
        true.into()
    }
    let mut list: Vec<(HWND, u32)> = Vec::new();
    // SAFETY: the callback only touches `list` through the pointer we pass.
    let _ = unsafe { EnumWindows(Some(collect), LPARAM(&mut list as *mut _ as isize)) };
    list
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

    fn hwnd(n: usize) -> HWND {
        HWND(n as *mut _)
    }

    #[test]
    fn windows_terminal_hosts_the_shell() {
        let p = tree(&[
            (30, 20, "claude.exe"),
            (20, 10, "pwsh.exe"),
            (10, 1, "WindowsTerminal.exe"),
            (11, 10, "OpenConsole.exe"),
        ]);
        let w = [(hwnd(7), 10)];
        assert_eq!(host_window(&p, &w, 30), Some(hwnd(7)));
    }

    #[test]
    fn a_classic_console_belongs_to_its_conhost() {
        let p = tree(&[
            (30, 20, "claude.exe"),
            (20, 1, "powershell.exe"),
            (21, 20, "conhost.exe"),
            (1, 0, "explorer.exe"),
        ]);
        let w = [(hwnd(5), 21), (hwnd(9), 1)];
        assert_eq!(host_window(&p, &w, 30), Some(hwnd(5)));
    }

    #[test]
    fn vs_code_runs_claude_directly() {
        let p = tree(&[
            (30, 12, "claude.exe"),
            (12, 11, "Code.exe"),
            (11, 1, "Code.exe"),
        ]);
        let w = [(hwnd(4), 11)];
        assert_eq!(host_window(&p, &w, 30), Some(hwnd(4)));
    }

    #[test]
    fn the_desktop_is_never_picked() {
        let p = tree(&[
            (30, 20, "claude.exe"),
            (20, 1, "pwsh.exe"),
            (1, 0, "explorer.exe"),
        ]);
        let w = [(hwnd(9), 1)];
        assert_eq!(host_window(&p, &w, 30), None);
    }

    #[test]
    fn no_window_anywhere_finds_nothing() {
        let p = tree(&[(30, 20, "claude.exe"), (20, 0, "services.exe")]);
        assert_eq!(host_window(&p, &[], 30), None);
    }
}
