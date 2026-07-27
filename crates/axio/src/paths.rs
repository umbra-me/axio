//! Where things live, and which layer decided.
//!
//! Every path the binary uses is derived here rather than at its use site, so
//! `--doctor` and `config --explain` report the same paths the run uses.

use super::*;

pub(crate) fn resolve_config(cli: &Cli) -> Resolved {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let user = Some(config_file_path());
    // Bounded at the home directory so the walk cannot reach into an unrelated
    // parent and apply someone else's project settings.
    let project = config::find_project_config(&cwd, home_dir().as_deref());
    let env: Vec<(String, String)> = std::env::vars().collect();
    config::resolve(
        &Paths { user, project },
        &env,
        &Flags {
            model: cli.model.clone(),
            effort: None,
        },
    )
}

pub(crate) fn home_dir() -> Option<PathBuf> {
    use etcetera::BaseStrategy;
    etcetera::choose_base_strategy()
        .ok()
        .map(|s| s.home_dir().to_path_buf())
}

pub(crate) fn explain(resolved: &Resolved, key: &str) -> u8 {
    print_notices(resolved);
    match resolved.explain(key) {
        Some(layer) => {
            println!("{key} came from {}", layer.describe());
            0
        }
        None => {
            eprintln!("axio: no such configuration key: {key}");
            eprintln!("known keys:");
            for k in resolved.keys() {
                eprintln!("  {k}");
            }
            2
        }
    }
}

/// The user's configuration file. Always inside `axio_home`, so relocating
/// the home relocates everything axio owns rather than only half of it.
pub(crate) fn config_file_path() -> PathBuf {
    axio_home().join("config.toml")
}

/// axio's own directory: configuration, and the credential file.
pub(crate) fn axio_home() -> PathBuf {
    if let Some(explicit) = std::env::var_os("AXIO_HOME") {
        return PathBuf::from(explicit);
    }
    default_config_dir().unwrap_or_else(|| PathBuf::from(".axio"))
}

pub(crate) fn default_config_dir() -> Option<PathBuf> {
    use etcetera::BaseStrategy;
    etcetera::choose_base_strategy()
        .ok()
        .map(|s| s.config_dir().join("axio"))
}

/// Where axio keeps state that is not the user's to curate.
pub(crate) fn state_dir() -> PathBuf {
    if let Some(explicit) = std::env::var_os("AXIO_STATE") {
        return PathBuf::from(explicit);
    }
    std::env::temp_dir().join(format!("axio-{}", std::process::id()))
}

/// Everything the config loader complained about, on stderr.
///
/// The local modes return before any event stream exists, so a notice replayed
/// through `announce` never reaches them — which left `--doctor` and
/// `--explain`, whose whole job is explaining the configuration, as the two
/// surfaces that hid a rejected `[permissions] allow` or a discarded section.
pub(crate) fn print_notices(resolved: &Resolved) {
    for notice in resolved.notices() {
        eprintln!("axio: {}", notice.message);
    }
}
