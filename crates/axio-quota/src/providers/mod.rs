pub mod claude;
pub mod codex;
pub mod cursor;
pub mod cursor_local;
pub mod deepseek;
pub mod grok;
pub mod ollama;
pub mod opencode;
pub mod openrouter;
pub mod xai;
pub mod zai;

use crate::model::ProviderId;
use crate::provider::Provider;

/// The cookies a site's session actually rides on, any one of which is enough.
///
/// Here so a diagnostic can answer the question people really have when a paste is
/// refused: not "is something wrong" but "which of these dozens of cookies did you need".
/// A page sets analytics, consent and feature-flag cookies alongside the session, and the
/// panel shows them all in one string — so a header can be pasted perfectly and still
/// carry nothing that signs you in, if it was copied from a request to a static asset.
pub fn session_cookie_names(id: ProviderId) -> &'static [&'static str] {
    match id {
        ProviderId::Cursor => &[
            "WorkosCursorSessionToken",
            "__Secure-next-auth.session-token",
            "next-auth.session-token",
        ],
        ProviderId::Ollama => &[
            "wos-session",
            "__Host-ollama_session",
            "ollama_session",
            "__Secure-next-auth.session-token",
            "next-auth.session-token",
        ],
        ProviderId::Opencode => &["__Host-auth", "auth"],
        _ => &[],
    }
}

/// The cookie a bare value belongs to, when someone pastes one.
///
/// The first entry of [`session_cookie_names`], which is the name the current site sets.
fn primary_cookie(id: ProviderId) -> Option<&'static str> {
    session_cookie_names(id).first().copied()
}

/// Turn whatever was pasted into a `Cookie` header value.
///
/// Two things get pasted and only one of them is a header. DevTools shows cookies in two
/// places: the Network panel's request headers, which give `name=value; name=value`, and
/// the Application panel's cookie table, where clicking a row copies the *value* alone. The
/// second is the easier thing to find and produces a string with no `=` in it anywhere —
/// which is not a header, carries no cookie name, and is refused by every site.
///
/// Rather than reject it, name it: a bare value is taken as the value of the provider's own
/// session cookie, which is the only cookie it could sensibly be. That turns a
/// certainly-broken paste into a probably-working one, and a wrong guess still fails with
/// the same message it would have anyway.
pub fn cookie_header_for(id: ProviderId, raw: &str) -> Option<String> {
    let cleaned = raw.trim().trim_start_matches("Cookie:").trim_start_matches("cookie:").trim();
    if let Some(header) = crate::provider::clean_cookie(cleaned) {
        return Some(header);
    }
    // No `=` anywhere: a lone value, not a header. Surrounding quotes come from a paste
    // out of a JSON view, and a cookie value never legitimately starts with one.
    let value = cleaned.trim().trim_matches('"').trim();
    if value.is_empty() {
        return None;
    }
    primary_cookie(id).map(|name| format!("{name}={value}"))
}

/// Which of the names a pasted header actually carries.
///
/// Matched on the name before `=`, so a value that happens to contain another cookie's
/// name cannot make a missing session look present.
pub fn cookies_present(header: &str, names: &[&str]) -> Vec<String> {
    let found: Vec<&str> = header
        .split(';')
        .filter_map(|pair| pair.split('=').next())
        .map(str::trim)
        .collect();
    names
        .iter()
        .filter(|name| found.contains(name))
        .map(|name| (*name).to_string())
        .collect()
}

/// Every provider this build knows about, in display order.
pub fn all() -> Vec<Box<dyn Provider>> {
    vec![
        Box::new(codex::CodexProvider),
        Box::new(claude::ClaudeProvider),
        Box::new(openrouter::OpenRouterProvider),
        Box::new(zai::ZaiProvider),
        Box::new(deepseek::DeepSeekProvider),
        Box::new(xai::XaiProvider),
        Box::new(grok::GrokProvider),
        Box::new(cursor::CursorProvider),
        Box::new(ollama::OllamaProvider),
        Box::new(opencode::OpenCodeProvider),
    ]
}

pub fn by_id(id: ProviderId) -> Box<dyn Provider> {
    match id {
        ProviderId::Codex => Box::new(codex::CodexProvider),
        ProviderId::Claude => Box::new(claude::ClaudeProvider),
        ProviderId::Openrouter => Box::new(openrouter::OpenRouterProvider),
        ProviderId::Zai => Box::new(zai::ZaiProvider),
        ProviderId::Deepseek => Box::new(deepseek::DeepSeekProvider),
        ProviderId::Xai => Box::new(xai::XaiProvider),
        ProviderId::Grok => Box::new(grok::GrokProvider),
        ProviderId::Cursor => Box::new(cursor::CursorProvider),
        ProviderId::Ollama => Box::new(ollama::OllamaProvider),
        ProviderId::Opencode => Box::new(opencode::OpenCodeProvider),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The check behind the diagnostic: a header full of analytics cookies must not read
    /// as a session just because it is long and well-formed.
    #[test]
    fn a_header_without_a_session_cookie_reports_none_found() {
        let names = session_cookie_names(ProviderId::Cursor);
        let analytics = "ph_phc_x=1; _ga=GA1.1.2; intercom-id-abc=def";
        assert!(cookies_present(analytics, names).is_empty());

        let real = "ph_phc_x=1; WorkosCursorSessionToken=user%3A%3Atok; _ga=GA1.1.2";
        assert_eq!(cookies_present(real, names), vec!["WorkosCursorSessionToken"]);
    }

    /// A value containing another cookie's name must not count as that cookie.
    #[test]
    fn a_name_inside_a_value_does_not_count() {
        let names = session_cookie_names(ProviderId::Opencode);
        assert!(cookies_present("other=__Host-auth-ish", names).is_empty());
        assert_eq!(cookies_present("__Host-auth=xyz", names), vec!["__Host-auth"]);
    }

    #[test]
    fn every_provider_id_has_an_implementation() {
        // Guards the match above: adding a ProviderId without a probe should fail here
        // rather than at runtime in the tray.
        for id in ProviderId::ALL {
            assert_eq!(by_id(*id).id(), *id);
        }
        assert_eq!(all().len(), ProviderId::ALL.len());
    }
}
