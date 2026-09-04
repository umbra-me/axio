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
            commands::session_turns,
            commands::session_turn_diff,
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
            commands::hosted_stop,
            commands::window_state,
            commands::save_window_state,
            commands::hosted_submit,
            commands::hosted_commands,
            commands::hosted_transcript,
            commands::hosted_approvals,
            commands::hosted_decide,
            commands::hosted_resume,
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

    // The terminals the last window had come back running, not as a list
    // of things to click. Each starts again in its own worktree, with its
    // tool asked to continue; one whose directory is gone stays ended and
    // says so in its pane. The pane size is not known yet — the first
    // attach resizes it.
    // On the async runtime, not the main thread: the resume spawns the
    // activity relay as a task, which needs a reactor to be spawned from.
    {
        let handle = app.handle().clone();
        tauri::async_runtime::spawn(async move {
            let state = handle.state::<AppState>();
            // The hook listener first, so the terminals resumed below
            // already carry it.
            let sink_handle = handle.clone();
            let sink: crate::hooks::Sink = std::sync::Arc::new(move |terminal, event, payload| {
                let state = sink_handle.state::<AppState>();
                state.hosted.observe_hook(&terminal, &event, &payload);
                let _ = sink_handle.emit("axio://hosted-activity", terminal.clone());
                if let Some(view) = state.hosted.list().into_iter().find(|v| v.id == terminal) {
                    hosted_attention(&sink_handle, &view);
                }
            });
            match crate::hooks::serve(&axio::home(), sink).await {
                Ok(endpoint) => state.hosted.listen_at(endpoint),
                Err(e) => eprintln!("axio-app: hooks are off, the listener could not start: {e}"),
            }
            if !state.resumes_on_launch() {
                return;
            }
            for view in state.hosted.list() {
                // Running already, or stopped by a person: neither is the
                // window's to restart. A stop is a decision, and a launch
                // that undid it would be the window arguing.
                if view.status == "running" || view.stopped {
                    continue;
                }
                let relay = handle.clone();
                if let Err(e) = state
                    .resume_hosted(&view.id, None, None, move |id| {
                        let _ = relay.emit("axio://hosted-activity", id);
                    })
                    .await
                {
                    // Said, not swallowed: a row that stays ended after a
                    // launch should be explicable from the log.
                    eprintln!("axio-app: {} was not resumed: {e}", view.name);
                }
            }
        });
    }

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
    // A burst of questions is one notification, not one per question: a
    // second within two seconds of the first says nothing the first did not,
    // and a sound per event is how a person turns notifications off.
    if (!focused || matches!(kind, EventKind::ApprovalRequested { .. })) && not_in_a_burst() {
        let _ = handle
            .notification()
            .builder()
            .title(title)
            .body(body)
            .show();
    }
}

/// A hosted agent's word that it needs a person, or that it finished, said
/// the way a session's is: the badge counts every agent blocked on a
/// permission, and a notification goes out when the window is not in front.
fn hosted_attention(handle: &tauri::AppHandle, view: &crate::hosted::HostedView) {
    let Some(window) = handle.get_webview_window("main") else {
        return;
    };
    let state = handle.state::<AppState>();
    let pending = state.approvals().len()
        + state
            .hosted
            .list()
            .iter()
            .filter(|v| v.agent_status.as_deref() == Some("blocked"))
            .count();
    let _ = window.set_badge_count((pending > 0).then_some(pending as i64));
    let focused = window.is_focused().unwrap_or(true);
    let (title, body) = match view.agent_status.as_deref() {
        Some("blocked") => (
            format!("{} needs you", view.name),
            "waiting on a permission".to_owned(),
        ),
        Some("done") if !focused => (
            format!("{} finished", view.name),
            view.branch.clone().unwrap_or_default(),
        ),
        _ => return,
    };
    if not_in_a_burst() {
        let _ = handle
            .notification()
            .builder()
            .title(title)
            .body(body)
            .show();
    }
}

fn not_in_a_burst() -> bool {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    let mut last = LAST.lock().expect("no lock is held across an await");
    let now = Instant::now();
    if last.is_some_and(|t| now.duration_since(t) < Duration::from_secs(2)) {
        return false;
    }
    *last = Some(now);
    true
}
