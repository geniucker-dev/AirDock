#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    airplay_windows::platform::attach_parent_console();
    if let Err(e) = airplay_windows::desktop::run() {
        eprintln!("{e:#}");
        airplay_windows::platform::startup_error(&format!("{e:#}"));
        std::process::exit(1);
    }
}
