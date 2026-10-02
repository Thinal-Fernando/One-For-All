//! Keeps the island above other windows.
//!
//! Tauri marks the window always-on-top once, when it is created, and Windows
//! doesn't keep it there for good: any other always-on-top window that comes
//! to the front is placed above ours, and stays there. So whenever the
//! foreground window changes, the island puts itself back on top.

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, SetWindowPos, HWND_TOPMOST, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE,
};

pub struct Watchdog {
    island: HWND,
    foreground: HWND,
}

impl Watchdog {
    pub fn new(island: HWND) -> Self {
        Self {
            island,
            foreground: HWND::default(),
        }
    }

    /// Called on every poll tick. Cheap unless the foreground window changed.
    pub fn tick(&mut self) {
        // SAFETY: GetForegroundWindow has no preconditions.
        let foreground = unsafe { GetForegroundWindow() };
        if foreground == self.foreground {
            return;
        }
        self.foreground = foreground;
        if foreground != self.island {
            self.raise();
        }
    }

    fn raise(&self) {
        // The window belongs to the main thread, so ask asynchronously rather
        // than block this thread until the main thread gets to it. Size,
        // position and focus are left alone.
        let flags =
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_ASYNCWINDOWPOS;
        // SAFETY: SetWindowPos fails harmlessly if the handle is no longer valid.
        if let Err(err) =
            unsafe { SetWindowPos(self.island, Some(HWND_TOPMOST), 0, 0, 0, 0, flags) }
        {
            eprintln!("island: could not stay on top: {err}");
        }
    }
}
