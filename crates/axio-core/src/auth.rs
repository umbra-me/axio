//! Where credentials live, and what is honestly claimed about protecting them.
//!
//! An environment variable always wins over the file. That ordering is
//! deliberate: it makes a stored credential easy to override for one command
//! without editing anything, and it means CI, which sets the variable, never
//! silently picks up a developer's saved key.
//!
//! On unix the file is created `0600` — at creation, not afterwards, because
//! chmod-after-write leaves a window where it is world-readable. On Windows no
//! protection is claimed at all. The predecessor project shipped a
//! `restrict_to_owner` that was a documented no-op there, and a false claim
//! about a credential is worse than an honest absence of one.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Bumped when the on-disk shape changes.
///
/// 2 added OAuth entries beside API keys. A version 1 file still loads: an
/// entry is a struct with two optional fields rather than a tagged union, so
/// the old `{"api_key": …}` reads as an entry whose token half is absent. That
/// matters because the alternative — refusing to load — logs someone out of
/// every provider to gain a field they may never use.
pub const AUTH_FORMAT_VERSION: u32 = 2;

const FILE_NAME: &str = "auth.json";

/// Where a credential came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// An environment variable, which outranks the file.
    Env(String),
    Stored(PathBuf),
}

impl Source {
    pub fn describe(&self) -> String {
        match self {
            Source::Env(var) => format!("environment ({var})"),
            Source::Stored(path) => format!("stored ({})", path.display()),
        }
    }
}

/// Every provider name axio accepts.
///
/// One list, so `auth status`, `auth login` and the run itself cannot disagree
/// about what exists — a name only one of them knows produces a credential that
/// stores fine and can never be used or listed.
pub const PROVIDERS: &[&str] = &["anthropic", "ollama", "openai-compatible", "openai-codex"];

pub fn is_known(provider: &str) -> bool {
    PROVIDERS.contains(&provider)
}

/// The environment variable a provider reads, if it has one.
pub fn env_var_for(provider: &str) -> Option<&'static str> {
    match provider {
        "anthropic" => Some("ANTHROPIC_API_KEY"),
        "ollama" | "openai-compatible" => Some("OLLAMA_API_KEY"),
        // Deliberately none. This provider's credential is a token pair with an
        // expiry that axio refreshes and writes back; a variable holding one
        // would go stale in the shell that set it, and the failure — a token
        // that worked this morning — would point at everything except the
        // export that is shadowing the fresh one.
        "openai-codex" => None,
        _ => None,
    }
}

/// A credential. Never printed, never logged, never in a `Debug` dump.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Hand the credential to the code that must send it. The only accessor,
    /// so `grep expose` finds every place a key leaves this module.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.trim().is_empty()
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Not `Secret("…")`: a Debug that looks like a tuple struct invites
        // someone to reach for `.0`.
        write!(f, "«{} characters, not shown»", self.0.len())
    }
}

/// A token pair from an OAuth exchange, and what is needed to use and renew it.
///
/// `expires_at_ms` is absolute rather than a lifetime, because a lifetime is
/// only meaningful next to the instant it was issued — and that instant is not
/// what gets written to disk, read back a week later and compared against now.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthTokens {
    pub access: Secret,
    pub refresh: Secret,
    pub expires_at_ms: u64,
    /// Which account the token speaks for, when the issuer says so. Some
    /// endpoints require it as a header and reject the token without it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
}

impl OAuthTokens {
    /// Whether this needs renewing before the next request.
    ///
    /// `skew_ms` is subtracted from the expiry, because a token that is valid
    /// for another two seconds is not valid for a request that takes three. The
    /// caller picks the margin; nothing here guesses at network latency.
    pub fn expired(&self, now_ms: u64, skew_ms: u64) -> bool {
        now_ms.saturating_add(skew_ms) >= self.expires_at_ms
    }
}

impl std::fmt::Debug for OAuthTokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OAuthTokens")
            .field("access", &self.access)
            .field("refresh", &self.refresh)
            .field("expires_at_ms", &self.expires_at_ms)
            .field("account_id", &self.account_id)
            .finish()
    }
}

/// What a provider was given to authenticate with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Credential {
    Key(Secret),
    OAuth(OAuthTokens),
}

impl Credential {
    /// The bearer value a request carries, whichever kind this is.
    pub fn bearer(&self) -> &Secret {
        match self {
            Credential::Key(key) => key,
            Credential::OAuth(tokens) => &tokens.access,
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Credential::Key(key) => key.is_empty(),
            Credential::OAuth(tokens) => tokens.access.is_empty() || tokens.refresh.is_empty(),
        }
    }

