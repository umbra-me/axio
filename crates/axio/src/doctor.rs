//! What axio can see, so a misconfiguration is one command away from obvious.

use super::*;

/// What axio can see, so a misconfiguration is one command away from obvious.
pub(crate) fn doctor(resolved: &Resolved) -> u8 {
    let mut out = std::io::stdout();
    let cfg = resolved.config();
    print_notices(resolved);
    let _ = writeln!(out, "axio {VERSION}");
    let _ = writeln!(out);

    let _ = writeln!(out, "credentials");
    // One resolution path, the same one the run itself uses, so doctor cannot
    // disagree with reality about which credential is in play.
    let env: Vec<(String, String)> = std::env::vars().collect();
    for (provider, source) in
        axio_core::auth::status(axio_core::auth::PROVIDERS, &axio_home(), &env)
    {
        match source {
            Some(source) => {
                let _ = writeln!(out, "  {provider:<18}  {}", source.describe());
            }
            None => {
                let _ = writeln!(out, "  {provider:<18}  not configured");
            }
        }
    }
    let _ = writeln!(
        out,
        "  -> axio auth login --provider {}",
        cfg.model.provider
    );
    let _ = writeln!(out);

    let _ = writeln!(out, "model");
    let _ = writeln!(out, "  provider            {}", cfg.model.provider);
    let _ = writeln!(out, "  model               {}", cfg.model.name);
    let _ = writeln!(out, "  effort              {}", cfg.model.effort.as_wire());
    let _ = writeln!(out, "  max_tokens          {}", cfg.model.max_tokens);
    let _ = writeln!(out, "  max_steps           {}", cfg.budget.max_steps);
    // The endpoint a stale shell export points at is exactly the misconfiguration
    // this command exists to make obvious, and it was the one field not shown.
    let _ = writeln!(
        out,
        "  base_url            {}",
        cfg.model
            .base_url
            .as_deref()
            .unwrap_or("(provider default)")
    );
    match cfg.budget.max_usd_per_turn {
        Some(limit) => {
            let _ = writeln!(out, "  max_usd_per_turn    {limit:.2}");
        }
        None => {
            let _ = writeln!(out, "  max_usd_per_turn    (none)");
        }
    }
    let _ = writeln!(out);

    // From the provider that will actually be used, never a literal. A table
    // printed "because a stale price table is invisible until the bill" is worse
    // than nothing when it is a different provider's table — which is what a
    // hardcoded one becomes the moment a second provider exists.
    let prices = provider_prices(cfg);
    match prices {
        Some(info) if info.input_price > 0.0 || info.output_price > 0.0 => {
            let _ = writeln!(out, "prices (USD per million tokens)");
            let _ = writeln!(out, "  input               {:.2}", info.input_price);
            let _ = writeln!(out, "  output              {:.2}", info.output_price);
            let _ = writeln!(out, "  cache read          {:.2}", info.cache_read_price);
            let _ = writeln!(out, "  cache write         {:.2}", info.cache_write_price);
        }
        Some(_) => {
            let _ = writeln!(out, "prices");
            let _ = writeln!(
                out,
                "  this provider reports no prices, so recorded cost is always 0.00"
            );
            if cfg.budget.max_usd_per_turn.is_some() {
                let _ = writeln!(
                    out,
                    "  max_usd_per_turn is set but cannot trip — nothing measures spend here"
                );
            }
        }
        None => {
            let _ = writeln!(out, "prices");
            let _ = writeln!(out, "  unknown: the provider could not be constructed");
        }
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "permissions");
    if cfg.permissions.allow.is_empty() && cfg.permissions.deny.is_empty() {
        let _ = writeln!(out, "  (no rules; the built-in deny list still applies)");
    }
    for rule in &cfg.permissions.deny {
        let _ = writeln!(out, "  deny                {rule}");
    }
    for rule in &cfg.permissions.allow {
        let _ = writeln!(out, "  allow               {rule}");
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "paths");
    let _ = writeln!(
        out,
        "  user config         {}",
        config_file_path().display()
    );
    let _ = writeln!(out, "  axio home           {}", axio_home().display());
    let _ = writeln!(out, "  state               {}", state_dir().display());
    let _ = writeln!(
        out,
        "  cwd                 {}",
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .display()
    );
    let _ = writeln!(out);

    let _ = writeln!(out, "surfaces");
    let _ = writeln!(
        out,
        "  stdin               {}",
        if std::io::stdin().is_terminal() {
            "terminal"
        } else {
            "piped"
        }
    );
    let _ = writeln!(
        out,
        "  stdout              {}",
        if std::io::stdout().is_terminal() {
            "terminal"
        } else {
            "piped"
        }
    );
    0
}

/// The prices the configured provider would actually charge against.
///
/// Read without constructing a provider, so `--doctor` still touches no
/// credential and opens no socket.
pub(crate) fn provider_prices(
    cfg: &axio_core::config::Config,
) -> Option<axio_core::provider::ModelInfo> {
    match cfg.model.provider.as_str() {
        "anthropic" => Some(axio_provider::anthropic::model_info(&cfg.model.name)),
        "ollama" | "openai-compatible" => Some(axio_provider::openai::model_info(&cfg.model.name)),
        _ => None,
    }
}
