//! One implementation of secret scrubbing, and one place to audit it.
//!
//! Every provider body, header echo and raw SSE span crosses `Redacted` before
//! it can reach an error, a log, the JSON stream, or the session file.

use std::fmt;
use std::sync::{LazyLock, RwLock};

use serde::{Serialize, Serializer};

/// Strings registered here are scrubbed wherever they appear, in addition to
/// the structural key pattern. Registered once when a credential is loaded.
static SECRETS: LazyLock<RwLock<Vec<String>>> = LazyLock::new(|| RwLock::new(Vec::new()));

/// Register a live credential so it is scrubbed by value as well as by shape.
/// Short strings are ignored: registering a 3-character secret would redact
/// most of the English language.
pub fn register_secret(secret: impl Into<String>) {
    let secret = secret.into();
    if secret.len() < 8 {
        return;
    }
    let mut guard = SECRETS.write().unwrap_or_else(|e| e.into_inner());
    if !guard.contains(&secret) {
        guard.push(secret);
    }
}

#[cfg(test)]
fn clear_secrets() {
    SECRETS.write().unwrap_or_else(|e| e.into_inner()).clear();
}

const MASK: &str = "«redacted»";

/// Scrub `sk-ant-…` style keys and every registered credential.
pub fn scrub(input: &str) -> String {
    let mut out = scrub_key_pattern(input);
    let guard = SECRETS.read().unwrap_or_else(|e| e.into_inner());
    for secret in guard.iter() {
        if out.contains(secret.as_str()) {
            out = out.replace(secret.as_str(), MASK);
        }
    }
    out
}

/// Replace `sk-ant-` followed by 8 or more key characters.
///
/// Hand-rolled rather than pulling in `regex`: this is the only pattern we
/// match, and it keeps a regex engine out of the dependency-free core.
fn scrub_key_pattern(input: &str) -> String {
    const PREFIX: &str = "sk-ant-";
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if input[i..].starts_with(PREFIX) {
            let start = i + PREFIX.len();
            let mut end = start;
            while end < bytes.len() && is_key_byte(bytes[end]) {
                end += 1;
            }
            if end - start >= 8 {
                out.push_str(MASK);
                i = end;
                continue;
            }
        }
        // Advance one full character so we never split a UTF-8 sequence.
        let ch = input[i..].chars().next().expect("index is a char boundary");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn is_key_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

/// A string that cannot be printed, logged or serialised in the clear.
///
/// There is no accessor returning the raw contents. If you need the original
/// for a local comparison, you are holding the wrong type.
#[derive(Clone, PartialEq, Eq)]
pub struct Redacted(String);

impl Redacted {
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    /// Already-scrubbed length, useful in diagnostics that must not leak.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Display for Redacted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&scrub(&self.0))
    }
}

impl fmt::Debug for Redacted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Deliberately not `Redacted("…")` — a Debug that looks like a tuple
        // struct invites someone to reach for `.0`.
        write!(f, "{}", scrub(&self.0))
    }
}

impl Serialize for Redacted {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&scrub(&self.0))
    }
}

impl From<String> for Redacted {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for Redacted {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrubs_key_shape_in_display_debug_and_serde() {
        let r = Redacted::new("failed with key sk-ant-api03-AbCdEf123456_x-y and more");
        assert!(!format!("{r}").contains("sk-ant-api03"));
        assert!(!format!("{r:?}").contains("sk-ant-api03"));
        let json = serde_json::to_string(&r).unwrap();
        assert!(!json.contains("sk-ant-api03"));
        assert!(json.contains("redacted"));
        // Surrounding text survives.
        assert!(format!("{r}").starts_with("failed with key "));
        assert!(format!("{r}").ends_with(" and more"));
    }

    #[test]
    fn too_short_to_be_a_key_is_left_alone() {
        let r = Redacted::new("sk-ant-abc");
        assert_eq!(format!("{r}"), "sk-ant-abc");
    }

    #[test]
    fn scrubs_a_registered_credential_by_value() {
        clear_secrets();
        register_secret("hunter2-not-a-key-shape-at-all");
        let r = Redacted::new("Authorization: Bearer hunter2-not-a-key-shape-at-all");
        assert!(!format!("{r}").contains("hunter2"));
        clear_secrets();
    }

    #[test]
    fn ignores_a_secret_too_short_to_register() {
        clear_secrets();
        register_secret("abc");
        assert_eq!(scrub("abc def"), "abc def");
        clear_secrets();
    }

    #[test]
    fn does_not_split_multibyte_characters() {
        // A naive byte-walking scrubber panics or mangles here.
        let r = Redacted::new("naïve — sk-ant-api03-ZZZZZZZZ — 日本語");
        let shown = format!("{r}");
        assert!(shown.contains("naïve"));
        assert!(shown.contains("日本語"));
        assert!(!shown.contains("sk-ant-api03"));
    }
}
