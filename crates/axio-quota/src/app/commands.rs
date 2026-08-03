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

/// `async` so it runs on a worker rather than the main thread: a sync Tauri command
/// occupies the thread that paints, and this one reads a file off disk.
#[tauri::command]
pub async fn history(state: State<'_, Arc<AppState>>) -> Result<Vec<Reading>, ()> {
    Ok(crate::history::load(&state.env))
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
    /// A second field, for providers that scope a key to a team or project in the URL
    /// rather than in the credential. Only xAI needs one today.
    pub workspace_id: Option<String>,
    pub takes_workspace_id: bool,
    pub workspace_hint: String,
    /// A pasted `Cookie:` header, for providers whose usage is only on the dashboard.
    pub cookie_header: Option<String>,
    pub takes_cookie: bool,
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
                workspace_id: entry.workspace_id.clone(),
                takes_workspace_id: takes_workspace_id(*id),
                workspace_hint: workspace_hint(*id).to_string(),
                cookie_header: entry.cookie_header.clone(),
                takes_cookie: takes_cookie(*id),
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
        if takes_cookie(id) {
            entry.cookie_header = setting
                .cookie_header
                .map(|header| header.trim().to_string())
                .filter(|header| !header.is_empty());
        }
        if takes_workspace_id(id) {
            entry.workspace_id = setting
                .workspace_id
                .map(|id| id.trim().to_string())
                .filter(|id| !id.is_empty());
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
pub async fn cost_report(
    group: String,
    cost: State<'_, Arc<super::cost::CostCache>>,
) -> Result<super::cost::CostReport, ()> {
    Ok(cost.report(&group))
}

/// The usage calendar and the habit it implies, off the same cached scan.
#[tauri::command]
pub async fn cost_stats(
    cost: State<'_, Arc<super::cost::CostCache>>,
) -> Result<super::cost::StatsView, ()> {
    Ok(cost.stats())
}

/// Read and write the refresh cadence.
///
/// Its own pair of commands rather than a field on the settings list: the list is per
/// provider and this is one setting for the app, and folding it in would mean every
/// provider row carrying a copy of it.
#[tauri::command]
pub async fn refresh_cadence(state: State<'_, Arc<AppState>>) -> Result<String, ()> {
    Ok(super::state::cadence_of(&state.env).as_str())
}

#[tauri::command]
pub async fn set_refresh_cadence(
    state: State<'_, Arc<AppState>>,
    cadence: String,
) -> Result<String, String> {
    let path = Config::default_path(&state.env);
    let mut config = Config::load(&path).unwrap_or_default();
    // Round-tripped through the parser so an unusable value never reaches the file: the
    // loop reads this on every pass and a bad one would be read forever.
    config.refresh = Some(super::schedule::Cadence::parse(Some(&cadence)).as_str());
    config.save(&path).map_err(|err| err.to_string())?;
    Ok(config.refresh.unwrap_or_default())
}

/// Where axio keeps its files, and how big the largest of them is.
///
/// Shown rather than hidden because both answers are things people ask: settings live in
/// a roaming directory they may want to back up, and the saved scan is tens of megabytes
/// they should be able to find and delete.
#[tauri::command]
pub async fn storage(
    state: State<'_, Arc<AppState>>,
    cost: State<'_, Arc<super::cost::CostCache>>,
) -> Result<Storage, ()> {
    Ok(Storage {
        config_path: Config::default_path(&state.env).display().to_string(),
        history_path: crate::history::history_path(&state.env).display().to_string(),
        scan: cost.stored(),
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Storage {
    pub config_path: String,
    pub history_path: String,
    pub scan: super::cost::StoredScan,
}

/// Read the transcripts again, on a worker.
///
/// Returns as soon as the worker is started, not when it finishes — the window stays live
/// throughout and reloads when `cost://updated` arrives. The previous figures stay on
/// screen in the meantime, flagged as `scanning`.
#[tauri::command]
pub fn refresh_cost(app: AppHandle, cost: State<'_, Arc<super::cost::CostCache>>) {
    cost.invalidate();
    let cost = Arc::clone(&cost);
    let notify = app.clone();
    cost.rescan(move || {
        use tauri::Emitter;
        let _ = notify.emit(super::cost::EVENT_COST_UPDATED, ());
    });
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
    matches!(
        id,
        ProviderId::Openrouter | ProviderId::Zai | ProviderId::Deepseek | ProviderId::Xai
    )
}

/// Providers whose usage lives only behind a dashboard session.
///
/// Neither Cursor nor Ollama exposes these figures on an API key, so a pasted header is
/// the whole credential rather than an escape hatch beside one.
fn takes_cookie(id: ProviderId) -> bool {
    matches!(
        id,
        ProviderId::Cursor | ProviderId::Ollama | ProviderId::Opencode
    )
}

fn takes_workspace_id(id: ProviderId) -> bool {
    matches!(id, ProviderId::Xai | ProviderId::Opencode)
}

fn workspace_hint(id: ProviderId) -> &'static str {
    match id {
        ProviderId::Xai => "Team ID — it is in the xAI console URL.",
        ProviderId::Opencode => "Workspace ID or URL — optional, skips the lookup.",
        _ => "",
    }
}

fn hint(id: ProviderId) -> &'static str {
    match id {
        ProviderId::Codex => "Signed in with the `codex` CLI — no key needed here.",
        ProviderId::Claude => "Signed in with the `claude` CLI — no key needed here.",
        ProviderId::Openrouter => "From openrouter.ai/settings/keys.",
        ProviderId::Zai => "From z.ai — the same API token the coding plan uses.",
        ProviderId::Deepseek => "From platform.deepseek.com. Shows the prepaid balance.",
        // Worth stating: the console offers two kinds of key and only one works here.
        ProviderId::Xai => "A Management API key, not an inference key — xAI console,                             Settings > Management Keys.",
        ProviderId::Grok => "Signed in with the `grok` CLI — no key needed here.",
        ProviderId::Cursor => "Cursor has no usage API. Paste a Cookie header from a                                cursor.com request.",
        ProviderId::Ollama => "The Cloud Usage bars are not on the API. Paste a Cookie                                header from ollama.com/settings.",
        ProviderId::Opencode => "Paste a Cookie header from an opencode.ai request.",
    }
}
