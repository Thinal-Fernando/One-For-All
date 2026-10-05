//! Window controller for the island.
//!
//! The island window is click-through whenever the cursor is away from it, so
//! it never blocks the app behind it. A click-through window gets no mouse
//! events at all, so it can't notice the cursor arriving. Instead, a background
//! thread polls the cursor position and compares it with the area the UI
//! reports as solid. The same thread runs the always-on-top watchdog and
//! hides the island while a full-screen app is in front. About once a
//! second it also checks the island still sits on the edge of the primary
//! monitor chosen in the settings, which moves when monitors, scaling or the
//! setting change.
//!
//! The window is a transparent strip against that edge, big enough for the
//! orb, the ripple it makes and the pop-up; only what the UI paints is solid.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewWindow};
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

use crate::settings::{self, Edge, IslandSettings};
use crate::{fullscreen, topmost};

/// Label of the single island window, as declared in tauri.conf.json.
pub const ISLAND: &str = "island";

/// Event sent to the UI when the island should open (`true`) or close (`false`).
pub const HOVER_EVENT: &str = "island-hover";

/// Event sent to the UI when the orb's edge or size changes.
pub const LAYOUT_EVENT: &str = "island-layout";

/// The window's size in CSS pixels: a strip along a side edge, or a band
/// along the top, with room for the pop-up, the ripple around the orb, and
/// the panel beside the pop-up that shows a permission request in full.
const SIDE_WINDOW: (f64, f64) = (1000.0, 720.0);
const TOP_WINDOW: (f64, f64) = (1240.0, 720.0);

/// How often the cursor is polled, about 30 times a second.
const TICK: Duration = Duration::from_millis(33);

/// How long the island stays open after the cursor leaves it.
const CLOSE_DELAY: Duration = Duration::from_millis(300);

/// Check for a full-screen app every this many ticks, about 4 times a second.
const FULLSCREEN_EVERY: u32 = 8;

/// Check the island's position every this many ticks, about once a second.
const PLACE_EVERY: u32 = 30;

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
    /// The orb's edge and size the UI was last told about.
    layout: Mutex<Option<IslandSettings>>,
    /// Hidden from the tray menu, until shown again or OFA restarts.
    hidden_by_user: AtomicBool,
}

/// Hides or shows the orb, from the tray menu.
pub fn set_hidden_by_user(app: &AppHandle, hidden: bool) {
    app.state::<IslandState>()
        .hidden_by_user
        .store(hidden, Ordering::Relaxed);
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

/// Tells the UI which edge the orb sits on and how big it is.
#[tauri::command]
pub fn get_island_layout(state: tauri::State<'_, IslandState>) -> IslandSettings {
    state
        .layout
        .lock()
        .unwrap()
        .unwrap_or_else(|| settings::load().island)
}

/// The window's frame for `edge` on a monitor at `monitor_origin` of
/// `monitor_size`, all in physical pixels: flush against that edge, centred
/// along it.
fn frame(
    edge: Edge,
    monitor_origin: PhysicalPosition<i32>,
    monitor_size: PhysicalSize<u32>,
    scale: f64,
) -> (PhysicalPosition<i32>, PhysicalSize<u32>) {
    let (w, h) = match edge {
        Edge::Top => TOP_WINDOW,
        Edge::Left | Edge::Right => SIDE_WINDOW,
    };
    // Never bigger than the monitor itself.
    let size = PhysicalSize::new(
        ((w * scale).round() as u32).min(monitor_size.width),
        ((h * scale).round() as u32).min(monitor_size.height),
    );
    let (mx, my) = (monitor_origin.x, monitor_origin.y);
    let (mw, mh) = (monitor_size.width as i32, monitor_size.height as i32);
    let (ww, wh) = (size.width as i32, size.height as i32);
    let origin = match edge {
        Edge::Right => PhysicalPosition::new(mx + mw - ww, my + (mh - wh) / 2),
        Edge::Left => PhysicalPosition::new(mx, my + (mh - wh) / 2),
        Edge::Top => PhysicalPosition::new(mx + (mw - ww) / 2, my),
    };
    (origin, size)
}

/// Places the island on the chosen edge of the primary monitor and records
/// where it went. Does nothing if it is already there.
///
/// Windows moves windows itself when a monitor is plugged in or removed, and
/// a scaling change resizes the window, so this compares against where the
/// window really is rather than where it was last put.
pub fn place(window: &WebviewWindow) -> tauri::Result<()> {
    let Some(monitor) = window.primary_monitor()? else {
        return Ok(());
    };
    let layout = settings::load().island;
    // The webview scales CSS pixels by the factor of the monitor the window
    // is on; once the window is in place, that's the primary monitor's.
    let scale = monitor.scale_factor();
    let (origin, size) = frame(layout.edge, *monitor.position(), *monitor.size(), scale);
    if window.outer_size()? != size {
        window.set_size(size)?;
    }
    if window.outer_position()? != origin {
        window.set_position(origin)?;
    }

    let state = window.state::<IslandState>();
    *state.placement.lock().unwrap() = Some(Placement { origin, scale });
    let changed = state.layout.lock().unwrap().replace(layout) != Some(layout);
    if changed {
        if let Err(err) = window.emit_to(ISLAND, LAYOUT_EVENT, layout) {
            eprintln!("island: could not send the layout: {err}");
        }
    }
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
    let mut full_screen = false;
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

        if tick.is_multiple_of(PLACE_EVERY) {
            if let Err(err) = place(&window) {
                eprintln!("island: could not place the island: {err}");
            }
        }

        if let Some(hwnd) = hwnd.filter(|_| tick.is_multiple_of(FULLSCREEN_EVERY)) {
            full_screen = fullscreen::in_front_of(hwnd);
        }
        let hide = full_screen || state.hidden_by_user.load(Ordering::Relaxed);
        if hide != hidden {
            hidden = hide;
            let result = if hidden { window.hide() } else { window.show() };
            if let Err(err) = result {
                eprintln!("island: could not hide or show: {err}");
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
    use super::*;

    /// A 1920x1080 monitor at 150% scaling, to the right of another one.
    fn on_second_monitor(edge: Edge) -> (PhysicalPosition<i32>, PhysicalSize<u32>) {
        frame(
            edge,
            PhysicalPosition::new(1920, 0),
            PhysicalSize::new(1920, 1080),
            1.5,
        )
    }

    #[test]
    fn the_right_edge_strip_is_flush_and_centred() {
        let (origin, size) = on_second_monitor(Edge::Right);
        assert_eq!(size, PhysicalSize::new(1500, 1080));
        assert_eq!(origin, PhysicalPosition::new(1920 + 1920 - 1500, 0));
    }

    #[test]
    fn the_window_never_outgrows_a_small_monitor() {
        let (origin, size) = frame(
            Edge::Right,
            PhysicalPosition::new(0, 0),
            PhysicalSize::new(800, 600),
            1.0,
        );
        assert_eq!(size, PhysicalSize::new(800, 600));
        assert_eq!(origin, PhysicalPosition::new(0, 0));
    }

    #[test]
    fn the_left_edge_strip_starts_at_the_monitor() {
        let (origin, _) = on_second_monitor(Edge::Left);
        assert_eq!(origin.x, 1920);
    }

    #[test]
    fn the_top_band_is_centred_along_the_top() {
        let (origin, size) = on_second_monitor(Edge::Top);
        assert_eq!(size, PhysicalSize::new(1860, 1080));
        assert_eq!(origin, PhysicalPosition::new(1920 + (1920 - 1860) / 2, 0));
    }

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
