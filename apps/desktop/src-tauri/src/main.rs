// Stops a console window from opening next to the island in release builds.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() {
    ofa_desktop_lib::run()
}
