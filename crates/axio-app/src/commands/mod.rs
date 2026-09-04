//! The window's commands, one per thing the interface can ask.
//!
//! Split by subject: sessions and the window here, hosted terminals,
//! groups, landing and settings in `hosted.rs`. Every command is `async`
//! so none of them runs on the thread that paints.

mod hosted;
pub use hosted::*;

use tauri::{Manager, State};

use crate::model::{
    AppError, ApprovalView, DecisionInput, ProjectView, SessionView, Snapshot, StartSessionInput,
    TranscriptView,
};
use crate::state::AppState;

pub(super) type Shared<'a> = State<'a, AppState>;

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

/// The turns of a session that have a checkpoint either side.
#[tauri::command]
pub async fn session_turns(state: Shared<'_>, session_id: String) -> Result<Vec<u32>, AppError> {
    state.turns(&session_id).await
}

/// What one turn changed, as a diff between its two checkpoints.
#[tauri::command]
pub async fn session_turn_diff(
    state: Shared<'_>,
    session_id: String,
    turn: u32,
) -> Result<String, AppError> {
    state.turn_diff(&session_id, turn).await
}

#[tauri::command]
pub async fn session_transcript(
    state: Shared<'_>,
    session_id: String,
) -> Result<TranscriptView, AppError> {
    state.transcript(&session_id)
}

/// Register a repository the interface already knows the path of.
#[tauri::command]
pub async fn open_project(state: Shared<'_>, path: String) -> Result<ProjectView, AppError> {
    state.open_project(&path).await
}

/// Ask for a repository with the native folder picker, then register it.
///
/// The dialog is opened from here rather than from the webview so that what
/// reaches the supervisor is a path this side chose to accept — the same
/// reason window controls are a command. `None` means the picker was
/// dismissed, which is not an error and must not be shown as one.
#[tauri::command]
pub async fn add_repository(
    app: tauri::AppHandle,
    state: Shared<'_>,
) -> Result<Option<ProjectView>, AppError> {
    use tauri_plugin_dialog::DialogExt;
    let dialog = app.dialog().file().set_title("Add a repository");
    // Blocking by design, on a worker: the picker is modal for as long as it is
    // open, and an async command already runs off the thread that paints.
    let picked = tauri::async_runtime::spawn_blocking(move || dialog.blocking_pick_folder())
        .await
        .map_err(|e| AppError::Supervisor(format!("the folder picker did not return: {e}")))?;
    let Some(picked) = picked else {
        return Ok(None);
    };
    let path = picked
        .into_path()
        .map_err(|e| AppError::NoRepository(format!("not a usable path: {e}")))?;
    state.open_project(&path.to_string_lossy()).await.map(Some)
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
