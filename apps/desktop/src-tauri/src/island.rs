//! Window controller for the island.
//!
//! The island window is click-through whenever the cursor is away from it, so
//! it never blocks the app behind it. A click-through window gets no mouse
//! events at all, so it can't notice the cursor arriving. Instead, a background
//! thread polls the cursor position and compares it with the area the UI
//! reports as solid. The same thread runs the always-on-top watchdog and
//! hides the island while a full-screen app is in front.

use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, WebviewWindow};
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

use crate::{fullscreen, topmost};

/// Label of the single island window, as declared in tauri.conf.json.
pub const ISLAND: &str = "island";

/// Event sent to the UI when the island should open (`true`) or close (`false`).
pub const HOVER_EVENT: &str = "island-hover";

/// How often the cursor is polled, about 30 times a second.
const TICK: Duration = Duration::from_millis(33);

/// How long the island stays open after the cursor leaves it.
const CLOSE_DELAY: Duration = Duration::from_millis(300);

/// Check for a full-screen app every this many ticks, about 4 times a second.
const FULLSCREEN_EVERY: u32 = 8;

/// A rectangle in CSS pixels, relative to the window's top-left corner.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }
}

/// Where the window sits on screen and how it is scaled. Kept here so the
/// poll loop never has to ask the main thread.
#[derive(Debug, Clone, Copy)]
struct Placement {
    origin: PhysicalPosition<i32>,
    scale: f64,
}

/// Shared between the poll thread and the `set_hit_area` command.
#[derive(Default)]
pub struct IslandState {
    /// The part of the window the UI has painted, which catches the mouse.
    hit_area: Mutex<Rect>,
    placement: Mutex<Option<Placement>>,
}

/// Called by the UI whenever the painted island changes size.
#[tauri::command]
pub fn set_hit_area(state: tauri::State<'_, IslandState>, x: f64, y: f64, width: f64, height: f64) {
    *state.hit_area.lock().unwrap() = Rect {
        x,
        y,
        width,
        height,
    };
}

/// Places the island at the top centre of the primary monitor and records
/// where it went.
pub fn place_top_centre(window: &WebviewWindow) -> tauri::Result<()> {
    let Some(monitor) = window.primary_monitor()? else {
        return Ok(());
    };
    let area = monitor.position();
    let screen = monitor.size();
    let size = window.outer_size()?;
    let origin = PhysicalPosition::new(
        area.x + (screen.width as i32 - size.width as i32) / 2,
        area.y,
    );
    window.set_position(origin)?;

    let state = window.state::<IslandState>();
    *state.placement.lock().unwrap() = Some(Placement {
        origin,
        scale: monitor.scale_factor(),
    });
    Ok(())
}

/// Makes the window click-through and starts the cursor poll thread.
pub fn start(app: &AppHandle, window: WebviewWindow) -> tauri::Result<()> {
    window.set_ignore_cursor_events(true)?;
    let app = app.clone();
    thread::Builder::new()
        .name("island".into())
        .spawn(move || poll(app, window))?;
    Ok(())
}

fn poll(app: AppHandle, window: WebviewWindow) {
    let state = app.state::<IslandState>();
    let hwnd = match window.hwnd() {
        Ok(hwnd) => Some(hwnd),
        Err(err) => {
            eprintln!(
                "island: no window handle, so no on-top watchdog or full-screen hiding: {err}"
            );
            None
        }
    };
    let mut watchdog = hwnd.map(topmost::Watchdog::new);
    let mut hidden = false;
    let mut click_through = true;
    let mut open = false;
    let mut last_inside = Instant::now();
    let mut tick: u32 = 0;

    loop {
        thread::sleep(TICK);
        tick = tick.wrapping_add(1);

        if let Some(watchdog) = &mut watchdog {
            watchdog.tick();
        }

        if let Some(hwnd) = hwnd.filter(|_| tick.is_multiple_of(FULLSCREEN_EVERY)) {
            let full_screen = fullscreen::in_front_of(hwnd);
            if full_screen != hidden {
                hidden = full_screen;
                let result = if hidden { window.hide() } else { window.show() };
                if let Err(err) = result {
                    eprintln!("island: could not hide or show for full screen: {err}");
                }
            }
        }

        // While hidden the island can't be hovered, so it closes as if the
        // cursor had left.
        let inside = !hidden && cursor_inside(&state);

        // Clicks must pass through the moment the cursor leaves, even while
        // the island is still closing, or the empty part of the window
        // would swallow them.
        if inside == click_through {
            click_through = !inside;
            if let Err(err) = window.set_ignore_cursor_events(click_through) {
                eprintln!("island: could not set click-through: {err}");
            }
        }

        if inside {
            last_inside = Instant::now();
        }
        let should_open = inside || (open && last_inside.elapsed() < CLOSE_DELAY);
        if should_open != open {
            open = should_open;
            if let Err(err) = window.emit_to(ISLAND, HOVER_EVENT, open) {
                eprintln!("island: could not send hover: {err}");
            }
        }
    }
}

/// Whether the cursor is over the solid part of the island.
fn cursor_inside(state: &IslandState) -> bool {
    let Some(placement) = *state.placement.lock().unwrap() else {
        return false;
    };
    let mut point = POINT::default();
    // SAFETY: GetCursorPos only writes to the POINT we pass in.
    if unsafe { GetCursorPos(&mut point) }.is_err() {
        // Fails while a secure desktop (UAC, lock screen) is showing.
        return false;
    }
    // The process is per-monitor DPI aware, so the cursor and the window
    // origin are both in physical pixels. The hit area is in CSS pixels.
    let x = f64::from(point.x - placement.origin.x) / placement.scale;
    let y = f64::from(point.y - placement.origin.y) / placement.scale;
    state.hit_area.lock().unwrap().contains(x, y)
}

#[cfg(test)]
mod tests {
    use super::Rect;

    #[test]
    fn rect_contains_its_edges_but_not_beyond() {
        let rect = Rect {
            x: 10.0,
            y: 6.0,
            width: 160.0,
            height: 32.0,
        };
        assert!(rect.contains(10.0, 6.0));
        assert!(rect.contains(169.9, 37.9));
        assert!(!rect.contains(170.0, 20.0));
        assert!(!rect.contains(9.9, 20.0));
        assert!(!rect.contains(50.0, 38.0));
    }

    #[test]
    fn empty_rect_contains_nothing() {
        assert!(!Rect::default().contains(0.0, 0.0));
    }
}
