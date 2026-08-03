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
            // `/auth`, not `/`. The home page is marketing and signs nobody in — it opened
            // to a blank window and stayed there. This path redirects into the OAuth flow
            // at auth.opencode.ai, which is the page someone can actually act on.
            url: "https://opencode.ai/auth",
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

    let url = site.origin.parse().map_err(|_| "Bad origin.".to_string())?;
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
        // Names, never values. This is the message someone stares at while a sign-in is
        // not completing, and "no session cookie yet" on its own cannot distinguish a page
        // that has not been signed in to from one whose cookie we are looking for under
        // the wrong name — which are opposite problems.
        let seen: Vec<&str> = cookies.iter().map(|cookie| cookie.name()).collect();
        return Err(if seen.is_empty() {
            "Nothing signed in yet — the window has no cookies for this site.".to_string()
        } else {
            format!(
                "Waiting for one of: {}. The window has {} cookies: {}",
                wanted.join(", "),
                seen.len(),
                seen.join(", ")
            )
        });
    }

    // A name is not proof. opencode sets a cookie called `auth` while the OAuth handshake
    // is still in flight, so the name check passed on a pre-sign-in artifact: the window
    // captured it, saved it and closed itself before anyone had signed in to anything.
    //
    // So the candidate is used, once, against the provider's own endpoint. Only a
    // credential that actually authenticates is accepted.
    if let Err(reason) = proves_signed_in(app, id, &header) {
        return Err(reason);
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
            config
                .providers
                .push(crate::config::ProviderConfig::new(id));
            config.providers.len() - 1
        }
    };
    config.providers[index].cookie_header = Some(header);
    config.providers[index].enabled = Some(true);
    config.save(&path).map_err(|err| err.to_string())?;

    Ok(format!("Connected to {}.", id.display_name()))
}

/// Ask the provider whether this credential is actually signed in.
///
/// Only `Unauthorized` means "keep waiting". Every other outcome — success, a missing
/// workspace, a network blip — means the session was recognised, which is the single thing
/// being established here. Treating any failure as "not yet" would leave the window open
/// forever on an account that is signed in but has nothing to report.
///
/// One request, and only once the cheap name check has already passed, so this does not run
/// on every poll.
fn proves_signed_in(app: &AppHandle, id: ProviderId, header: &str) -> Result<(), String> {
    let state = app.state::<Arc<AppState>>();
    let stored = Config::load(&Config::default_path(&state.env)).unwrap_or_default();

    let mut candidate = stored.provider_or_default(id);
    candidate.cookie_header = Some(header.to_string());

    let context = crate::provider::FetchContext::new(id)
        .map_err(|err| format!("Could not check the session: {err}"))?
        .with_env(state.env.clone())
        .with_config(candidate);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| format!("Could not check the session: {err}"))?;

    match runtime.block_on(crate::providers::by_id(id).fetch(&context)) {
        Err(crate::error::ProbeError::Unauthorized(detail)) => {
            Err(format!("Signed in page not finished yet — {detail}"))
        }
        _ => Ok(()),
    }
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
    eprintln!(
        "axio: opening sign-in window for {} at {}",
        id.as_str(),
        site.url
    );
    WebviewWindowBuilder::new(app, WINDOW, WebviewUrl::External(url))
        .title(site.title)
        .inner_size(920.0, 760.0)
        // Decorated, unlike the app's own windows: this is somebody else's page, and it
        // should look like a browser rather than like part of axio. A sign-in form inside
        // chrome that looks native is the shape of a phishing screen.
        .decorations(true)
        .build()
        .map_err(|err| {
            eprintln!("axio: sign-in window failed to open: {err}");
            format!("Could not open the sign-in window: {err}")
        })?;
    Ok(())
}

/// A one-shot check that the sign-in window and the cookie read both work.
///
/// `AXIO_CONNECT_PROBE=cursor` opens the window, waits, and prints the cookie names it can
/// see. It exists because the failure it diagnoses is invisible from the outside: a window
/// that opens but renders nothing, and a cookie read that returns nothing, look identical
/// to someone who has not signed in yet. Names only, never values.
pub fn spawn_probe(app: &AppHandle) {
    let Some(raw) = std::env::var("AXIO_CONNECT_PROBE").ok() else {
        return;
    };
    let Some(id) = ProviderId::parse(raw.trim()) else {
        eprintln!("axio: AXIO_CONNECT_PROBE names no provider: {raw}");
        return;
    };
    let app = app.clone();
    std::thread::spawn(move || {
        if let Err(err) = open(&app, id) {
            eprintln!("axio probe: window failed: {err}");
            return;
        }
        // Long enough for a page to load and set whatever it sets before anyone signs in.
        for round in 1..=6 {
            std::thread::sleep(std::time::Duration::from_secs(5));
            let Some(window) = app.get_webview_window(WINDOW) else {
                eprintln!("axio probe: window gone");
                return;
            };
            let url = site(id).map(|site| site.origin).unwrap_or_default();
            match url.parse().map(|url| window.cookies_for_url(url)) {
                Ok(Ok(cookies)) => {
                    let names: Vec<&str> = cookies.iter().map(|c| c.name()).collect();
                    eprintln!(
                        "axio probe {round}: {} cookies for {url}: {}",
                        names.len(),
                        names.join(", ")
                    );
                }
                Ok(Err(err)) => eprintln!("axio probe {round}: cookie read failed: {err}"),
                Err(_) => eprintln!("axio probe {round}: bad origin"),
            }
        }
    });
}

/// Try to capture, and report whether it worked.
///
/// Polled by the frontend rather than watched here: the interesting moment is "the session
/// cookie now exists", and no navigation event means that. A site can set it on a redirect,
/// on an XHR after the page settles, or on a second factor — so the reliable signal is the
/// cookie itself, asked for repeatedly while the window is open.
pub fn try_capture(app: &AppHandle, id: ProviderId) -> Result<String, String> {
    // Checked before anything blocking. Reading cookies waits on a reply from the event
    // loop, and asking a window that has already gone waits for a reply nobody will send.
    if app.get_webview_window(WINDOW).is_none() {
        return Err("The sign-in window is closed.".to_string());
    }
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
            assert!(
                site.url.starts_with("https://"),
                "{id} must sign in over TLS"
            );
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
