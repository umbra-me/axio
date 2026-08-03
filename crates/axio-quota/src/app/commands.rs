//! The Rust surface the webview can call.
//!
//! Every command returns plain data. `ProbeError` is rendered to a string at this boundary
//! rather than crossing it: the frontend needs a message and one flag — is this something
//! the user must fix — not the error's shape.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

use super::state::{AppState, refresh};
use crate::config::{Config, ProviderConfig};
use crate::history::Reading;
use crate::model::{ProviderId, UsageSnapshot};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderView {
    pub id: String,
    pub name: String,
    pub snapshot: Option<UsageSnapshot>,
    pub error: Option<String>,
    pub needs_user_action: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Overview {
    pub providers: Vec<ProviderView>,
    pub refreshing: bool,
}

#[tauri::command]
pub fn overview(state: State<'_, Arc<AppState>>) -> Overview {
    let results = state.results.lock().unwrap_or_else(|err| err.into_inner());
    Overview {
        providers: results
            .iter()
            .map(|(id, outcome)| ProviderView {
                id: id.as_str().to_string(),
                name: id.display_name().to_string(),
                snapshot: outcome.as_ref().ok().cloned(),
                error: outcome.as_ref().err().map(|err| err.to_string()),
                needs_user_action: outcome
                    .as_ref()
                    .err()
                    .map(|err| err.needs_user_action())
                    .unwrap_or(false),
            })
            .collect(),
        refreshing: state.is_refreshing(),
    }
}

#[tauri::command]
pub fn refresh_now(app: AppHandle) {
    refresh(&app);
}

#[tauri::command]
pub fn history(state: State<'_, Arc<AppState>>) -> Vec<Reading> {
    crate::history::load(&state.env)
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSetting {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    /// `None` for providers that read a credential their own CLI wrote — the frontend
    /// shows a hint instead of a key field, because an input that does nothing is a worse
    /// lie than no input at all.
    pub api_key: Option<String>,
    pub takes_api_key: bool,
    pub hint: String,
}

#[tauri::command]
pub fn settings(state: State<'_, Arc<AppState>>) -> Vec<ProviderSetting> {
    let config = Config::load(&Config::default_path(&state.env)).unwrap_or_default();
    ProviderId::ALL
        .iter()
        .map(|id| {
            let entry = config.provider_or_default(*id);
            ProviderSetting {
                id: id.as_str().to_string(),
                name: id.display_name().to_string(),
                enabled: entry.is_enabled(),
                api_key: entry.api_key.clone(),
                takes_api_key: takes_api_key(*id),
                hint: hint(*id).to_string(),
            }
        })
        .collect()
}

/// Writes the config back, preserving anything this build does not model.
///
/// The entry is mutated in place rather than replaced: a config shared with the macOS
/// CodexBar carries per-provider fields we do not know about, and rebuilding the entry
/// from our own struct would delete every one of them.
#[tauri::command]
pub fn save_settings(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    settings: Vec<ProviderSetting>,
) -> Result<String, String> {
    let path = Config::default_path(&state.env);
    let mut config = Config::load(&path).unwrap_or_default();

    for setting in settings {
        let Some(id) = ProviderId::parse(&setting.id) else {
            continue;
        };
        let index = match config
            .providers
            .iter()
            .position(|entry| entry.provider_id() == Some(id))
        {
            Some(index) => index,
            None => {
                config.providers.push(ProviderConfig::new(id));
                config.providers.len() - 1
            }
        };
        let entry = &mut config.providers[index];
        entry.enabled = Some(setting.enabled);
        if takes_api_key(id) {
            entry.api_key = setting
                .api_key
                .map(|key| key.trim().to_string())
                .filter(|key| !key.is_empty());
        }
    }

    config.save(&path).map_err(|err| err.to_string())?;
    // Which providers are probed at all just changed, so the view would otherwise keep
    // showing one that has been switched off.
    refresh(&app);
    Ok(path.display().to_string())
}

/// The Cost view's table, grouped as the window asks.
///
/// The scan behind it is cached in [`super::cost::CostCache`]: grouping is cheap and
/// happens per call, scanning is not and happens once. That module explains why a few
/// minutes of staleness beats a window that blocks for thirty seconds.
#[tauri::command]
pub fn cost_report(
    group: String,
    cost: State<'_, Arc<super::cost::CostCache>>,
) -> super::cost::CostReport {
    cost.report(&group)
}

/// The usage calendar and the habit it implies, off the same cached scan.
#[tauri::command]
pub fn cost_stats(cost: State<'_, Arc<super::cost::CostCache>>) -> super::cost::StatsView {
    cost.stats()
}

/// Drop the cached scan so the next `cost_report` reads the transcripts again.
#[tauri::command]
pub fn refresh_cost(cost: State<'_, Arc<super::cost::CostCache>>) {
    cost.invalidate();
}

#[tauri::command]
pub fn open_main_window(app: AppHandle) {
    if let Some(window) = app.get_webview_window(super::MAIN_WINDOW) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
    super::tray::hide_flyout(&app);
}

/// Minimise the window the titlebar belongs to.
#[tauri::command]
pub fn minimize_window(app: AppHandle) {
    if let Some(window) = app.get_webview_window(super::MAIN_WINDOW) {
        let _ = window.minimize();
    }
}

/// Close the window, which for a tray app means hide.
///
/// The process outlives it deliberately — the tray icon is the app, and quitting is a
/// menu item rather than a side effect of dismissing a panel.
#[tauri::command]
pub fn close_window(app: AppHandle) {
    if let Some(window) = app.get_webview_window(super::MAIN_WINDOW) {
        let _ = window.hide();
    }
}

#[tauri::command]
pub fn hide_flyout(app: AppHandle) {
    super::tray::hide_flyout(&app);
}

#[tauri::command]
pub fn quit(app: AppHandle) {
    app.exit(0);
}

fn takes_api_key(id: ProviderId) -> bool {
    matches!(id, ProviderId::Openrouter)
}

fn hint(id: ProviderId) -> &'static str {
    match id {
        ProviderId::Codex => "Signed in with the `codex` CLI — no key needed here.",
        ProviderId::Claude => "Signed in with the `claude` CLI — no key needed here.",
        ProviderId::Openrouter => "From openrouter.ai/settings/keys.",
    }
}