    /// What `auth status` calls it. Not the value, and never the value.
    pub fn kind(&self) -> &'static str {
        match self {
            Credential::Key(_) => "api key",
            Credential::OAuth(_) => "oauth",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Store {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    providers: BTreeMap<String, Entry>,
}

/// Two optional fields rather than a tagged union, so a version 1 file — which
/// knew only `api_key` — still deserialises. A tag would have made every
/// existing entry unreadable, and being logged out of every provider is a steep
/// price for a field most people will never use.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Entry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    api_key: Option<Secret>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    oauth: Option<OAuthTokens>,
}

impl Entry {
    fn credential(&self) -> Option<Credential> {
        // Tokens first. An entry holding both is one that was an API key and
        // was then signed in to; the newer kind is the one that was chosen.
        if let Some(tokens) = &self.oauth {
            return Some(Credential::OAuth(tokens.clone()));
        }
        self.api_key.clone().map(Credential::Key)
    }
}

pub fn auth_path(home: &Path) -> PathBuf {
    home.join(FILE_NAME)
}

/// Resolve a provider's credential, environment first.
pub fn resolve(
    provider: &str,
    home: &Path,
    env: &[(String, String)],
) -> Option<(Credential, Source)> {
    if let Some(var) = env_var_for(provider)
        && let Some((_, value)) = env.iter().find(|(k, _)| k == var)
        && !value.trim().is_empty()
    {
        return Some((
            Credential::Key(Secret::new(value.clone())),
            Source::Env(var.to_owned()),
        ));
    }

    let path = auth_path(home);
    let store = read_store(&path).ok()?;
    let credential = store.providers.get(provider)?.credential()?;
    if credential.is_empty() {
        return None;
    }
    Some((credential, Source::Stored(path)))
}

/// Every provider that has a credential, and where it came from.
///
/// Deliberately does not return the credentials themselves: `status` needs to
/// report what exists, and a function that hands back keys for a listing is a
/// function someone will eventually print.
pub fn status(
    providers: &[&str],
    home: &Path,
    env: &[(String, String)],
) -> Vec<(String, Option<Source>)> {
    providers
        .iter()
        .map(|p| {
            (
                (*p).to_owned(),
                resolve(p, home, env).map(|(_, source)| source),
            )
        })
        .collect()
}

/// Store a credential, replacing any existing one for that provider.
///
/// Replacing rather than merging: an entry holding a stale API key beside a
/// fresh token is one where which of them a run picks is decided by the order
/// of an `if`, and signing in should not leave the previous credential behind
/// to be found later.
pub fn save(home: &Path, provider: &str, credential: Credential) -> std::io::Result<PathBuf> {
    if credential.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the credential is empty",
        ));
    }
    std::fs::create_dir_all(home)?;

    let entry = match credential {
        Credential::Key(key) => Entry {
            api_key: Some(key),
            oauth: None,
        },
        Credential::OAuth(tokens) => Entry {
            api_key: None,
            oauth: Some(tokens),
        },
    };

    let path = auth_path(home);
    let mut store = read_store(&path).unwrap_or_default();
    store.version = AUTH_FORMAT_VERSION;
    store.providers.insert(provider.to_owned(), entry);

    write_store(&path, &store)?;
    Ok(path)
}

/// Remove a provider's stored credential. Returns whether there was one.
pub fn forget(home: &Path, provider: &str) -> std::io::Result<bool> {
    let path = auth_path(home);
    let Ok(mut store) = read_store(&path) else {
        return Ok(false);
    };
    let removed = store.providers.remove(provider).is_some();
    if removed {
        if store.providers.is_empty() {
            // Nothing left to protect; leaving an empty file implies otherwise.
            std::fs::remove_file(&path)?;
        } else {
            write_store(&path, &store)?;
        }
    }
    Ok(removed)
}

fn read_store(path: &Path) -> std::io::Result<Store> {
    let text = std::fs::read_to_string(path)?;
    serde_json::from_str(&text).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{} is not readable: {e}", path.display()),
        )
    })
}

/// Write the store, atomically and with the tightest permissions available.
///
/// Written to a sibling temp file and renamed, so an interrupted write cannot
/// leave a truncated credential file — losing a stored key to a crash is a
/// support ticket that starts with "it forgot my login".
fn write_store(path: &Path, store: &Store) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(store)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

    let temp = path.with_extension("json.tmp");
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);

        // The mode is set at creation. Creating world-readable and chmodding
        // afterwards leaves a window in which the credential is exposed.
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }

        let mut file = options.open(&temp)?;
        use std::io::Write;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
    }
    std::fs::rename(&temp, path)?;
    Ok(())
}

