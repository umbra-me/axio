//! Cursor's session, from Cursor's own files.
//!
//! Cursor signs itself in and writes the result to a VS Code-style state database in its
//! own application-data directory. That token is the same credential the dashboard uses, so
//! anyone with Cursor installed and signed in already has everything needed — no window to
//! sign in to, and nothing to paste.
//!
//! Worth being precise about what this reads, because a neighbouring idea is much worse:
//! this opens **Cursor's own file**, not a browser's cookie store. No decryption, no DPAPI,
//! no key belonging to another application. If Cursor is signed in, the token is sitting
//! there in plain text because Cursor put it there for itself.
//!
//! The database is copied before it is read. Cursor holds it open while running, and SQLite
//! on a live database either fails or, worse, recovers a journal — writing into a file
//! belonging to another program that is still using it.

use std::path::PathBuf;

use crate::error::ProbeError;
use crate::paths::{Env, app_data_dir, home_dir};

/// Where Cursor keeps its state database.
///
/// Roaming application data on Windows, which is where VS Code-derived editors put
/// `globalStorage` — not the local data directory the scan cache uses.
pub fn state_db(env: &Env) -> PathBuf {
    let relative = ["Cursor", "User", "globalStorage", "state.vscdb"];

    #[cfg(target_os = "windows")]
    let base = app_data_dir(env);
    #[cfg(target_os = "macos")]
    let base = home_dir(env).join("Library").join("Application Support");
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let base = home_dir(env).join(".config");

    relative.iter().fold(base, |path, part| path.join(part))
}

/// The key Cursor stores its bearer token under.
const TOKEN_KEY: &str = "cursorAuth/accessToken";

/// Read the token Cursor wrote for itself.
#[cfg(feature = "sqlite")]
pub fn access_token(env: &Env) -> Result<String, ProbeError> {
    let path = state_db(env);
    if !path.exists() {
        return Err(ProbeError::NotConfigured(format!(
            "No Cursor state at {}. Sign in to Cursor, or paste a Cookie header.",
            path.display()
        )));
    }

    // Copied first: Cursor keeps this open, and opening a live SQLite database read-write
    // can replay its journal into a file another program is still using.
    let scratch = std::env::temp_dir().join("axio-cursor-state.vscdb");
    std::fs::copy(&path, &scratch)
        .map_err(|err| ProbeError::NotConfigured(format!("Cannot read Cursor's state: {err}")))?;

    let outcome = read_token(&scratch);
    let _ = std::fs::remove_file(&scratch);
    outcome
}

#[cfg(feature = "sqlite")]
fn read_token(path: &std::path::Path) -> Result<String, ProbeError> {
    let connection = rusqlite::Connection::open_with_flags(
        format!("file:{}?mode=ro", path.display()),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|err| ProbeError::NotConfigured(format!("Cannot open Cursor's state: {err}")))?;

    // The column is typed loosely by VS Code and comes back as text or as a blob depending
    // on how it was written, so both are accepted rather than one guessed at.
    let token: Option<String> = connection
        .query_row(
            "SELECT value FROM ItemTable WHERE key = ?1 LIMIT 1",
            [TOKEN_KEY],
            |row| {
                row.get::<_, String>(0).or_else(|_| {
                    row.get::<_, Vec<u8>>(0)
                        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                })
            },
        )
        .ok();

    token
        .map(|token| token.trim().trim_matches('"').to_string())
        .filter(|token| !token.is_empty())
        .ok_or_else(|| {
            ProbeError::NotConfigured(
                "Cursor is installed but not signed in — no token in its state store."
                    .to_string(),
            )
        })
}

#[cfg(not(feature = "sqlite"))]
pub fn access_token(_env: &Env) -> Result<String, ProbeError> {
    Err(ProbeError::NotConfigured(
        "This build cannot read Cursor's state store. Paste a Cookie header instead."
            .to_string(),
    ))
}

/// The account id a Cursor token was issued for.
///
/// Read out of the token's own payload rather than from a second file: a JWT carries its
/// subject, and taking it from the credential itself means the pair can never disagree
/// about which account they belong to.
///
/// Nothing is verified. There is no signature check here and there should not be — this is
/// not authenticating anybody, it is reading a field out of a credential the user already
/// holds, to hand straight back to the service that issued it.
pub fn subject(token: &str) -> Option<String> {
    use base64::Engine;

    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let subject = claims.get("sub")?.as_str()?.trim();
    (!subject.is_empty()).then(|| subject.to_string())
}

/// Percent-encode a subject for a cookie value.
///
/// Cursor's own cookie is URL-encoded — the `::` between the two halves is stored as
/// `%3A%3A` — so the subject is encoded to match. Subjects look like `auth0|user_01ABC`,
/// and the pipe is the character that matters.
pub fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A token's own payload names the account, so no second lookup can disagree with it.
    #[test]
    fn the_subject_comes_out_of_the_token() {
        // header.payload.signature, payload = {"sub":"auth0|user_01ABC","exp":1}
        let token = "aGVhZGVy.eyJzdWIiOiJhdXRoMHx1c2VyXzAxQUJDIiwiZXhwIjoxfQ.c2ln";
        assert_eq!(subject(token).as_deref(), Some("auth0|user_01ABC"));
    }

    #[test]
    fn a_token_that_is_not_a_jwt_has_no_subject() {
        assert_eq!(subject("not-a-jwt"), None);
        assert_eq!(subject("a.b.c"), None);
        assert_eq!(subject(""), None);
    }

    /// The pipe is the character this exists for: unencoded it changes what the server
    /// reads back as the account id.
    #[test]
    fn a_subject_is_percent_encoded() {
        assert_eq!(encode("auth0|user_01ABC"), "auth0%7Cuser_01ABC");
        assert_eq!(encode("user_01ABC"), "user_01ABC");
        assert_eq!(encode("a.b-c_d~e"), "a.b-c_d~e");
    }

    /// The path is where Cursor actually writes, which is worth pinning: it is the roaming
    /// directory on Windows, not the local one the cost cache uses.
    #[test]
    fn the_state_path_is_cursors_own_directory() {
        let env: Env = [
            ("APPDATA".to_string(), "C:\\Users\\x\\AppData\\Roaming".to_string()),
            ("HOME".to_string(), "/home/x".to_string()),
        ]
        .into_iter()
        .collect();

        let path = state_db(&env);
        assert!(path.ends_with("state.vscdb"), "{}", path.display());
        assert!(
            path.to_string_lossy().contains("globalStorage"),
            "{}",
            path.display()
        );
        assert!(path.to_string_lossy().contains("Cursor"));
    }
}
