//! The command surface.
//!
//! **Every one of these is `async`, and that is not stylistic.** A Tauri
//! command declared without `async` — and without `#[tauri::command(async)]` —
//! runs on the main thread, which is the thread that paints and handles input.
//! The prior art declares all nine of its commands synchronous, and three of
//! them do real blocking work there: a startup probe that runs three process
//! launches in series, a teardown that spins for up to three seconds *per
//! session*, and a per-keystroke PTY write behind two mutexes. The symptom is a
//! window that stops repainting, and nothing in the code says why.
//!
//! `tauri::State<'_, T>` works in an async command as long as `T: Send + Sync +
//! 'static`, so there is no cost to the rule.
//!
//! Window controls are commands rather than capabilities. Granting
//! `core:window:allow-close` would let any script in the webview close the
//! window; routing it through here keeps a place that can refuse — which
//! matters as soon as a session is running work somebody would lose.

use tauri::{Manager, State};

use crate::hosted::{HostedOutput, HostedView, StartHostedInput};
use crate::model::{
    AppError, ApprovalView, DecisionInput, SessionView, Snapshot, StartSessionInput,
};
use crate::state::AppState;

type Shared<'a> = State<'a, AppState>;

/// Everything the interface needs to paint itself.
#[tauri::command]
pub async fn snapshot(state: Shared<'_>) -> Result<Snapshot, AppError> {
    Ok(state.snapshot())
}

/// Only the questions, for a poll that does not repaint the world.
#[tauri::command]
pub async fn approvals(state: Shared<'_>) -> Result<Vec<ApprovalView>, AppError> {
    Ok(state.approvals())
}

#[tauri::command]
pub async fn start_session(
    state: Shared<'_>,
    input: StartSessionInput,
) -> Result<SessionView, AppError> {
    state.start_session(input).await
}

#[tauri::command]
pub async fn send_prompt(
    state: Shared<'_>,
    session_id: String,
    prompt: String,
) -> Result<(), AppError> {
    state.send(&session_id, prompt).await
}

#[tauri::command]
pub async fn cancel_session(state: Shared<'_>, session_id: String) -> Result<(), AppError> {
    state.cancel(&session_id)
}

#[tauri::command]
pub async fn close_session(
    state: Shared<'_>,
    session_id: String,
    discard: bool,
) -> Result<(), AppError> {
    state.close(&session_id, discard).await
}

#[tauri::command]
pub async fn session_diff(state: Shared<'_>, session_id: String) -> Result<String, AppError> {
    state.diff(&session_id).await
}

#[tauri::command]
pub async fn resolve_approval(
    state: Shared<'_>,
    approval_id: String,
    decision: DecisionInput,
) -> Result<bool, AppError> {
    state.resolve_approval(&approval_id, decision)
}

// --- hosted agents -------------------------------------------------------
//
// Claude Code, Codex and Pi, each in a terminal this process owns. Reads are
// pulled by cursor rather than pushed, so a webview that reloaded asks for
// everything after the position it had and gets exactly the gap.

#[tauri::command]
pub async fn hosted_available() -> Result<Vec<HostedView>, AppError> {
    Ok(crate::hosted::available())
}

#[tauri::command]
pub async fn hosted_list(state: Shared<'_>) -> Result<Vec<HostedView>, AppError> {
    Ok(state.hosted.list())
}

#[tauri::command]
pub async fn hosted_start(
    state: Shared<'_>,
    input: StartHostedInput,
) -> Result<HostedView, AppError> {
    state.hosted.start(input)
}

#[tauri::command]
pub async fn hosted_read(
    state: Shared<'_>,
    id: String,
    from: u64,
) -> Result<HostedOutput, AppError> {
    state.hosted.read(&id, from)
}

#[tauri::command]
pub async fn hosted_write(
    state: Shared<'_>,
    id: String,
    data: String,
    submit: bool,
) -> Result<(), AppError> {
    state.hosted.write(&id, &data, submit)
}

#[tauri::command]
pub async fn hosted_resize(
    state: Shared<'_>,
    id: String,
    rows: u16,
    cols: u16,
) -> Result<(), AppError> {
    state.hosted.resize(&id, rows, cols)
}

#[tauri::command]
pub async fn hosted_kill(state: Shared<'_>, id: String) -> Result<(), AppError> {
    state.hosted.kill(&id).await
}

/// Minimise, maximise and close, through one door.
///
/// `destroy` is deliberately separate from `close`: closing runs whatever guard
/// is in front of it, destroying does not. Only something that has already
/// dealt with the running work should reach for the second.
#[tauri::command]
pub async fn window_control(window: tauri::WebviewWindow, action: String) -> Result<(), AppError> {
    let failed = |e: tauri::Error| AppError::Supervisor(e.to_string());
    match action.as_str() {
        "minimize" => window.minimize().map_err(failed),
        "toggle-maximize" => {
            if window.is_maximized().map_err(failed)? {
                window.unmaximize().map_err(failed)
            } else {
                window.maximize().map_err(failed)
            }
        }
        "close" => window.close().map_err(failed),
        "destroy" => window.destroy().map_err(failed),
        other => Err(AppError::Supervisor(format!(
            "no such window action: {other}"
        ))),
    }
}

/// Whether closing now would abandon work.
///
/// Asked by the native close guard rather than by the webview, because a
/// `beforeunload` listener does not fire for a taskbar close or Alt+F4 — the
/// two ways somebody actually closes a window they have stopped looking at.
pub(crate) fn has_running_work(app: &tauri::AppHandle) -> bool {
    app.try_state::<AppState>().is_some_and(|state| {
        // A hosted terminal counts. Closing over a live Claude Code loses
        // whatever it was in the middle of just as surely as closing over one
        // of ours does, and it is the one the window cannot restart for you.
        state.hosted.running() > 0
            || state
                .snapshot()
                .sessions
                .iter()
                .any(|s| s.status == crate::model::SessionStatus::Running)
    })
}
