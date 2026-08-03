//! `axio quota` — how much of each provider's limit is left.
//!
//! This surface reads *other tools'* credential files: the ones `codex` and `claude` wrote
//! at login. It never touches axio's own credential store, which is a different `auth.json`
//! in a different place — see `axio_core::auth`. Confusing the two is the failure this
//! comment exists to prevent.

use axio_quota::config::Config;
use axio_quota::model::{ProviderId, UsageSnapshot};
use axio_quota::paths::current_env;
use axio_quota::provider::{FetchContext, default_http_client};
use axio_quota::{ProbeError, providers};
use time::format_description::well_known::Rfc3339;

pub(crate) async fn quota_command(provider: Option<&str>, json: bool, diagnose: bool) -> u8 {
    let env = current_env();
    let config_path = Config::default_path(&env);
    let config = match Config::load(&config_path) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("axio: {err}");
            return 1;
        }
    };

    if diagnose {
        println!("config: {}", config_path.display());
        for probe in providers::all() {
            let hint = probe.credential_hint(&env);
            let state = if hint.starts_with("config ") {
                "n/a"
            } else if std::path::Path::new(&hint).exists() {
                "found"
            } else {
                "missing"
            };
            let enabled = config
                .provider(probe.id())
                .map(|entry| entry.is_enabled())
                .unwrap_or(false);
            println!(
                "{:<12} enabled={:<5} credential={:<8} {hint}",
                probe.id().display_name(),
                enabled,
                state
            );

            // For a cookie provider, "is a header set" is not the useful question. A
            // header can be pasted perfectly and still carry no session, because the panel
            // lists analytics and consent cookies in the same string — and a request to a
            // static asset carries those and nothing else. Naming the cookie that is
            // missing is the difference between a diagnostic and a shrug.
            let names = providers::session_cookie_names(probe.id());
            if names.is_empty() {
                continue;
            }
            let raw = config
                .provider(probe.id())
                .and_then(|entry| entry.cookie_header.as_deref())
                .filter(|text| !text.trim().is_empty());
            let Some(raw) = raw else {
                println!("             cookie: none pasted yet");
                continue;
            };

            // Whether the paste even looked like a header is the interesting half. A
            // string with no `=` in it anywhere is a lone cookie value, which is what the
            // Application panel's copy gives you — and saying that is far more use than
            // reporting that it did not work.
            let bare = !raw.contains('=');
            match providers::cookie_header_for(probe.id(), raw) {
                None => println!("             cookie: {} chars, unusable", raw.trim().len()),
                Some(header) => {
                    let found = providers::cookies_present(&header, names);
                    let count = header.split(';').filter(|pair| pair.contains('=')).count();
                    if bare {
                        println!(
                            "             cookie: a bare value, no name — read as {}",
                            found
                                .first()
                                .map(String::as_str)
                                .unwrap_or("nothing usable")
                        );
                    } else if found.is_empty() {
                        println!(
                            "             cookie: {count} pasted, none of them a session. \
                             Needs one of: {}",
                            names.join(", ")
                        );
                    } else {
                        println!(
                            "             cookie: {count} pasted, carries {}",
                            found.join(", ")
                        );
                    }
                }
            }
        }
        return 0;
    }

    let targets = match provider {
        Some(raw) => match ProviderId::parse(raw) {
            Some(id) => vec![id],
            None => {
                eprintln!("axio: unknown provider '{raw}'");
                return 1;
            }
        },
        None => config.enabled_providers(),
    };
    if targets.is_empty() {
        eprintln!(
            "axio: no providers enabled in {}. Enable one, or pass --provider.",
            config_path.display()
        );
        return 1;
    }

    let http = match default_http_client() {
        Ok(http) => http,
        Err(err) => {
            eprintln!("axio: {err}");
            return 1;
        }
    };

    let mut results = Vec::new();
    for id in targets {
        let ctx = FetchContext {
            http: http.clone(),
            env: env.clone(),
            config: config.provider_or_default(id),
        };
        results.push((id, providers::by_id(id).fetch(&ctx).await));
    }

    // Naming a provider with `--provider` asks a different question, and there the
    // missing key *is* the answer — so the rule only applies to the general case.
    if provider.is_none() {
        axio_quota::drop_unconfigured(&mut results);
        if results.is_empty() {
            eprintln!(
                "axio: no provider is configured. `axio quota --diagnose` shows where each one looks."
            );
            return 1;
        }
    }

    if json {
        print_json(&results);
    } else {
        print_table(&results);
    }

    // A provider that could not report is a failure, so `&&` sees it — the same reasoning
    // as a turn that completed with something refused.
    u8::from(results.iter().any(|(_, outcome)| outcome.is_err()))
}

fn print_json(results: &[(ProviderId, Result<UsageSnapshot, ProbeError>)]) {
    for (id, outcome) in results {
        let line = match outcome {
            Ok(snapshot) => serde_json::json!({
                "provider": id.as_str(),
                "ok": true,
                "snapshot": snapshot,
            }),
            Err(err) => serde_json::json!({
                "provider": id.as_str(),
                "ok": false,
                "error": err.to_string(),
                "needsUserAction": err.needs_user_action(),
            }),
        };
        println!("{line}");
    }
}

fn print_table(results: &[(ProviderId, Result<UsageSnapshot, ProbeError>)]) {
    for (id, outcome) in results {
        match outcome {
            Ok(snapshot) => {
                let plan = snapshot
                    .plan
                    .as_deref()
                    .map(|plan| format!(" ({plan})"))
                    .unwrap_or_default();
                println!("{}{plan}", id.display_name());
                for window in &snapshot.windows {
                    let reset = window
                        .resets_at
                        .and_then(|at| at.format(&Rfc3339).ok())
                        .map(|at| format!("  resets {at}"))
                        .unwrap_or_default();
                    println!(
                        "  {:<28} {:>5.1}% used{reset}",
                        window.label, window.used_percent
                    );
                }
                if let Some(credits) = &snapshot.credits
                    && let Some(balance) = credits.balance
                {
                    println!("  {:<28} {balance:.2} remaining", "Credits");
                }
            }
            Err(err) => println!("{}: {err}", id.display_name()),
        }
    }
}
