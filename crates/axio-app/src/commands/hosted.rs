//! Commands for hosted terminals, groups, landing and settings.

use crate::hosted::{HostedOutput, HostedView, StartHostedInput};
use crate::model::{
    AddToGroupInput, AppError, GroupStart, LandAction, LandOutcome, LandingView, ProviderView,
    StartGroupInput, TranscriptView,
};
use crate::settings::{AppSettings, ModelDefault, SettingsView};
use tauri::Emitter;

use super::Shared;

#[tauri::command]
pub async fn hosted_available() -> Result<Vec<HostedView>, AppError> {
    Ok(crate::hosted::available())
}

#[tauri::command]
pub async fn hosted_list(state: Shared<'_>) -> Result<Vec<HostedView>, AppError> {
    Ok(state.hosted.list())
}

/// Start a hosted agent, and tell the window when it writes.
///
/// The event carries the session id only. Bytes still come back through
/// `hosted_read` and a cursor, so a listener that missed a signal is late
/// rather than out of sync — which is what makes a webview reload survivable.
#[tauri::command]
pub async fn hosted_start(
    app: tauri::AppHandle,
    state: Shared<'_>,
    input: StartHostedInput,
) -> Result<HostedView, AppError> {
    state
        .start_hosted(input, move |id| {
            let _ = app.emit("axio://hosted-activity", id);
        })
        .await
}

/// Several agents on one prompt, each in a worktree of its own.
#[tauri::command]
pub async fn start_group(
    app: tauri::AppHandle,
    state: Shared<'_>,
    input: StartGroupInput,
) -> Result<GroupStart, AppError> {
    state
        .start_group(input, move |id| {
            let _ = app.emit("axio://hosted-activity", id);
        })
        .await
}

/// One more member for a group.
#[tauri::command]
pub async fn add_to_group(
    app: tauri::AppHandle,
    state: Shared<'_>,
    input: AddToGroupInput,
) -> Result<GroupStart, AppError> {
    state
        .add_to_group(input, move |id| {
            let _ = app.emit("axio://hosted-activity", id);
        })
        .await
}

#[tauri::command]
pub async fn rename_session(
    state: Shared<'_>,
    session_id: String,
    title: Option<String>,
) -> Result<(), AppError> {
    state.rename_session(&session_id, title)
}

#[tauri::command]
pub async fn rename_hosted(
    state: Shared<'_>,
    id: String,
    title: Option<String>,
) -> Result<HostedView, AppError> {
    state.hosted.rename(&id, title)
}

#[tauri::command]
pub async fn hosted_diff(state: Shared<'_>, id: String) -> Result<String, AppError> {
    state.hosted.diff(&id).await
}

/// Where a session's branch stands, and what can be done with it.
#[tauri::command]
pub async fn landing(state: Shared<'_>, session_id: String) -> Result<LandingView, AppError> {
    state.landing(&session_id).await
}

#[tauri::command]
pub async fn land(
    state: Shared<'_>,
    session_id: String,
    action: LandAction,
) -> Result<LandOutcome, AppError> {
    state.land(&session_id, action).await
}

/// Every provider, and whether this machine can use it.
#[tauri::command]
pub async fn providers(state: Shared<'_>) -> Result<Vec<ProviderView>, AppError> {
    Ok(state.providers(&axio::home()))
}

/// Show a path in the file manager, or open it in the configured editor.
#[tauri::command]
pub async fn reveal(state: Shared<'_>, path: String, editor: bool) -> Result<(), AppError> {
    let command = if editor {
        state
            .settings_view()
            .ok()
            .map(|v| v.settings.editor)
            .filter(|e| !e.trim().is_empty())
            .or_else(|| Some("code".to_owned()))
    } else {
        None
    };
    crate::state::reveal(&path, command.as_deref())
}

// --- settings --------------------------------------------------------------

#[tauri::command]
pub async fn settings(state: Shared<'_>) -> Result<SettingsView, AppError> {
    state.settings_view()
}

/// Save the window's settings, whole, and return what is now on disk.
#[tauri::command]
pub async fn save_settings(
    state: Shared<'_>,
    settings: AppSettings,
) -> Result<SettingsView, AppError> {
    state.save_settings(&settings)
}

/// Set the default model in axio's own configuration file.
#[tauri::command]
pub async fn set_default_model(
    state: Shared<'_>,
    model: ModelDefault,
) -> Result<SettingsView, AppError> {
    state.set_default_model(&model)
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

/// Type a line into a terminal and submit it, paced so the agent's own
/// interface treats it as typed rather than pasted.
#[tauri::command]
pub async fn hosted_submit(state: Shared<'_>, id: String, text: String) -> Result<(), AppError> {
    state.hosted.say(&id, text).await
}

/// A hosted agent's own transcript, from the file its hooks named.
#[tauri::command]
pub async fn hosted_transcript(state: Shared<'_>, id: String) -> Result<TranscriptView, AppError> {
    state.hosted.any_transcript(&id)
}

/// The slash commands a terminal's agent answers to.
#[tauri::command]
pub async fn hosted_commands(
    state: Shared<'_>,
    id: String,
) -> Result<Vec<crate::hosted::SlashCommand>, AppError> {
    state.hosted.commands(&id)
}

/// How the window was looking at things when it last saved.
#[tauri::command]
pub async fn window_state(state: Shared<'_>) -> Result<crate::window_state::WindowState, AppError> {
    Ok(state.window_state())
}

/// Remember how the window is looking at things.
#[tauri::command]
pub async fn save_window_state(
    state: Shared<'_>,
    window: crate::window_state::WindowState,
) -> Result<(), AppError> {
    state.save_window_state(&window)
}

/// Stop a terminal and forget it. Its worktree stays.
#[tauri::command]
pub async fn hosted_kill(state: Shared<'_>, id: String) -> Result<(), AppError> {
    state.hosted.kill(&id).await
}

/// Stop a terminal and keep its row, ended, for a resume.
#[tauri::command]
pub async fn hosted_stop(state: Shared<'_>, id: String) -> Result<(), AppError> {
    state.hosted.stop(&id).await
}

/// Start an ended or remembered terminal again in its own directory.
#[tauri::command]
pub async fn hosted_resume(
    app: tauri::AppHandle,
    state: Shared<'_>,
    id: String,
    rows: Option<u16>,
    cols: Option<u16>,
) -> Result<HostedView, AppError> {
    state
        .resume_hosted(&id, rows, cols, move |id| {
            let _ = app.emit("axio://hosted-activity", id);
        })
        .await
}

/// The questions a structured agent is waiting on.
#[tauri::command]
pub async fn hosted_approvals(
    state: Shared<'_>,
    id: String,
) -> Result<Vec<crate::hosted::appserver::HostedApproval>, AppError> {
    state.hosted.approvals(&id)
}

/// Answer one: `accept`, `acceptForSession`, `decline` or `cancel`.
#[tauri::command]
pub async fn hosted_decide(
    state: Shared<'_>,
    id: String,
    approval: String,
    decision: String,
) -> Result<(), AppError> {
    state.hosted.decide(&id, &approval, &decision).await
}
