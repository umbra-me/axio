//! Reading the token Claude Code wrote at login.

use serde_json::Value;
use time::OffsetDateTime;

use crate::error::ProbeError;
use crate::json::{as_i64, as_string};
use crate::paths::{Env, home_dir, non_empty};

#[derive(Debug, Clone)]
pub struct ClaudeCredentials {
    pub access_token: String,
    pub expires_at: Option<OffsetDateTime>,
    pub subscription_type: Option<String>,
}

impl ClaudeCredentials {
    pub fn is_expired(&self, now: OffsetDateTime) -> bool {
        self.expires_at.map(|expiry| now >= expiry).unwrap_or(false)
    }
}

/// `%USERPROFILE%\.claude\.credentials.json`.
///
/// Claude Code supports two overrides and they are not interchangeable:
/// `CLAUDE_SECURESTORAGE_CONFIG_DIR` moves only the credential store, while
/// `CLAUDE_CONFIG_DIR` moves the whole profile. Checking them in the wrong order reads
/// the wrong account for anyone who separates the two.
pub fn credentials_file(env: &Env) -> std::path::PathBuf {
    let root = match non_empty(env, "CLAUDE_SECURESTORAGE_CONFIG_DIR") {
        Some(secure) => std::path::PathBuf::from(secure),
        None => config_root(env),
    };
    root.join(".credentials.json")
}

fn config_root(env: &Env) -> std::path::PathBuf {
    match non_empty(env, "CLAUDE_CONFIG_DIR") {
        Some(explicit) => std::path::PathBuf::from(explicit),
        None => home_dir(env).join(".claude"),
    }
}

pub fn parse_credentials(raw: &str) -> Result<ClaudeCredentials, ProbeError> {
    let root: Value = serde_json::from_str(raw)
        .map_err(|err| ProbeError::decode("Claude .credentials.json", err))?;

    let oauth = root.get("claudeAiOauth").ok_or_else(|| {
        // A file containing only `mcpOAuth` means MCP servers are authorized but the user
        // never signed in to Claude Code itself — a different fix from "token expired".
        ProbeError::NotAuthenticated(
            "Claude credentials contain no claudeAiOauth entry. Run `claude` to log in."
                .to_string(),
        )
    })?;

    let access_token = as_string(oauth.get("accessToken")).ok_or_else(|| {
        ProbeError::NotAuthenticated(
            "Claude credentials have no access token. Run `claude` to log in.".to_string(),
        )
    })?;

    // `expiresAt` is milliseconds since the epoch, not seconds.
    let expires_at = as_i64(oauth.get("expiresAt"))
        .filter(|millis| *millis > 0)
        .and_then(|millis| {
            OffsetDateTime::from_unix_timestamp_nanos(millis as i128 * 1_000_000).ok()
        });

    Ok(ClaudeCredentials {
        access_token,
        expires_at,
        subscription_type: as_string(oauth.get("subscriptionType")),
    })
}

pub fn load_credentials(env: &Env) -> Result<ClaudeCredentials, ProbeError> {
    let path = credentials_file(env);
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(ProbeError::NotAuthenticated(format!(
                "Claude is not signed in ({} not found). Run `claude` to log in.",
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
    use time::format_description::well_known::Rfc3339;

    #[test]
    fn reads_the_oauth_block() {
        let raw = r#"{
            "claudeAiOauth": {
                "accessToken": "sk-ant-oat-1",
                "refreshToken": "sk-ant-ort-1",
                "expiresAt": 4102444800000,
                "subscriptionType": "max"
            }
        }"#;
        let credentials = parse_credentials(raw).expect("parses");
        assert_eq!(credentials.access_token, "sk-ant-oat-1");
        assert_eq!(credentials.subscription_type.as_deref(), Some("max"));
        assert!(!credentials.is_expired(OffsetDateTime::now_utc()));
    }

    #[test]
    fn mcp_only_credentials_are_reported_as_not_signed_in() {
        let raw = r#"{ "mcpOAuth": { "some-server": { "accessToken": "x" } } }"#;
        let error = parse_credentials(raw).expect_err("should not authenticate");
        assert!(error.needs_user_action());
    }

    #[test]
    fn expiry_is_milliseconds_not_seconds() {
        // 2020-01-01T00:00:00Z in millis. Read as seconds this would land in the year 51,
        // and the token would never look expired.
        let raw = r#"{ "claudeAiOauth": { "accessToken": "t", "expiresAt": 1577836800000 } }"#;
        let credentials = parse_credentials(raw).expect("parses");
        assert_eq!(
            credentials.expires_at.unwrap().format(&Rfc3339).unwrap(),
            "2020-01-01T00:00:00Z"
        );
        assert!(credentials.is_expired(OffsetDateTime::now_utc()));
    }

    #[test]
    fn secure_storage_override_beats_the_profile_override() {
        let env: Env = [
            ("CLAUDE_CONFIG_DIR".to_string(), r"D:\profile".to_string()),
            (
                "CLAUDE_SECURESTORAGE_CONFIG_DIR".to_string(),
                r"D:\secrets".to_string(),
            ),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            credentials_file(&env),
            std::path::PathBuf::from(r"D:\secrets").join(".credentials.json")
        );
    }
}
