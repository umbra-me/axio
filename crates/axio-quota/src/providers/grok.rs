//! Grok, from the Grok Build CLI's own credential file.
//!
//! Two halves with very different confidence. Identity — who is signed in, on what kind of
//! principal, and whether the token has expired — comes from `~/.grok/auth.json`, which is
//! a local file this reads directly and can be tested exactly. Usage comes from grok.com's
//! billing RPC, which is the part that may not answer: the CLI's own `x.ai/billing` method
//! is not wired to the agent surface in current releases, and the web endpoint expects a
//! browser session rather than the CLI's bearer.
//!
//! So the rule here is that identity is reported whenever the file is readable, and usage
//! is best-effort on top of it. A provider that says "signed in as you, usage unavailable"
//! is useful; one that says nothing because the second half failed is not.

use async_trait::async_trait;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::error::ProbeError;
use crate::json::{as_f64, as_string, pick};
use crate::model::{Credits, ProviderId, UsageSnapshot};
use crate::paths::{Env, home_dir};
use crate::provider::{FetchContext, Provider};

const BILLING_URL: &str = "https://grok.com/grok_api_v2.GrokBuildBilling/GetGrokCreditsConfig";

/// The prefix the CLI keys its credential entry with. The rest of the key is an install
/// id, so the entry has to be found by prefix rather than by name.
const ISSUER_PREFIX: &str = "https://auth.x.ai::";

pub struct GrokProvider;

/// What the CLI's credential file says about who is signed in.
#[derive(Debug, Clone, PartialEq)]
pub struct Identity {
    pub token: String,
    pub email: Option<String>,
    pub first_name: Option<String>,
    /// `User` or a team principal. Grok exposes no team usage surface, so a team principal
    /// gets identity only — reported as such rather than as a failure.
    pub principal_type: Option<String>,
    pub expired: bool,
}

impl Identity {
    pub fn is_team(&self) -> bool {
        self.principal_type
            .as_deref()
            .is_some_and(|kind| !kind.eq_ignore_ascii_case("user"))
    }
}

/// Read the CLI's credential file.
///
/// `now` is passed rather than read so expiry is a test rather than a wait.
pub fn parse_auth(raw: &str, now: OffsetDateTime) -> Result<Identity, ProbeError> {
    let root: serde_json::Value =
        serde_json::from_str(raw).map_err(|err| ProbeError::decode("Grok auth.json", err))?;

    let entry = root
        .as_object()
        .and_then(|map| {
            map.iter()
                .find(|(key, _)| key.starts_with(ISSUER_PREFIX))
                .map(|(_, value)| value)
        })
        .ok_or_else(|| {
            ProbeError::NotConfigured(
                "No x.ai entry in ~/.grok/auth.json. Run `grok` and sign in.".to_string(),
            )
        })?;

    let token = as_string(pick(entry, &["key", "access_token", "accessToken"]))
        .filter(|token| !token.trim().is_empty())
        .ok_or_else(|| ProbeError::decode("Grok auth.json", "entry carries no token"))?;

    // An expired token is not sent. It would be refused, and sending a stale credential to
    // a server that has already rejected it is worth avoiding for its own sake.
    let expired = as_string(pick(entry, &["expires_at", "expiresAt"]))
        .and_then(|text| OffsetDateTime::parse(&text, &Rfc3339).ok())
        .is_some_and(|at| at <= now);

    Ok(Identity {
        token,
        email: as_string(pick(entry, &["email"])),
        first_name: as_string(pick(entry, &["first_name", "firstName"])),
        principal_type: as_string(pick(entry, &["principal_type", "principalType"])),
        expired,
    })
}

/// Fold whatever the billing RPC returned into the snapshot.
///
/// Deliberately permissive about field names and strict about the outcome: if nothing that
/// looks like a credit figure is present, the snapshot is left alone rather than being
/// given a zero. This endpoint is the least certain thing in the provider, and a wrong
/// balance is worse than an absent one.
pub fn apply_credits(snapshot: &mut UsageSnapshot, raw: &str) -> bool {
    let Ok(root) = serde_json::from_str::<serde_json::Value>(raw) else {
        return false;
    };
    let data = root
        .get("config")
        .or_else(|| root.get("data"))
        .unwrap_or(&root);

    let Some(remaining) = as_f64(pick(
        data,
        &[
            "credits",
            "remaining_credits",
            "remainingCredits",
            "balance",
        ],
    )) else {
        return false;
    };

    snapshot.credits = Some(Credits {
        balance: Some(remaining),
        unlimited: false,
        has_credits: remaining > 0.0,
    });
    true
}

