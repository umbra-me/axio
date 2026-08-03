//! The desktop app: tray icon, HTML flyout and window, in one Tauri process.
//!
//! One process rather than two binaries because the flyout has to be positioned by the
//! tray icon that opened it, and that is only knowable inside the process that owns the
//! icon. The CLI (`axio quota`) stays entirely separate and shares only the probe layer.

mod commands;
mod connect;
mod cost;
mod icon;
mod schedule;
mod state;
mod tray;
mod view;

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
            commands::cost_stats,
            commands::refresh_cost,
            commands::storage,
            commands::refresh_cadence,
            commands::set_refresh_cadence,
            commands::connect_provider,
            commands::capture_provider,
            commands::open_main_window,
            commands::minimize_window,
            commands::close_window,
            commands::hide_flyout,
            commands::quit,
        ])
        .setup(|app| {
            let state = Arc::new(AppState::new());
            // Separate from AppState: the quota probes refresh on a timer, the cost scan
            // is expensive and refreshes only when asked. One lock each keeps a slow
            // scan from blocking the tray's next update.
            let costs = Arc::new(cost::CostCache::new(axio_cost::store::cache_path(
                &crate::paths::local_data_dir(&state.env).join("axio"),
            )));
            app.manage(Arc::clone(&state));
            app.manage(Arc::clone(&costs));
            let handle = app.handle().clone();

            // Both windows exist from the start and are only shown on demand. Creating a
            // webview lazily costs a visible delay on first open, which for a panel that
            // is supposed to feel like part of the shell is the whole experience.
            build_main_window(&handle)?;
            build_flyout_window(&handle)?;

            tray::build(&handle)?;
            tray::update_icon(&handle);
            state::spawn_refresh_loop(&handle);
            connect::spawn_probe(&handle);

            // Start scanning now rather than when the Cost tab is first opened. The saved
            // scan lands in milliseconds and the live one replaces it while the user is
            // still looking at Providers, so the tab is populated before it is reached.
            let notify = handle.clone();
            costs.rescan(move || {
                use tauri::Emitter;
                let _ = notify.emit(cost::EVENT_COST_UPDATED, ());
            });
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
        .inner_size(880.0, 640.0)
        .min_inner_size(640.0, 440.0)
        // The frontend draws its own titlebar. A tray app is a panel rather than a
        // document window, and the OS chrome — a title, an icon, a menu strip — states
        // things this surface has no use for while costing 32px of the only vertical
        // space the tables have.
        .decorations(false)
        // Without decorations the OS stops rounding the corners, so the window says what
        // shape it is.
        .shadow(true)
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
