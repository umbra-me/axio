//! Reading the token the `codex` CLI wrote at login.

use serde_json::Value;

use crate::error::ProbeError;
use crate::json::{as_string, pick};
use crate::paths::{Env, home_dir, non_empty};

#[derive(Debug, Clone)]
pub struct CodexCredentials {
    pub access_token: String,
    pub account_id: Option<String>,
}

/// `%USERPROFILE%\.codex\auth.json`, or `$CODEX_HOME\auth.json`.
pub fn auth_file(env: &Env) -> std::path::PathBuf {
    codex_home(env).join("auth.json")
}

pub(super) fn codex_home(env: &Env) -> std::path::PathBuf {
    match non_empty(env, "CODEX_HOME") {
        Some(explicit) => std::path::PathBuf::from(explicit),
        None => home_dir(env).join(".codex"),
    }
}

/// Two shapes live in this file: an OAuth block under `tokens`, or a bare `OPENAI_API_KEY`
/// for users who authenticate with a key instead. Both are valid; the key case has no
/// refresh token and no account id.
pub fn parse_credentials(raw: &str) -> Result<CodexCredentials, ProbeError> {
    let root: Value =
        serde_json::from_str(raw).map_err(|err| ProbeError::decode("Codex auth.json", err))?;

    if let Some(api_key) = as_string(root.get("OPENAI_API_KEY")) {
        return Ok(CodexCredentials {
            access_token: api_key,
            account_id: None,
        });
    }

    let tokens = root.get("tokens").ok_or_else(|| {
        ProbeError::NotAuthenticated(
            "Codex auth.json has no tokens. Run `codex` to log in.".to_string(),
        )
    })?;

    let access_token =
        as_string(pick(tokens, &["access_token", "accessToken"])).ok_or_else(|| {
            ProbeError::NotAuthenticated(
                "Codex auth.json has no access token. Run `codex` to log in.".to_string(),
            )
        })?;

    Ok(CodexCredentials {
        access_token,
        account_id: as_string(pick(tokens, &["account_id", "accountId"])),
    })
}

pub fn load_credentials(env: &Env) -> Result<CodexCredentials, ProbeError> {
    let path = auth_file(env);
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(ProbeError::NotAuthenticated(format!(
                "Codex is not signed in ({} not found). Run `codex` to log in.",
                path.display()
            )));
        }
        Err(err) => {
            return Err(ProbeError::Io {
                path: path.display().to_string(),
                detail: err.to_string(),
            });
        }
    };
    parse_credentials(&raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_oauth_tokens() {
        let raw = r#"{
            "tokens": { "access_token": "at-1", "refresh_token": "rt-1", "account_id": "acct-9" },
            "last_refresh": "2026-01-01T00:00:00Z"
        }"#;
        let credentials = parse_credentials(raw).expect("parses");
        assert_eq!(credentials.access_token, "at-1");
        assert_eq!(credentials.account_id.as_deref(), Some("acct-9"));
    }

    #[test]
    fn reads_camel_case_tokens() {
        let raw = r#"{ "tokens": { "accessToken": "at-2", "accountId": "acct-2" } }"#;
        let credentials = parse_credentials(raw).expect("parses");
        assert_eq!(credentials.access_token, "at-2");
        assert_eq!(credentials.account_id.as_deref(), Some("acct-2"));
    }

    #[test]
    fn falls_back_to_a_bare_api_key() {
        let raw = r#"{ "OPENAI_API_KEY": "sk-test", "tokens": null }"#;
        let credentials = parse_credentials(raw).expect("parses");
        assert_eq!(credentials.access_token, "sk-test");
        assert!(credentials.account_id.is_none());
    }

    #[test]
    fn codex_home_env_overrides_the_profile_default() {
        let env: Env = [("CODEX_HOME".to_string(), r"D:\codex".to_string())]
            .into_iter()
            .collect();
        assert_eq!(
            auth_file(&env),
            std::path::PathBuf::from(r"D:\codex").join("auth.json")
        );
    }
}
