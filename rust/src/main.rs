// SPDX-License-Identifier: MPL-2.0
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    airdock::platform::attach_parent_console();
    if let Err(e) = airdock::desktop::run() {
        eprintln!("{e:#}");
        airdock::platform::startup_error(&format!("{e:#}"));
        std::process::exit(1);
    }
}
