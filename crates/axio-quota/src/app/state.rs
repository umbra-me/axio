//! What the app knows, and how it gets refreshed.

use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Emitter, Manager};

use crate::Results;
use crate::config::Config;
use crate::paths::{Env, current_env};

/// Emitted whenever results change. The frontend listens rather than polling, so a probe
/// that finishes while a window is closed still lands the moment one opens.
pub const EVENT_UPDATED: &str = "quota://updated";

pub struct AppState {
    pub env: Env,
    pub results: Mutex<Results>,
    refreshing: Mutex<bool>,
}

impl AppState {
    pub fn new() -> Self {
        AppState {
            env: current_env(),
            results: Mutex::new(Vec::new()),
            refreshing: Mutex::new(false),
        }
    }

    pub fn is_refreshing(&self) -> bool {
        *self
            .refreshing
            .lock()
            .unwrap_or_else(|err| err.into_inner())
    }
}

/// Probes on a worker thread, then emits [`EVENT_UPDATED`].
///
/// Never on the UI thread: a webview that stops painting because an HTTP request is slow
/// is the failure a desktop app most obviously must not have. Re-entry is guarded, so
/// mashing Refresh queues nothing.
pub fn refresh(app: &AppHandle) {
    let state = app.state::<Arc<AppState>>();
    {
        let mut refreshing = state
            .refreshing
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        if *refreshing {
            return;
        }
        *refreshing = true;
    }

    let handle = app.clone();
    let shared = Arc::clone(&state);
    std::thread::spawn(move || {
        if let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            let config = Config::load(&Config::default_path(&shared.env)).unwrap_or_default();
            let mut fetched = runtime.block_on(crate::fetch_enabled(&config));
            crate::drop_unconfigured(&mut fetched);
            crate::history::record(&shared.env, &fetched);
            *shared.results.lock().unwrap_or_else(|err| err.into_inner()) = fetched;
        }
        *shared
            .refreshing
            .lock()
            .unwrap_or_else(|err| err.into_inner()) = false;

        let _ = handle.emit(EVENT_UPDATED, ());
        super::tray::update_icon(&handle);
    });
}

/// How often the app re-probes in the background.
///
/// Five minutes is a compromise, and a poor one at both ends: far more often than a weekly
/// window needs, too slow to watch a session window drain under heavy use. Upstream solves
/// this with a schedule that tightens near a reset; this constant is the placeholder.
pub const REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(300);

pub fn spawn_refresh_loop(app: &AppHandle) {
    let handle = app.clone();
    std::thread::spawn(move || {
        loop {
            refresh(&handle);
            std::thread::sleep(REFRESH_INTERVAL);
        }
    });
}
