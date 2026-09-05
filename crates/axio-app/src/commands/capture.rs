//! Explicitly pick an exported capture and attach it to a new hosted session.
use super::Shared;
use crate::{
    hosted::{HostedView, StartHostedInput},
    model::AppError,
};
use tauri::{Emitter, Manager};
use tauri_plugin_dialog::DialogExt;

#[tauri::command]
pub async fn hosted_start_capture(
    app: tauri::AppHandle,
    state: Shared<'_>,
    cwd: String,
) -> Result<Option<HostedView>, AppError> {
    let picked = app
        .dialog()
        .file()
        .add_filter("Axio Capture attachment", &["json"])
        .blocking_pick_file();
    let Some(file) = picked else { return Ok(None) };
    let file = file
        .into_path()
        .map_err(|e| AppError::Unavailable(e.to_string()))?;
    let destination = app
        .path()
        .app_data_dir()
        .map_err(|e| AppError::Unavailable(e.to_string()))?
        .join("attachments");
    let image = crate::attachments::import(&file, &destination)?;
    let input = StartHostedInput {
        harness: "codex".into(),
        cwd,
        isolation: None,
        group: None,
        args: crate::attachments::image_args(&image),
        prompt: None,
        transport: None,
        model: None,
        effort: None,
        permission: None,
        rows: None,
        cols: None,
    };
    let view = state
        .start_hosted(input, move |id| {
            let _ = app.emit("axio://hosted-activity", id);
        })
        .await?;
    Ok(Some(view))
}
