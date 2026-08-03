//! Path resolution that reads from an injected environment rather than the process's.
//!
//! Every probe takes its environment as a map so tests can point `USERPROFILE` at a temp
//! directory and exercise the real credential-loading code against fixture files. Reading
//! `std::env` directly inside a probe would make that impossible.

use std::collections::HashMap;
use std::path::PathBuf;

pub type Env = HashMap<String, String>;

/// Snapshot of the real process environment, for production callers.
pub fn current_env() -> Env {
    std::env::vars().collect()
}

/// A non-empty environment value. Providers treat `VAR=""` as "unset, use the default",
/// which is not what a plain `get` gives you.
pub fn non_empty<'a>(env: &'a Env, key: &str) -> Option<&'a str> {
    env.get(key).map(String::as_str).filter(|v| !v.is_empty())
}

/// The user's home directory.
///
/// `USERPROFILE` first because that is the Windows-native answer; `HOME` second because
/// Git Bash, WSL interop, and CI containers set it and users expect it to win over nothing.
pub fn home_dir(env: &Env) -> PathBuf {
    if let Some(profile) = non_empty(env, "USERPROFILE") {
        return PathBuf::from(profile);
    }
    if let Some(home) = non_empty(env, "HOME") {
        return PathBuf::from(home);
    }
    PathBuf::from(".")
}

/// Roaming application data — `%APPDATA%`, where per-user config belongs on Windows.
///
/// Roaming (not `LOCALAPPDATA`) because provider toggles and API keys are settings that
/// should follow a user to another machine in a domain environment.
pub fn app_data_dir(env: &Env) -> PathBuf {
    if let Some(app_data) = non_empty(env, "APPDATA") {
        return PathBuf::from(app_data);
    }
    home_dir(env).join("AppData").join("Roaming")
}

/// Machine-local application data — `%LOCALAPPDATA%`.
///
/// The counterpart to [`app_data_dir`], and the difference matters for anything large.
/// Settings roam with a user because they are worth carrying to another machine; a cached
/// scan of *this* machine's transcripts is tens of megabytes of rebuildable data that
/// describes files the other machine does not have.
pub fn local_data_dir(env: &Env) -> PathBuf {
    if let Some(local) = non_empty(env, "LOCALAPPDATA") {
        return PathBuf::from(local);
    }
    home_dir(env).join("AppData").join("Local")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(pairs: &[(&str, &str)]) -> Env {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn userprofile_wins_over_home() {
        let env = env_of(&[("USERPROFILE", r"C:\Users\ada"), ("HOME", "/home/ada")]);
        assert_eq!(home_dir(&env), PathBuf::from(r"C:\Users\ada"));
    }

    #[test]
    fn empty_values_fall_through_to_the_next_source() {
        let env = env_of(&[("USERPROFILE", ""), ("HOME", "/home/ada")]);
        assert_eq!(home_dir(&env), PathBuf::from("/home/ada"));
    }

    #[test]
    fn app_data_falls_back_to_the_conventional_location() {
        let env = env_of(&[("USERPROFILE", r"C:\Users\ada")]);
        assert_eq!(
            app_data_dir(&env),
            PathBuf::from(r"C:\Users\ada")
                .join("AppData")
                .join("Roaming")
        );
    }
}
