//! Detects a full-screen app (a game, a video, a presentation) in front of
//! the island, so the island can get out of the way.

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONULL,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetForegroundWindow, GetWindowLongW, GetWindowRect, GWL_STYLE, WS_CAPTION,
};

/// Window classes of the desktop and taskbar. They cover the screen too,
/// but clicking the desktop must not hide the island.
const SHELL_CLASSES: [&str; 4] = [
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
];

/// Whether the foreground window fills the whole monitor the island is on.
///
/// A maximised window is not full screen: it stops at the taskbar, and even
/// with an auto-hiding taskbar it keeps its title bar. A full-screen window
/// covers the entire monitor and has no title bar.
pub fn in_front_of(island: HWND) -> bool {
    // SAFETY: all calls below only read window state. They fail or return
    // null for a window that has just closed, which reads as "not full screen".
    unsafe {
        let foreground = GetForegroundWindow();
        if foreground.is_invalid() || foreground == island || is_shell(foreground) {
            return false;
        }

        let monitor = MonitorFromWindow(foreground, MONITOR_DEFAULTTONULL);
        if monitor.is_invalid() || monitor != MonitorFromWindow(island, MONITOR_DEFAULTTONULL) {
            return false;
        }
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return false;
        }

        let mut window = RECT::default();
        if GetWindowRect(foreground, &mut window).is_err() {
            return false;
        }

        let style = GetWindowLongW(foreground, GWL_STYLE) as u32;
        let has_title_bar = style & WS_CAPTION.0 == WS_CAPTION.0;
        covers(&window, &info.rcMonitor) && !has_title_bar
    }
}

fn covers(window: &RECT, monitor: &RECT) -> bool {
    window.left <= monitor.left
        && window.top <= monitor.top
        && window.right >= monitor.right
        && window.bottom >= monitor.bottom
}

fn is_shell(window: HWND) -> bool {
    let mut buf = [0u16; 64];
    // SAFETY: GetClassNameW writes at most buf.len() characters.
    let len = unsafe { GetClassNameW(window, &mut buf) } as usize;
    let class = String::from_utf16_lossy(&buf[..len]);
    SHELL_CLASSES.contains(&class.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
        RECT {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn exact_and_oversized_windows_cover_the_monitor() {
        let monitor = rect(0, 0, 1920, 1080);
        assert!(covers(&rect(0, 0, 1920, 1080), &monitor));
        assert!(covers(&rect(-8, -8, 1928, 1088), &monitor));
    }

    #[test]
    fn a_window_above_the_taskbar_does_not_cover_it() {
        let monitor = rect(0, 0, 1920, 1080);
        assert!(!covers(&rect(-8, -8, 1928, 1040), &monitor));
    }

    #[test]
    fn coordinates_on_a_second_monitor_are_compared_as_is() {
        let monitor = rect(1920, -200, 4480, 1240);
        assert!(covers(&rect(1920, -200, 4480, 1240), &monitor));
        assert!(!covers(&rect(0, 0, 1920, 1080), &monitor));
    }
}