/// What can honestly be said about how the file is protected.
///
/// A caller shows this once, on save. Saying nothing would let a Windows user
/// reasonably assume the same protection a unix user gets.
pub fn protection_note() -> &'static str {
    #[cfg(unix)]
    {
        "the file is readable only by you (0600)"
    }
    #[cfg(not(unix))]
    {
        "no file protection is applied on this platform; prefer the environment \
         variable if others can read your profile"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Claiming file protection on a platform that has none is worse than
    /// claiming nothing, so the sentence differs by platform and both halves
    /// are asserted rather than only the mode.
    #[test]
    fn the_protection_note_promises_only_what_the_platform_delivers() {
        let note = protection_note();
        if cfg!(unix) {
            assert!(note.contains("0600"), "{note}");
        } else {
            assert!(!note.contains("0600"), "{note}");
            assert!(
                note.to_ascii_lowercase().contains("not")
                    || note.to_ascii_lowercase().contains("no "),
                "off unix it must not imply a guarantee: {note}"
            );
        }
    }

    fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn a_stored_credential_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        save(
            dir.path(),
            "anthropic",
            Credential::Key(Secret::new("sk-ant-stored")),
        )
        .unwrap();

        let (credential, source) = resolve("anthropic", dir.path(), &[]).unwrap();
        assert_eq!(credential.bearer().expose(), "sk-ant-stored");
        assert!(matches!(source, Source::Stored(_)));
    }

    #[test]
    fn the_environment_outranks_the_file() {
        // So one command can override without editing anything, and so CI
        // never silently picks up a developer's saved key.
        let dir = tempfile::tempdir().unwrap();
        save(
            dir.path(),
            "anthropic",
            Credential::Key(Secret::new("from-file")),
        )
        .unwrap();

        let (credential, source) = resolve(
            "anthropic",
            dir.path(),
            &env(&[("ANTHROPIC_API_KEY", "from-env")]),
        )
        .unwrap();
        assert_eq!(credential.bearer().expose(), "from-env");
        assert_eq!(source, Source::Env("ANTHROPIC_API_KEY".into()));
    }

    #[test]
    fn an_empty_environment_variable_does_not_shadow_a_stored_key() {
        let dir = tempfile::tempdir().unwrap();
        save(
            dir.path(),
            "anthropic",
            Credential::Key(Secret::new("from-file")),
        )
        .unwrap();
        let (credential, _) = resolve(
            "anthropic",
            dir.path(),
            &env(&[("ANTHROPIC_API_KEY", "  ")]),
        )
        .unwrap();
        assert_eq!(credential.bearer().expose(), "from-file");
    }

    #[test]
    fn providers_are_stored_independently() {
        let dir = tempfile::tempdir().unwrap();
        save(
            dir.path(),
            "anthropic",
            Credential::Key(Secret::new("a-key")),
        )
        .unwrap();
        save(dir.path(), "ollama", Credential::Key(Secret::new("o-key"))).unwrap();

        assert_eq!(
            resolve("anthropic", dir.path(), &[])
                .unwrap()
                .0
                .bearer()
                .expose(),
            "a-key"
        );
        assert_eq!(
            resolve("ollama", dir.path(), &[])
                .unwrap()
                .0
                .bearer()
                .expose(),
            "o-key"
        );
    }

    #[test]
    fn saving_twice_replaces_rather_than_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), "ollama", Credential::Key(Secret::new("first"))).unwrap();
        save(dir.path(), "ollama", Credential::Key(Secret::new("second"))).unwrap();
        assert_eq!(
            resolve("ollama", dir.path(), &[])
                .unwrap()
                .0
                .bearer()
                .expose(),
            "second"
        );
    }

    #[test]
    fn forgetting_one_provider_leaves_the_other() {
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), "anthropic", Credential::Key(Secret::new("a"))).unwrap();
        save(dir.path(), "ollama", Credential::Key(Secret::new("o"))).unwrap();

        assert!(forget(dir.path(), "anthropic").unwrap());
        assert!(resolve("anthropic", dir.path(), &[]).is_none());
        assert!(resolve("ollama", dir.path(), &[]).is_some());
    }

    #[test]
    fn forgetting_the_last_provider_removes_the_file() {
        // An empty credential file implies there is something to protect.
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), "ollama", Credential::Key(Secret::new("o"))).unwrap();
        assert!(forget(dir.path(), "ollama").unwrap());
        assert!(!auth_path(dir.path()).exists());
    }

    #[test]
    fn forgetting_what_was_never_stored_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!forget(dir.path(), "anthropic").unwrap());
    }

    #[test]
    fn an_empty_credential_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        assert!(save(dir.path(), "anthropic", Credential::Key(Secret::new("   "))).is_err());
        assert!(!auth_path(dir.path()).exists());
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_created_readable_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = save(
            dir.path(),
            "anthropic",
            Credential::Key(Secret::new("secret")),
        )
        .unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "credentials must not be group or world readable"
        );
    }

    #[test]
    fn a_secret_is_never_revealed_by_debug() {
        let secret = Secret::new("sk-ant-SUPERSECRET");
        assert!(!format!("{secret:?}").contains("SUPERSECRET"));
        // And not through a struct that contains one.
        let entry = Entry {
            api_key: Some(secret.clone()),
            oauth: None,
        };
        assert!(!format!("{entry:?}").contains("SUPERSECRET"));

        // Nor through the token pair, which has two of them and a hand-written
        // Debug that has to remember both.
        let tokens = OAuthTokens {
            access: Secret::new("access-SUPERSECRET"),
            refresh: Secret::new("refresh-ALSOSECRET"),
            expires_at_ms: 0,
            account_id: Some("acct_1".into()),
        };
        let dumped = format!("{tokens:?}");
        assert!(!dumped.contains("SUPERSECRET"), "{dumped}");
        assert!(!dumped.contains("ALSOSECRET"), "{dumped}");
        assert!(dumped.contains("acct_1"), "the account id is not a secret");
    }

    /// A version 1 file knew only `api_key`. Refusing to read one would log
    /// someone out of every provider to gain a field they may never use.
    #[test]
    fn a_version_one_file_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            auth_path(dir.path()),
            r#"{"version":1,"providers":{"anthropic":{"api_key":"sk-ant-old"}}}"#,
        )
        .unwrap();

        let (credential, _) = resolve("anthropic", dir.path(), &[]).expect("a credential");
        assert_eq!(credential.bearer().expose(), "sk-ant-old");
        assert_eq!(credential.kind(), "api key");
    }

    #[test]
    fn a_token_pair_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let tokens = OAuthTokens {
            access: Secret::new("at"),
            refresh: Secret::new("rt"),
            expires_at_ms: 1_700_000_000_000,
            account_id: Some("acct_9".into()),
        };
        save(
            dir.path(),
            "openai-codex",
            Credential::OAuth(tokens.clone()),
        )
        .unwrap();

        let (credential, _) = resolve("openai-codex", dir.path(), &[]).expect("a credential");
        assert_eq!(credential, Credential::OAuth(tokens));
        assert_eq!(credential.kind(), "oauth");
        assert_eq!(credential.bearer().expose(), "at");
    }

    /// Signing in must not leave the previous credential in the file for a
    /// later `if` to pick.
    #[test]
    fn storing_a_token_replaces_an_api_key_rather_than_joining_it() {
        let dir = tempfile::tempdir().unwrap();
        save(
            dir.path(),
            "openai-codex",
            Credential::Key(Secret::new("sk-old")),
        )
        .unwrap();
        save(
            dir.path(),
            "openai-codex",
            Credential::OAuth(OAuthTokens {
                access: Secret::new("at"),
                refresh: Secret::new("rt"),
                expires_at_ms: 1,
                account_id: None,
            }),
        )
        .unwrap();

        let text = std::fs::read_to_string(auth_path(dir.path())).unwrap();
        assert!(
            !text.contains("sk-old"),
            "the old key is still there: {text}"
        );
    }

    /// A token good for another two seconds is not good for a request that
    /// takes three.
    #[test]
    fn expiry_is_judged_with_a_margin() {
        let tokens = OAuthTokens {
            access: Secret::new("at"),
            refresh: Secret::new("rt"),
            expires_at_ms: 10_000,
            account_id: None,
        };
        assert!(!tokens.expired(5_000, 0));
        assert!(tokens.expired(9_500, 1_000), "the margin must count");
        assert!(tokens.expired(10_000, 0), "at the expiry it is gone");
        // A clock far enough ahead must not wrap into "fine".
        assert!(tokens.expired(u64::MAX, 60_000));
    }

    #[test]
    fn a_half_written_token_pair_is_not_a_credential() {
        let dir = tempfile::tempdir().unwrap();
        let empty_refresh = Credential::OAuth(OAuthTokens {
            access: Secret::new("at"),
            refresh: Secret::new("  "),
            expires_at_ms: 1,
            account_id: None,
        });
        assert!(empty_refresh.is_empty());
        assert!(save(dir.path(), "openai-codex", empty_refresh).is_err());
    }

    #[test]
    fn a_damaged_file_does_not_crash_the_lookup() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(auth_path(dir.path()), "{ not json").unwrap();
        assert!(resolve("anthropic", dir.path(), &[]).is_none());
    }

    #[test]
    fn status_reports_where_each_credential_came_from_without_returning_it() {
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), "ollama", Credential::Key(Secret::new("o"))).unwrap();
        let rows = status(
            &["anthropic", "ollama"],
            dir.path(),
            &env(&[("ANTHROPIC_API_KEY", "e")]),
        );
        assert_eq!(rows[0].1, Some(Source::Env("ANTHROPIC_API_KEY".into())));
        assert!(matches!(rows[1].1, Some(Source::Stored(_))));
    }

    #[test]
    fn no_temporary_file_is_left_behind() {
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), "anthropic", Credential::Key(Secret::new("a"))).unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }
}
