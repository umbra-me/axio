//! Constructing the provider the configuration names.
//!
//! Three implementations selected by name, which is the registry the seam spent
//! two providers avoiding. The third earned it: a subscription token is only
//! accepted by an endpoint speaking a third dialect, so the alternative to a
//! third arm was not having it.
//!
//! Split from `surfaces` because that file reached the width limit.

use super::*;

/// Construct the provider the configuration names.
pub(crate) fn build_provider(
    resolved: &Resolved,
) -> Result<Arc<dyn axio_core::provider::Provider>, String> {
    let cfg = resolved.config();
    build_named(cfg, &cfg.model.provider)
}

/// A way to build any provider, for a surface that has no configuration.
///
/// The interface can move a session to another provider, and doing that needs
/// a credential looked up and a transport constructed — neither of which it
/// should know how to do. It gets this instead.
#[cfg(feature = "tui")]
pub(crate) type Factory =
    Arc<dyn Fn(&str) -> Result<Arc<dyn axio_core::provider::Provider>, String> + Send + Sync>;

#[cfg(feature = "tui")]
pub(crate) fn factory(resolved: &Resolved) -> Factory {
    let cfg = resolved.config().clone();
    Arc::new(move |name: &str| build_named(&cfg, name))
}

/// Construct a named provider against the resolved configuration.
///
/// Named separately from the configured one so a session can be moved to
/// another provider without restarting. `base_url` still comes from the
/// configuration, which is right for an override aimed at the provider being
/// built and wrong for one aimed at the provider being left — so it is ignored
/// unless the two agree, rather than pointing a new transport at an endpoint
/// meant for the old one.
pub(crate) fn build_named(
    cfg: &axio_core::config::Config,
    name: &str,
) -> Result<Arc<dyn axio_core::provider::Provider>, String> {
    let configured = &cfg.model;
    let base_url = if configured.provider == name {
        configured.base_url.clone()
    } else {
        None
    };
    let model = axio_core::config::ModelSection {
        provider: name.to_owned(),
        base_url,
        ..configured.clone()
    };
    let model = &model;
    // One lookup for every provider: the environment, then the store. A second
    // resolution path is how the two disagree about which key is in use.
    let (found, _source) = credential(&model.provider)?;

    match model.provider.as_str() {
        "anthropic" => AnthropicProvider::new(found.bearer().expose())
            .map(|p| match &model.base_url {
                // `model.base_url` was accepted, reported by `--explain`, and
                // ignored here — so a gateway or proxy endpoint silently went
                // to the public API instead.
                Some(url) => p.with_base_url(url.clone()),
                None => p,
            })
            .map(|p| Arc::new(p) as Arc<dyn axio_core::provider::Provider>)
            .map_err(|e| format!("could not start the http client: {e}")),
        "ollama" | "openai-compatible" => {
            let base = model
                .base_url
                .clone()
                .unwrap_or_else(|| OLLAMA_BASE.to_owned());
            OpenAiProvider::new(found.bearer().expose(), base, model.provider.clone())
                .map(|p| Arc::new(p) as Arc<dyn axio_core::provider::Provider>)
                .map_err(|e| format!("could not start the http client: {e}"))
        }
        "openai-codex" => {
            let axio_core::auth::Credential::OAuth(tokens) = found else {
                return Err("`openai-codex` is signed in to, not given a key.\n\n\
                     Start axio and run /login, which opens a browser.\n\n\
                     What is stored for it is an API key, which this endpoint \
                     does not accept."
                    .to_owned());
            };
            // The sink is what makes a renewal outlive the process. Without it
            // every run starts from the pair that already expired.
            axio_provider::CodexProvider::new(
                tokens,
                Arc::new(credentials::StoreTokens),
                model.base_url.clone(),
            )
            .map(|p| Arc::new(p) as Arc<dyn axio_core::provider::Provider>)
            .map_err(|e| format!("could not start the http client: {e}"))
        }
        other => Err(unknown_provider(other)),
    }
}
