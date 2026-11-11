//! Profiles: named starting points that pick modules, plugins and defaults.
//! `minimal` and `standard` are built in; `~/.axio/agent/profiles/<name>.json`
//! adds or adjusts others.

use crate::tools::Modules;
use serde_json::Value;
use std::path::Path;

pub const DEFAULT: &str = "standard";

#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    pub name: String,
    pub modules: Modules,
    /// Plugin names to load; `None` loads all of them.
    pub plugins: Option<Vec<String>>,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// Fraction of the context window at which compaction starts.
    pub compact_at: f64,
}

fn builtin(name: &str) -> Option<Profile> {
    let modules = match name {
        "minimal" => Modules::default(),
        "standard" => Modules {
            search: true,
            plan: true,
            background: true,
            ..Modules::default()
        },
        "full" => Modules {
            search: true,
            plan: true,
            background: true,
            web: true,
            sandbox: true,
        },
        _ => return None,
    };
    Some(Profile {
        name: name.into(),
        modules,
        plugins: None,
        model: None,
        effort: None,
        compact_at: 0.85,
    })
}

/// The profile a program name selects: `axio-full` or `ax-full` picks
/// `full`; `axio` and `ax` pick none, leaving the configured default.
pub fn from_program(argv0: &str) -> Option<String> {
    let base = Path::new(argv0).file_name()?.to_str()?;
    let base = base.strip_suffix(".exe").unwrap_or(base);
    let rest = base
        .strip_prefix("axio-")
        .or_else(|| base.strip_prefix("ax-"))?;
    (!rest.is_empty()).then(|| rest.to_string())
}

/// `min` is short for `minimal`.
pub fn canonical(name: &str) -> &str {
    if name == "min" { "minimal" } else { name }
}

/// Makes `name` the default in `~/.axio/agent/settings.json`, keeping the
/// file's other settings.
pub fn set_default(home: &Path, name: &str) -> Result<String, String> {
    let name = canonical(name);
    load(home, name)?;
    crate::settings::set(home, "profile", Value::String(name.into()))?;
    Ok(name.to_string())
}

pub fn load(home: &Path, name: &str) -> Result<Profile, String> {
    let name = canonical(name);
    let path = home.join("profiles").join(format!("{name}.json"));
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return builtin(name).ok_or_else(|| {
                format!(
                    "unknown profile {name}: built in are minimal, standard and full, or add {}",
                    path.display()
                )
            });
        }
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let doc: Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let base = doc["extends"].as_str().unwrap_or(DEFAULT);
    let mut p =
        builtin(base).ok_or_else(|| format!("{}: unknown base profile {base}", path.display()))?;
    p.name = name.into();
    if let Some(list) = doc["modules"].as_array() {
        let names: Vec<&str> = list.iter().filter_map(Value::as_str).collect();
        p.modules = Modules::from_names(&names).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    if let Some(list) = doc["plugins"].as_array() {
        p.plugins = Some(
            list.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect(),
        );
    }
    p.model = doc["model"].as_str().map(String::from).or(p.model);
    p.effort = doc["effort"].as_str().map(String::from).or(p.effort);
    if let Some(f) = doc["compactAt"].as_f64() {
        if !(0.1..=1.0).contains(&f) {
            return Err(format!(
                "{}: compactAt must be between 0.1 and 1",
                path.display()
            ));
        }
        p.compact_at = f;
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn program_names_pick_profiles() {
        assert_eq!(from_program("/usr/local/bin/axio"), None);
        assert_eq!(from_program("ax"), None);
        assert_eq!(from_program("/x/axio-full").as_deref(), Some("full"));
        assert_eq!(from_program("ax-min").as_deref(), Some("min"));
        assert_eq!(from_program("axio-lean.exe").as_deref(), Some("lean"));
        assert_eq!(from_program("axio-"), None);
        assert_eq!(from_program("taxi"), None);
    }

    #[test]
    fn sets_the_default_and_keeps_other_settings() {
        let home =
            std::env::temp_dir().join(format!("axio-profile-default-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(
            home.join("settings.json"),
            r#"{"model": "anthropic/x", "profile": "full"}"#,
        )
        .unwrap();
        assert_eq!(set_default(&home, "min").unwrap(), "minimal");
        let doc: Value =
            serde_json::from_str(&std::fs::read_to_string(home.join("settings.json")).unwrap())
                .unwrap();
        assert_eq!(
            (doc["profile"].as_str(), doc["model"].as_str()),
            (Some("minimal"), Some("anthropic/x"))
        );
        assert!(set_default(&home, "nope").is_err());
        assert_eq!(load(&home, "min").unwrap().name, "minimal");
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn builtins_and_custom_profiles() {
        let home = std::env::temp_dir().join(format!("axio-profile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        assert_eq!(load(&home, "minimal").unwrap().modules, Modules::default());
        assert!(load(&home, "standard").unwrap().modules.search);
        assert!(
            load(&home, "nope")
                .unwrap_err()
                .starts_with("unknown profile nope")
        );
        std::fs::create_dir_all(home.join("profiles")).unwrap();
        std::fs::write(
            home.join("profiles/lean.json"),
            r#"{"extends": "minimal", "modules": ["plan"], "plugins": ["guard"], "model": "openai/x", "compactAt": 0.6}"#,
        )
        .unwrap();
        let p = load(&home, "lean").unwrap();
        assert_eq!(
            p.modules,
            Modules {
                plan: true,
                ..Modules::default()
            }
        );
        assert_eq!(p.plugins, Some(vec!["guard".to_string()]));
        assert_eq!((p.model.as_deref(), p.compact_at), (Some("openai/x"), 0.6));
        std::fs::write(
            home.join("profiles/bad.json"),
            r#"{"modules": ["teleport"]}"#,
        )
        .unwrap();
        assert!(
            load(&home, "bad")
                .unwrap_err()
                .contains("unknown module teleport")
        );
        std::fs::remove_dir_all(home).unwrap();
    }
}
