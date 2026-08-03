//! Signing in to a cookie provider, without anybody copying a header.
//!
//! Three providers here have no API key: their usage lives behind a browser session, and
//! the only way to get one was to open the dev tools, find the right request, and paste its
//! `cookie:` line. That asks someone to know which of thirty cookies is the session, and
//! the two obvious copy routes both produce something the server refuses — a bare value
//! with no name, or the header with its own name still on the front.
//!
//! So the app signs in instead. A window opens on the provider's real sign-in page, the
//! user signs in to that vendor exactly as they would in a browser, and the cookies land in
//! this webview where [`tauri::Webview::cookies_for_url`] can read them back.
//!
//! Two properties worth stating, because both were requirements rather than conveniences:
//!
//! * The credential never goes anywhere but the vendor. There is no relay, no extension and
//!   no reading of another browser's cookie store — the password is typed into the
//!   provider's own page, over TLS, to the provider.
//! * Nothing is decrypted. Reading Chrome's jar would mean DPAPI and AES-GCM against a key
//!   the OS holds for a different application, which is a lot of machinery to acquire a
//!   credential the user can simply grant.

use std::sync::Arc;

use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::config::Config;
use crate::model::ProviderId;

use super::state::AppState;

/// Emitted when a sign-in window captures a session, so Settings can refresh itself.
pub const EVENT_CONNECTED: &str = "quota://connected";

const WINDOW: &str = "connect";

/// Where to send someone to sign in, and which host's cookies to read afterwards.
struct Site {
    /// The page to open. A dashboard rather than a sign-in form: an already-signed-in user
    /// lands on their data and the window closes at once, and a signed-out one is
    /// redirected to sign in by the site itself. Guessing the sign-in URL would break the
    /// day the vendor moves it.
    url: &'static str,
    /// The origin whose cookies are captured.
    origin: &'static str,
    title: &'static str,
}

fn site(id: ProviderId) -> Option<Site> {
    match id {
        ProviderId::Cursor => Some(Site {
            url: "https://cursor.com/dashboard",
            origin: "https://cursor.com",
            title: "Sign in to Cursor",
        }),
        ProviderId::Ollama => Some(Site {
            url: "https://ollama.com/settings",
            origin: "https://ollama.com",
            title: "Sign in to Ollama",
        }),
        ProviderId::Opencode => Some(Site {
            url: "https://opencode.ai/",
            origin: "https://opencode.ai",
            title: "Sign in to opencode",
        }),
        _ => None,
    }
}

/// Whether this provider is signed in through a window rather than a pasted key.
pub fn is_connectable(id: ProviderId) -> bool {
    site(id).is_some()
}

/// Read the session out of a webview and write it into the config.
///
/// Every cookie the origin holds is kept, not just the session one. A dashboard request
/// carries the whole jar, and some of these sites pair the session with a CSRF or region
/// cookie that the endpoint also checks — keeping only the one we recognise would work
/// until it quietly did not.
fn capture(app: &AppHandle, id: ProviderId) -> Result<String, String> {
    let Some(site) = site(id) else {
        return Err(format!("{} does not sign in this way.", id.display_name()));
    };
    let window = app
        .get_webview_window(WINDOW)
        .ok_or_else(|| "The sign-in window is gone.".to_string())?;

    let url = site
        .origin
        .parse()
        .map_err(|_| "Bad origin.".to_string())?;
    let cookies = window
        .cookies_for_url(url)
        .map_err(|err| format!("Could not read the sign-in window's cookies: {err}"))?;

    let header = cookies
        .iter()
        .map(|cookie| format!("{}={}", cookie.name(), cookie.value()))
        .collect::<Vec<_>>()
        .join("; ");

    let wanted = crate::providers::session_cookie_names(id);
    if crate::providers::cookies_present(&header, wanted).is_empty() {
        return Err(format!(
            "Signed in, but no session cookie yet. Waiting for one of: {}",
            wanted.join(", ")
        ));
    }

    let state = app.state::<Arc<AppState>>();
    let path = Config::default_path(&state.env);
    let mut config = Config::load(&path).unwrap_or_default();
    let index = match config
        .providers
        .iter()
        .position(|entry| entry.provider_id() == Some(id))
    {
        Some(index) => index,
        None => {
            config.providers.push(crate::config::ProviderConfig::new(id));
            config.providers.len() - 1
        }
    };
    config.providers[index].cookie_header = Some(header);
    config.providers[index].enabled = Some(true);
    config.save(&path).map_err(|err| err.to_string())?;

    Ok(format!("Connected to {}.", id.display_name()))
}

/// Open the provider's own page so the user can sign in.
pub fn open(app: &AppHandle, id: ProviderId) -> Result<(), String> {
    let Some(site) = site(id) else {
        return Err(format!("{} does not sign in this way.", id.display_name()));
    };
    // One window, reused. Two sign-in windows for two providers would share a cookie jar
    // anyway, and the second would land on top of the first with no way to tell them apart.
    if let Some(existing) = app.get_webview_window(WINDOW) {
        let _ = existing.close();
    }

    let url = site
        .url
        .parse()
        .map_err(|_| "Bad sign-in URL.".to_string())?;
    WebviewWindowBuilder::new(app, WINDOW, WebviewUrl::External(url))
        .title(site.title)
        .inner_size(920.0, 760.0)
        // Decorated, unlike the app's own windows: this is somebody else's page, and it
        // should look like a browser rather than like part of axio. A sign-in form inside
        // chrome that looks native is the shape of a phishing screen.
        .decorations(true)
        .build()
        .map_err(|err| err.to_string())?;
    Ok(())
}

/// Try to capture, and report whether it worked.
///
/// Polled by the frontend rather than watched here: the interesting moment is "the session
/// cookie now exists", and no navigation event means that. A site can set it on a redirect,
/// on an XHR after the page settles, or on a second factor — so the reliable signal is the
/// cookie itself, asked for repeatedly while the window is open.
pub fn try_capture(app: &AppHandle, id: ProviderId) -> Result<String, String> {
    let outcome = capture(app, id);
    if outcome.is_ok() {
        if let Some(window) = app.get_webview_window(WINDOW) {
            let _ = window.close();
        }
        let _ = app.emit(EVENT_CONNECTED, id.as_str());
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The button only appears for providers that sign in this way, and every one of them
    /// needs a site to open. A mismatch would render a button that cannot work.
    #[test]
    fn every_connectable_provider_has_a_site_and_a_session_cookie() {
        for id in ProviderId::ALL {
            if !is_connectable(*id) {
                continue;
            }
            let site = site(*id).expect("a connectable provider has a site");
            assert!(site.url.starts_with("https://"), "{id} must sign in over TLS");
            assert!(site.origin.starts_with("https://"));
            assert!(
                !crate::providers::session_cookie_names(*id).is_empty(),
                "{id} has nothing to look for once signed in"
            );
        }
    }

    /// The three cookie providers are exactly the connectable ones — a provider that needs
    /// a pasted header and has no button is the gap this was built to close.
    #[test]
    fn the_cookie_providers_are_the_connectable_ones() {
        for id in ProviderId::ALL {
            let needs_cookie = !crate::providers::session_cookie_names(*id).is_empty();
            assert_eq!(needs_cookie, is_connectable(*id), "{id}");
        }
    }
}
