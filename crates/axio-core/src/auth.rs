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

/// Bumped if the on-disk shape changes incompatibly.
pub const AUTH_FORMAT_VERSION: u32 = 1;

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

/// The environment variable a provider reads, if it has one.
pub fn env_var_for(provider: &str) -> Option<&'static str> {
    match provider {
        "anthropic" => Some("ANTHROPIC_API_KEY"),
        "ollama" | "openai-compatible" => Some("OLLAMA_API_KEY"),
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Store {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    providers: BTreeMap<String, Entry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    api_key: Secret,
}

pub fn auth_path(home: &Path) -> PathBuf {
    home.join(FILE_NAME)
}

/// Resolve a provider's credential, environment first.
pub fn resolve(provider: &str, home: &Path, env: &[(String, String)]) -> Option<(Secret, Source)> {
    if let Some(var) = env_var_for(provider)
        && let Some((_, value)) = env.iter().find(|(k, _)| k == var)
        && !value.trim().is_empty()
    {
        return Some((Secret::new(value.clone()), Source::Env(var.to_owned())));
    }

    let path = auth_path(home);
    let store = read_store(&path).ok()?;
    let entry = store.providers.get(provider)?;
    if entry.api_key.is_empty() {
        return None;
    }
    Some((entry.api_key.clone(), Source::Stored(path)))
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
pub fn save(home: &Path, provider: &str, key: Secret) -> std::io::Result<PathBuf> {
    if key.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the credential is empty",
        ));
    }
    std::fs::create_dir_all(home)?;

    let path = auth_path(home);
    let mut store = read_store(&path).unwrap_or_default();
    store.version = AUTH_FORMAT_VERSION;
    store
        .providers
        .insert(provider.to_owned(), Entry { api_key: key });

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

    fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn a_stored_credential_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), "anthropic", Secret::new("sk-ant-stored")).unwrap();

        let (secret, source) = resolve("anthropic", dir.path(), &[]).unwrap();
        assert_eq!(secret.expose(), "sk-ant-stored");
        assert!(matches!(source, Source::Stored(_)));
    }

    #[test]
    fn the_environment_outranks_the_file() {
        // So one command can override without editing anything, and so CI
        // never silently picks up a developer's saved key.
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), "anthropic", Secret::new("from-file")).unwrap();

        let (secret, source) = resolve(
            "anthropic",
            dir.path(),
            &env(&[("ANTHROPIC_API_KEY", "from-env")]),
        )
        .unwrap();
        assert_eq!(secret.expose(), "from-env");
        assert_eq!(source, Source::Env("ANTHROPIC_API_KEY".into()));
    }

    #[test]
    fn an_empty_environment_variable_does_not_shadow_a_stored_key() {
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), "anthropic", Secret::new("from-file")).unwrap();
        let (secret, _) = resolve(
            "anthropic",
            dir.path(),
            &env(&[("ANTHROPIC_API_KEY", "  ")]),
        )
        .unwrap();
        assert_eq!(secret.expose(), "from-file");
    }

    #[test]
    fn providers_are_stored_independently() {
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), "anthropic", Secret::new("a-key")).unwrap();
        save(dir.path(), "ollama", Secret::new("o-key")).unwrap();

        assert_eq!(
            resolve("anthropic", dir.path(), &[]).unwrap().0.expose(),
            "a-key"
        );
        assert_eq!(
            resolve("ollama", dir.path(), &[]).unwrap().0.expose(),
            "o-key"
        );
    }

    #[test]
    fn saving_twice_replaces_rather_than_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), "ollama", Secret::new("first")).unwrap();
        save(dir.path(), "ollama", Secret::new("second")).unwrap();
        assert_eq!(
            resolve("ollama", dir.path(), &[]).unwrap().0.expose(),
            "second"
        );
    }

    #[test]
    fn forgetting_one_provider_leaves_the_other() {
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), "anthropic", Secret::new("a")).unwrap();
        save(dir.path(), "ollama", Secret::new("o")).unwrap();

        assert!(forget(dir.path(), "anthropic").unwrap());
        assert!(resolve("anthropic", dir.path(), &[]).is_none());
        assert!(resolve("ollama", dir.path(), &[]).is_some());
    }

    #[test]
    fn forgetting_the_last_provider_removes_the_file() {
        // An empty credential file implies there is something to protect.
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), "ollama", Secret::new("o")).unwrap();
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
        assert!(save(dir.path(), "anthropic", Secret::new("   ")).is_err());
        assert!(!auth_path(dir.path()).exists());
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_created_readable_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = save(dir.path(), "anthropic", Secret::new("secret")).unwrap();
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
            api_key: secret.clone(),
        };
        assert!(!format!("{entry:?}").contains("SUPERSECRET"));
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
        save(dir.path(), "ollama", Secret::new("o")).unwrap();
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
        save(dir.path(), "anthropic", Secret::new("a")).unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }
}