fn auth_path(env: &Env) -> Option<std::path::PathBuf> {
    home_dir(env).join(".grok").join("auth.json").into()
}

#[async_trait]
impl Provider for GrokProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Grok
    }

    fn credential_hint(&self, env: &Env) -> String {
        match auth_path(env) {
            Some(path) => path.display().to_string(),
            None => "~/.grok/auth.json".to_string(),
        }
    }

    async fn fetch(&self, ctx: &FetchContext) -> Result<UsageSnapshot, ProbeError> {
        let path = auth_path(&ctx.env).ok_or_else(|| {
            ProbeError::NotConfigured("No home directory to find ~/.grok in.".to_string())
        })?;
        let raw = std::fs::read_to_string(&path).map_err(|err| {
            ProbeError::NotConfigured(format!(
                "Cannot read {}: {err}. Install the Grok CLI and sign in.",
                path.display()
            ))
        })?;
        let identity = parse_auth(&raw, OffsetDateTime::now_utc())?;

        let mut snapshot = UsageSnapshot::new(ProviderId::Grok);
        snapshot.account_label = identity
            .email
            .clone()
            .or_else(|| identity.first_name.clone());
        snapshot.plan = identity.principal_type.clone();

        if identity.expired {
            return Err(ProbeError::Unauthorized(
                "The Grok CLI's saved token has expired. Run `grok` to sign in again.".to_string(),
            ));
        }
        // Grok exposes no team usage surface. Saying so beats forwarding whatever the
        // personal endpoint rejects a team principal with.
        if identity.is_team() {
            return Ok(snapshot);
        }

        // Best effort from here. A failure leaves the identity standing rather than losing
        // the whole provider — see the module note.
        let response = ctx
            .http
            .post(BILLING_URL)
            .header("Authorization", format!("Bearer {}", identity.token))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .body("{}")
            .send()
            .await;

        if let Ok(response) = response
            && response.status().is_success()
            && let Ok(body) = response.text().await
        {
            apply_credits(&mut snapshot, &body);
        }
        Ok(snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn now() -> OffsetDateTime {
        OffsetDateTime::parse("2026-08-03T12:00:00Z", &Rfc3339).expect("fixed stamp")
    }

    fn auth(expires: &str) -> String {
        format!(
            r#"{{"https://auth.x.ai::b1a00492-073a-47ea-816f-4c329264a828":{{
                "key":"ey.token","email":"a@b.co","first_name":"Kian",
                "principal_type":"User","expires_at":"{expires}"}}}}"#
        )
    }

    /// The entry is keyed by issuer plus an install id, so it has to be found by prefix.
    #[test]
    fn the_credential_entry_is_found_by_its_issuer_prefix() {
        let identity = parse_auth(&auth("2026-09-01T00:00:00Z"), now()).expect("parses");
        assert_eq!(identity.token, "ey.token");
        assert_eq!(identity.email.as_deref(), Some("a@b.co"));
        assert_eq!(identity.principal_type.as_deref(), Some("User"));
        assert!(!identity.expired);
        assert!(!identity.is_team());
    }

    #[test]
    fn an_expired_token_is_flagged_rather_than_used() {
        let identity = parse_auth(&auth("2026-07-01T00:00:00Z"), now()).expect("parses");
        assert!(identity.expired);
    }

    /// A team principal has no usage surface, and identity-only is the correct answer.
    #[test]
    fn a_team_principal_is_recognised() {
        let raw = r#"{"https://auth.x.ai::x":{"key":"t","principal_type":"Team"}}"#;
        assert!(parse_auth(raw, now()).unwrap().is_team());
    }

    #[test]
    fn a_file_with_no_x_ai_entry_says_to_sign_in() {
        assert!(matches!(
            parse_auth(r#"{"https://other::1":{"key":"t"}}"#, now()),
            Err(ProbeError::NotConfigured(_))
        ));
        assert!(parse_auth(r#"{"https://auth.x.ai::x":{}}"#, now()).is_err());
    }

    /// The billing endpoint is the least certain thing here. An unrecognised answer must
    /// leave the snapshot alone rather than write a zero balance into it.
    #[test]
    fn an_unrecognised_billing_answer_changes_nothing() {
        let mut snapshot = UsageSnapshot::new(ProviderId::Grok);
        assert!(!apply_credits(&mut snapshot, "not json"));
        assert!(!apply_credits(&mut snapshot, r#"{"unexpected":true}"#));
        assert!(snapshot.credits.is_none());

        assert!(apply_credits(
            &mut snapshot,
            r#"{"config":{"credits":42.5}}"#
        ));
        assert_eq!(snapshot.credits.unwrap().balance, Some(42.5));
    }
}
