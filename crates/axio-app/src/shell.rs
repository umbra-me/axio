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

use axio_core::protocol::EventKind;
use tauri::{Emitter, Manager, RunEvent, WindowEvent};
use tauri_plugin_notification::NotificationExt;

use crate::commands;
use crate::state::AppState;

/// Run the desktop surface until it is closed.
///
/// The supervisor is built by the caller, so this crate resolves no
/// configuration and reads no credential — the same seam `axio-supervisor`
/// keeps for agents, one level up.
pub fn run(
    state: AppState,
    events: Option<crate::SessionEvents>,
) -> Result<(), Box<dyn std::error::Error>> {
    let app = tauri::Builder::default()
        // First, as its documentation asks: a second launch must be answered
        // before anything else in this process has been set up. Two windows
        // would be two supervisors over one index and one set of worktrees,
        // each unaware of what the other is running — so the second launch
        // brings the first window forward instead.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            commands::snapshot,
            commands::approvals,
            commands::start_session,
            commands::send_prompt,
            commands::cancel_session,
            commands::close_session,
            commands::session_diff,
            commands::session_transcript,
            commands::open_project,
            commands::add_repository,
            commands::resolve_approval,
            commands::window_control,
            commands::hosted_available,
            commands::hosted_list,
            commands::hosted_start,
            commands::hosted_read,
            commands::hosted_write,
            commands::hosted_resize,
            commands::hosted_kill,
            commands::settings,
            commands::save_settings,
            commands::set_default_model,
            commands::start_group,
            commands::add_to_group,
            commands::rename_session,
            commands::rename_hosted,
            commands::hosted_diff,
            commands::landing,
            commands::land,
            commands::providers,
            commands::reveal,
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

    // Relay the supervisor's own event stream to the window.
    //
    // It was being dropped, and the window polled for state it was already
    // being handed. The relay sends the session id and nothing else: what
    // changed still comes back through `snapshot` and `session_transcript`,
    // so a listener that missed one is late rather than wrong — the same
    // discipline the terminal path follows, for the same reason.
    //
    // The state sees each event before the window is told about it, so the
    // window's read never races the write it was woken for.
    if let Some(mut events) = events {
        let handle = app.handle().clone();
        tauri::async_runtime::spawn(async move {
            while let Some(event) = events.recv().await {
                handle.state::<AppState>().observe(&event);
                let _ = handle.emit("axio://session-activity", event.event.session.to_string());
                attention(&handle, &event.event.kind);
            }
        });
    }

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

/// Say so when an agent needs a person and the window is not in front.
///
/// Two events matter: a question waiting, and a turn that ended. Both are
/// posted as a system notification when the window is unfocused — an agent
/// that stops to ask, in a window behind three others, is an agent that waits
/// an hour — and the badge on the dock icon carries the number of questions
/// waiting, focused or not. Nothing is sent for a turn ending in a focused
/// window: the rail already says it.
fn attention(handle: &tauri::AppHandle, kind: &EventKind) {
    let Some(window) = handle.get_webview_window("main") else {
        return;
    };
    let pending = handle
        .try_state::<AppState>()
        .map(|s| s.approvals().len())
        .unwrap_or(0);
    let _ = window.set_badge_count((pending > 0).then_some(pending as i64));

    let focused = window.is_focused().unwrap_or(true);
    let (title, body) = match kind {
        EventKind::ApprovalRequested { request, .. } => (
            "axio needs you",
            format!("{} — {}", request.subject, request.reason),
        ),
        EventKind::TurnEnded { outcome, .. } if !focused => {
            ("a session finished", format!("{outcome:?}").to_lowercase())
        }
        _ => return,
    };
    if !focused || matches!(kind, EventKind::ApprovalRequested { .. }) {
        let _ = handle
            .notification()
            .builder()
            .title(title)
            .body(body)
            .show();
    }
}
