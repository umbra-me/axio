//! The desktop binary.
//!
//! `windows_subsystem = "windows"` on release only: a shipped GUI application
//! must not open a console behind itself, and a debug build keeps one because
//! that is where diagnostics go.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Err(e) = axio_app_lib::run(axio_app_lib::shell_state()) {
        eprintln!("axio-app: {e}");
        std::process::exit(1);
    }
}
