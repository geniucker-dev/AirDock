#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    airplay_windows::integration::attach_parent_console();
    if let Err(e) = airplay_windows::desktop::run() {
        eprintln!("{e:#}");
        airplay_windows::integration::startup_error(&format!("{e:#}"));
        std::process::exit(1);
    }
}
