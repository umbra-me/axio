//! Signing in from the surface.
//!
//! The flow itself lives in the transport crate; this is the part that belongs
//! to the interface — running it off the loop, and turning what comes back into
//! lines that say what happened without saying what was granted.
//!
//! Split from `mod` because that file reached the width limit.

use super::*;

/// What a sign-in tells the surface while it happens.
pub(super) enum SignIn {
    /// The URL, and whether a browser took it. Sent before waiting, so the URL
    /// is on screen for the machine where nothing could be opened.
    Opened {
        url: String,
        opened: bool,
    },
    Done(Result<axio_core::auth::OAuthTokens, String>),
}

/// Run a browser sign-in, reporting through `tx`.
///
/// Off the loop entirely: it opens a listener, waits up to five minutes for a
/// person, and none of that may block the interface.
pub(super) async fn sign_in(
    provider: &'static str,
    tx: mpsc::UnboundedSender<SignIn>,
    cancel: CancellationToken,
) {
    let started = match axio_provider::oauth::codex::start() {
        Ok(started) => started,
        Err(e) => {
            let _ = tx.send(SignIn::Done(Err(e.to_string())));
            return;
        }
    };
    let http = match axio_provider::oauth::http_client() {
        Ok(http) => http,
        Err(e) => {
            let _ = tx.send(SignIn::Done(Err(e.to_string())));
            return;
        }
    };

    let url = started.url.clone();
    let opened = axio_tools::browser::open(&url);
    let _ = tx.send(SignIn::Opened { url, opened });

    let result = started
        .finish(&http, cancel)
        .await
        .map_err(|e| format!("{provider}: {e}"));
    let _ = tx.send(SignIn::Done(result));
}

/// Store what a sign-in produced, and say what happened without saying what it
/// produced.
pub(super) fn finish_signin(result: Result<axio_core::auth::OAuthTokens, String>) -> Vec<String> {
    use axio_core::auth::{self, Credential};

    let tokens = match result {
        Ok(tokens) => tokens,
        Err(why) => return vec![why],
    };
    let account = tokens.account_id.clone();

    match auth::save(
        &crate::paths::axio_home(),
        "openai-codex",
        Credential::OAuth(tokens),
    ) {
        Ok(path) => {
            let mut said = vec!["signed in to `openai-codex`".to_owned()];
            if let Some(account) = account {
                said.push(format!("  account {account}"));
            }
            said.push(format!("  {}", path.display()));
            said.push(format!("  {}", auth::protection_note()));
            said.push("  this session keeps the credential it started with".into());
            said
        }
        Err(e) => vec![format!("signed in, but could not store the tokens: {e}")],
    }
}

/// Report what a running sign-in just did.
///
/// One of the two arms the loop hands straight back: the sign-in is already
/// off the loop, so what arrives here is only ever something to say.
pub(super) fn on_signin_step<B: Backend>(
    app: &mut Tui,
    terminal: &mut Terminal<B>,
    step: Option<SignIn>,
) -> Result<(), B::Error> {
    match step {
        Some(SignIn::Opened { url, opened }) => {
            let mut said = if opened {
                vec!["a browser was opened to finish signing in".to_owned()]
            } else {
                vec!["no browser could be opened — visit this to sign in:".to_owned()]
            };
            said.push(format!("  {url}"));
            app.push_command_output(terminal, &said)?;
            app.status = "waiting for the browser".into();
        }
        Some(SignIn::Done(result)) => {
            app.status.clear();
            let said = finish_signin(result);
            app.push_command_output(terminal, &said)?;
        }
        None => {}
    }
    Ok(())
}

/// Ask a provider what it serves, off the loop.
///
/// The provider is built by name rather than taken from the agent, because the
/// point of the first stage is to reach one the session is not using yet.
pub(super) fn list_models(
    app: &mut Tui,
    agent: Option<&Agent>,
    name: &str,
    tx: mpsc::UnboundedSender<Result<Vec<String>, String>>,
) {
    app.pending_provider = Some(name.to_owned());
    app.status = format!("listing {name} models");

    // Reuse the running provider when it is already the one asked about: it
    // holds a live token, and building a second would refresh separately and
    // leave two of them disagreeing about which pair is current.
    let built = match agent {
        Some(agent) if agent.id_of_provider() == name => Ok(agent.provider()),
        _ => (app.factory)(name),
    };

    match built {
        Ok(provider) => {
            let token = CancellationToken::new();
            tokio::spawn(async move {
                let _ = tx.send(provider.models(token).await.map_err(|e| e.to_string()));
            });
        }
        Err(why) => {
            let _ = tx.send(Err(why));
        }
    }
}

/// Open the picker on what a provider listed.
pub(super) fn on_models_listed(
    app: &mut Tui,
    agent: Option<&Agent>,
    listed: Option<Result<Vec<String>, String>>,
) {
    app.status.clear();
    match listed {
        Some(Ok(models)) if models.is_empty() => {
            app.status = "the provider listed no models".into();
        }
        Some(Ok(models)) => {
            // Marked against the running model only when the provider did not
            // change; a tick beside a name the new endpoint merely happens to
            // share would be a lie.
            let staying = app
                .pending_provider
                .as_deref()
                .is_none_or(|p| agent.is_none_or(|a| a.id_of_provider() == p));
            let current = if staying {
                agent
                    .map(|a| a.model().to_owned())
                    .unwrap_or_else(|| app.model.clone())
            } else {
                String::new()
            };
            app.mode = Mode::PickingModel(Picker::new(models, current));
        }
        Some(Err(why)) => app.status = format!("could not list models: {why}"),
        None => {}
    }
}
