//! The desktop app: tray icon, HTML flyout and window, in one Tauri process.
//!
//! One process rather than two binaries because the flyout has to be positioned by the
//! tray icon that opened it, and that is only knowable inside the process that owns the
//! icon. The CLI (`axio quota`) stays entirely separate and shares only the probe layer.

mod commands;
mod cost;
mod icon;
mod state;
mod tray;

use std::sync::Arc;

use tauri::{Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};

use state::AppState;

pub const MAIN_WINDOW: &str = "main";
pub const FLYOUT_WINDOW: &str = "flyout";

pub fn run() -> Result<(), String> {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            commands::overview,
            commands::refresh_now,
            commands::history,
            commands::settings,
            commands::save_settings,
            commands::cost_report,
            commands::refresh_cost,
            commands::open_main_window,
            commands::hide_flyout,
            commands::quit,
        ])
        .setup(|app| {
            app.manage(Arc::new(AppState::new()));
            // Separate from AppState: the quota probes refresh on a timer, the cost scan
            // is expensive and refreshes only when asked. One lock each keeps a slow
            // scan from blocking the tray's next update.
            app.manage(Arc::new(cost::CostCache::default()));
            let handle = app.handle().clone();

            // Both windows exist from the start and are only shown on demand. Creating a
            // webview lazily costs a visible delay on first open, which for a panel that
            // is supposed to feel like part of the shell is the whole experience.
            build_main_window(&handle)?;
            build_flyout_window(&handle)?;

            tray::build(&handle)?;
            tray::update_icon(&handle);
            state::spawn_refresh_loop(&handle);
            Ok(())
        })
        .on_window_event(|window, event| match event {
            // Closing the window must not end the process — the tray is the app, and the
            // window is one of its surfaces.
            WindowEvent::CloseRequested { api, .. } if window.label() == MAIN_WINDOW => {
                api.prevent_close();
                let _ = window.hide();
            }
            // A flyout that stays up after you click elsewhere is a window, not a flyout.
            WindowEvent::Focused(false) if window.label() == FLYOUT_WINDOW => {
                let _ = window.hide();
            }
            _ => {}
        })
        .run(tauri::generate_context!())
        .map_err(|err| err.to_string())
}

fn build_main_window(app: &tauri::AppHandle) -> tauri::Result<()> {
    WebviewWindowBuilder::new(app, MAIN_WINDOW, WebviewUrl::App("index.html".into()))
        .title("axio quota")
        .inner_size(860.0, 620.0)
        .min_inner_size(620.0, 420.0)
        .visible(false)
        .build()?;
    Ok(())
}

fn build_flyout_window(app: &tauri::AppHandle) -> tauri::Result<()> {
    WebviewWindowBuilder::new(
        app,
        FLYOUT_WINDOW,
        // Same bundle, different route. One frontend build, two surfaces.
        WebviewUrl::App("index.html#flyout".into()),
    )
    .title("axio quota")
    .inner_size(340.0, 460.0)
    .decorations(false)
    .resizable(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .visible(false)
    .build()?;
    Ok(())
}
