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
    /// When the last probe finished, so a burst of triggers becomes one probe.
    last_finished: Mutex<Option<std::time::Instant>>,
}

impl AppState {
    pub fn new() -> Self {
        AppState {
            env: current_env(),
            results: Mutex::new(Vec::new()),
            refreshing: Mutex::new(false),
            last_finished: Mutex::new(None),
        }
    }

    pub fn is_refreshing(&self) -> bool {
        *self
            .refreshing
            .lock()
            .unwrap_or_else(|err| err.into_inner())
    }
}

/// The shortest gap between two probes.
///
/// Saving settings triggers a refresh, so does capturing a sign-in, so does the schedule,
/// and so does the button. Under a run of edits those pile onto vendors that rate-limit
/// their own usage endpoints — Claude began answering 429, which the view reported as
/// "rate limited by provider" and which was our doing rather than theirs.
const MIN_GAP: std::time::Duration = std::time::Duration::from_secs(20);

/// Probes on a worker thread, then emits [`EVENT_UPDATED`].
///
/// Never on the UI thread: a webview that stops painting because an HTTP request is slow is
/// the failure a desktop app most obviously must not have. Re-entry is guarded, so mashing
/// Refresh queues nothing, and [`MIN_GAP`] keeps a burst of triggers to one probe.
pub fn refresh(app: &AppHandle) {
    refresh_inner(app, false)
}

/// Probe regardless of how recently the last one ran — for the Refresh button only.
///
/// Someone who presses it is asking a question that the throttle would otherwise decline
/// to answer, silently, which is the one case where declining is wrong.
pub fn refresh_now(app: &AppHandle) {
    refresh_inner(app, true)
}

fn refresh_inner(app: &AppHandle, forced: bool) {
    let state = app.state::<Arc<AppState>>();
    {
        let mut refreshing = state
            .refreshing
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        if *refreshing {
            return;
        }
        if !forced {
            let last = state
                .last_finished
                .lock()
                .unwrap_or_else(|err| err.into_inner());
            if last.is_some_and(|at| at.elapsed() < MIN_GAP) {
                return;
            }
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
            .last_finished
            .lock()
            .unwrap_or_else(|err| err.into_inner()) = Some(std::time::Instant::now());
        *shared
            .refreshing
            .lock()
            .unwrap_or_else(|err| err.into_inner()) = false;

        let _ = handle.emit(EVENT_UPDATED, ());
        super::tray::update_icon(&handle);
    });
}

/// Re-probe on the schedule the last probe implies.
///
/// The interval is recomputed after each pass rather than fixed, so the loop tightens as a
/// window nears its reset or its limit and relaxes when nothing is happening — see
/// [`super::schedule`] for why a constant is wrong at both ends.
///
/// Manual mode still runs the loop, parked on a poll: the setting can change while the app
/// is running, and a thread that exited on "manual" would need the app restarted to notice
/// it had been switched back.
pub fn spawn_refresh_loop(app: &AppHandle) {
    let handle = app.clone();
    std::thread::spawn(move || {
        loop {
            refresh(&handle);

            // After the probe, so the decision is made on what it just returned. The probe
            // is asynchronous, so this reads the previous pass's results — one interval
            // behind, which for a cadence rather than a deadline is close enough.
            let state = handle.state::<Arc<AppState>>();
            let cadence = cadence_of(&state.env);
            let results = state.results.lock().unwrap_or_else(|err| err.into_inner());
            let wait = super::schedule::interval(
                cadence,
                &results,
                time::OffsetDateTime::now_utc().unix_timestamp(),
            );
            drop(results);

            std::thread::sleep(wait.unwrap_or(MANUAL_POLL));
        }
    });
}

/// How often a manual-mode loop wakes to notice the setting has changed back.
const MANUAL_POLL: std::time::Duration = std::time::Duration::from_secs(60);

pub fn cadence_of(env: &Env) -> super::schedule::Cadence {
    let config = Config::load(&Config::default_path(env)).unwrap_or_default();
    super::schedule::Cadence::parse(config.refresh.as_deref())
}
