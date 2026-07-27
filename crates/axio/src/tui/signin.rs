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
