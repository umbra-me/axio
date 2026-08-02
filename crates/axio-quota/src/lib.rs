//! Provider quota: how much of each AI coding provider's limit is left, and when it resets.
//!
//! The library half links no UI. Everything touching the outside world — the filesystem,
//! the environment, HTTP — arrives through [`provider::FetchContext`], so a test can point
//! `USERPROFILE` at a fixture directory and run the real credential-loading path. `axio
//! quota` and the Windows tray are two surfaces over the same probes.
//!
//! Provider protocol knowledge here was derived by reading CodexBar (MIT); see `NOTICE`.

/// The desktop app — tray icon, HTML flyout and window, in one Tauri process.
/// Feature-gated so a default `cargo install axio` compiles none of it.
#[cfg(all(windows, feature = "app"))]
pub mod app;
pub mod config;
pub mod error;
pub mod focus;
pub mod history;
pub mod json;
pub mod model;
pub mod paths;
pub mod provider;
pub mod providers;

pub use error::ProbeError;
pub use model::{Credits, ProviderId, RateWindow, UsageSnapshot};
pub use provider::{FetchContext, Provider};

use crate::config::Config;
use crate::paths::current_env;
use crate::provider::default_http_client;

/// Drops providers that were never set up.
///
/// A provider with no API key is not a failure to report: the user asked what their limits
/// are, not to be reminded of a service they do not use. Asking for one by name is a
/// different question and keeps its error. This lives here rather than in either surface
/// because the CLI and the tray must not disagree about what counts as a failure — they
/// did, briefly, and the tray was the one shouting.
pub fn drop_unconfigured(results: &mut Results) {
    results.retain(|(_, outcome)| !matches!(outcome, Err(ProbeError::NotConfigured(_))));
}

/// One refresh's worth of answers, successes and failures together.
pub type Results = Vec<(ProviderId, Result<UsageSnapshot, ProbeError>)>;

/// Probes every provider enabled in `config`, in order.
///
/// Sequential rather than concurrent on purpose: three requests on a refresh timer are not
/// worth the coordination, and a provider that rate-limits us is better met one at a time.
pub async fn fetch_enabled(
    config: &Config,
) -> Vec<(ProviderId, Result<UsageSnapshot, ProbeError>)> {
    let env = current_env();
    let http = match default_http_client() {
        Ok(http) => http,
        Err(err) => {
            let detail = err.to_string();
            return config
                .enabled_providers()
                .into_iter()
                .map(|id| {
                    (
                        id,
                        Err(ProbeError::Network {
                            provider: "http",
                            detail: detail.clone(),
                        }),
                    )
                })
                .collect();
        }
    };

    let mut results = Vec::new();
    for id in config.enabled_providers() {
        let ctx = FetchContext {
            http: http.clone(),
            env: env.clone(),
            config: config.provider_or_default(id),
        };
        results.push((id, providers::by_id(id).fetch(&ctx).await));
    }
    results
}
