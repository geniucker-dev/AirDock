// SPDX-License-Identifier: MPL-2.0
#![cfg_attr(windows, windows_subsystem = "windows")]
// Keep this executable independent of airdock::lib and its FFmpeg DLL imports.
#[path = "update/install.rs"]
mod install;
#[path = "update/manifest.rs"]
mod manifest;
fn main() {
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--version")) {
        println!("AirDock updater {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if let Err(error) = install::run() {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
}
