//! Starting the window.
//!
//! Built with `Builder::build` and then `run(closure)` rather than
//! `.run(context)`, because `RunEvent::ExitRequested` cannot be intercepted any
//! other way — and that interception is the only thing standing between a
//! taskbar close and five sessions of abandoned work.
//!
//! Two guards, not one. `WindowEvent::CloseRequested` covers the title-bar
//! button; `RunEvent::ExitRequested` covers Alt+F4 and the taskbar. A webview
//! `beforeunload` handler covers neither.

use tauri::{Emitter, Manager, RunEvent, WindowEvent};

use crate::commands;
use crate::state::AppState;

/// Run the desktop surface until it is closed.
///
/// The supervisor is built by the caller, so this crate resolves no
/// configuration and reads no credential — the same seam `axio-supervisor`
/// keeps for agents, one level up.
pub fn run(state: AppState) -> Result<(), Box<dyn std::error::Error>> {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            commands::snapshot,
            commands::approvals,
            commands::start_session,
            commands::send_prompt,
            commands::cancel_session,
            commands::close_session,
            commands::session_diff,
            commands::resolve_approval,
            commands::window_control,
            commands::hosted_available,
            commands::hosted_list,
            commands::hosted_start,
            commands::hosted_read,
            commands::hosted_write,
            commands::hosted_resize,
            commands::hosted_kill,
        ])
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event
                && commands::has_running_work(&window.app_handle().clone())
            {
                // Refused, and the interface is told why. Closing over running
                // work is a decision, and this is where it gets made rather
                // than discovered afterwards.
                api.prevent_close();
                let _ = window.emit("axio://close-requested", ());
            }
        })
        .build(tauri::generate_context!())?;

    app.run(|handle, event| {
        if let RunEvent::ExitRequested { api, .. } = &event
            && commands::has_running_work(handle)
        {
            api.prevent_exit();
            if let Some(window) = handle.get_webview_window("main") {
                let _ = window.emit("axio://close-requested", ());
            }
        }
    });
    Ok(())
}
