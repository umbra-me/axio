//! Settings from `~/.axio/agent/settings.json`, overlaid by a trusted
//! project's `.axio/settings.json`, and the record of which projects are
//! trusted to supply configuration.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub model: Option<String>,
    pub effort: Option<String>,
    pub profile: Option<String>,
    pub import_from_other_harnesses: Option<String>,
    /// Lets sandboxed commands reach the network.
    pub sandbox_network: Option<bool>,
    /// The model compaction runs on: `fast`, `inherit` or a model.
    pub compact_model: Option<String>,
    /// Fast mode by default, on models that have it.
    pub fast_mode: Option<bool>,
    #[serde(default)]
    pub status_line: StatusLine,
    #[serde(default)]
    pub permissions: PermissionSettings,
    #[serde(default)]
    pub subagents: Subagents,
}

/// `"permissions"` in settings.json, as in Claude Code.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionSettings {
    pub default_mode: Option<String>,
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub deny: Vec<String>,
}

/// `"statusLine"` in settings.json: which items each status line shows;
/// unset means all of them, an empty list hides the line.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StatusLine {
    pub under_prompt: Option<Vec<String>>,
    pub after_turn: Option<Vec<String>>,
}

/// `"subagents"` in settings.json.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Subagents {
    /// Levels below the main agent that may start sub-agents (0 by default).
    pub nesting: Option<u32>,
    /// Sub-agents that may run at once (4 by default).
    pub parallel: Option<usize>,
    /// The model a role's `fast` means.
    pub fast_model: Option<String>,
    /// Fast mode for sub-agents on models that have it (off by default).
    pub fast: Option<bool>,
    /// Models the main agent may pick for roles set to `choose`.
    pub choices: Option<Vec<String>>,
    /// Characters of a sub-agent's answer returned to the main agent.
    pub result_limit: Option<usize>,
    /// Per-role changes that win over the role file.
    #[serde(default)]
    pub roles: std::collections::BTreeMap<String, RoleSettings>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RoleSettings {
    pub model: Option<String>,
    pub effort: Option<String>,
    pub context: Option<String>,
    pub max_turns: Option<usize>,
    pub sandbox: Option<String>,
}

impl Subagents {
    /// Fields set in `over` win; roles merge field by field.
    fn overlay(mut self, over: Subagents) -> Subagents {
        for (name, o) in over.roles {
            let r = self.roles.entry(name).or_default();
            r.model = o.model.or(r.model.take());
            r.effort = o.effort.or(r.effort.take());
            r.context = o.context.or(r.context.take());
            r.max_turns = o.max_turns.or(r.max_turns);
            r.sandbox = o.sandbox.or(r.sandbox.take());
        }
        Subagents {
            nesting: over.nesting.or(self.nesting),
            parallel: over.parallel.or(self.parallel),
            fast_model: over.fast_model.or(self.fast_model),
            fast: over.fast.or(self.fast),
            choices: over.choices.or(self.choices),
            result_limit: over.result_limit.or(self.result_limit),
            roles: self.roles,
        }
    }
}

impl Settings {
    fn read(path: &Path) -> Result<Settings, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// Fields set in `over` win.
    fn overlay(self, over: Settings) -> Settings {
        Settings {
            model: over.model.or(self.model),
            effort: over.effort.or(self.effort),
            profile: over.profile.or(self.profile),
            import_from_other_harnesses: over
                .import_from_other_harnesses
                .or(self.import_from_other_harnesses),
            sandbox_network: over.sandbox_network.or(self.sandbox_network),
            compact_model: over.compact_model.or(self.compact_model),
            fast_mode: over.fast_mode.or(self.fast_mode),
            permissions: PermissionSettings {
                default_mode: over
                    .permissions
                    .default_mode
                    .or(self.permissions.default_mode),
                // Rules from every scope apply together.
                allow: [self.permissions.allow, over.permissions.allow].concat(),
                deny: [self.permissions.deny, over.permissions.deny].concat(),
            },
            status_line: StatusLine {
                under_prompt: over
                    .status_line
                    .under_prompt
                    .or(self.status_line.under_prompt),
                after_turn: over.status_line.after_turn.or(self.status_line.after_turn),
            },
            subagents: self.subagents.overlay(over.subagents),
        }
    }

    pub fn load(home: &Path, project: Option<&Path>) -> Result<Settings, String> {
        let global = Settings::read(&home.join("settings.json"))?;
        Ok(match project {
            Some(root) => global.overlay(Settings::read(&root.join(".axio/settings.json"))?),
            None => global,
        })
    }
}

/// Sets one key in `home/settings.json`, keeping the others.
pub fn set(home: &Path, key: &str, value: serde_json::Value) -> Result<(), String> {
    let path = home.join("settings.json");
    let mut doc: serde_json::Map<String, serde_json::Value> = match std::fs::read_to_string(&path) {
        Ok(t) => serde_json::from_str(&t).map_err(|e| format!("{}: {e}", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::Map::new(),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    doc.insert(key.into(), value);
    std::fs::create_dir_all(home).map_err(|e| e.to_string())?;
    std::fs::write(&path, format!("{:#}\n", serde_json::Value::Object(doc)))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Sets a possibly nested key, such as `["subagents", "parallel"]`, in
/// `home/settings.json`, keeping everything else.
pub fn set_path(home: &Path, path: &[&str], value: serde_json::Value) -> Result<(), String> {
    let Some((first, rest)) = path.split_first() else {
        return Err("no setting named".into());
    };
    if rest.is_empty() {
        return set(home, first, value);
    }
    let file = home.join("settings.json");
    let doc: serde_json::Value = match std::fs::read_to_string(&file) {
        Ok(t) => serde_json::from_str(&t).map_err(|e| format!("{}: {e}", file.display()))?,
        Err(_) => serde_json::json!({}),
    };
    let mut section = doc
        .get(*first)
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let mut at = &mut section;
    for key in &rest[..rest.len() - 1] {
        if !at[*key].is_object() {
            at[*key] = serde_json::json!({});
        }
        at = at.get_mut(*key).expect("just set");
    }
    at[rest[rest.len() - 1]] = value;
    set(home, first, section)
}

/// The nearest ancestor holding `.git` or `.axio`, else `cwd` itself.
pub fn project_root(cwd: &Path) -> PathBuf {
    cwd.ancestors()
        .find(|d| d.join(".git").exists() || d.join(".axio").is_dir())
        .unwrap_or(cwd)
        .to_path_buf()
}

/// Files in a project that only load once the project is trusted, because
/// they can make axio run programs.
pub fn project_config(root: &Path) -> Vec<PathBuf> {
    [
        ".axio/settings.json",
        ".axio/plugins",
        ".axio/agents",
        ".mcp.json",
    ]
    .iter()
    .map(|p| root.join(p))
    .filter(|p| p.exists())
    .collect()
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Trust {
    #[serde(default)]
    pub trusted: Vec<PathBuf>,
    #[serde(default)]
    pub declined: Vec<PathBuf>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    Trusted,
    Declined,
    Unknown,
}

impl Trust {
    pub fn load(home: &Path) -> Trust {
        std::fs::read_to_string(home.join("trust.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn decision(&self, root: &Path) -> Decision {
        if self.trusted.iter().any(|p| p == root) {
            Decision::Trusted
        } else if self.declined.iter().any(|p| p == root) {
            Decision::Declined
        } else {
            Decision::Unknown
        }
    }

    pub fn record(&mut self, home: &Path, root: &Path, trusted: bool) -> Result<(), String> {
        self.trusted.retain(|p| p != root);
        self.declined.retain(|p| p != root);
        if trusted {
            self.trusted.push(root.to_path_buf());
        } else {
            self.declined.push(root.to_path_buf());
        }
        std::fs::create_dir_all(home).map_err(|e| format!("{}: {e}", home.display()))?;
        let path = home.join("trust.json");
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, text + "\n").map_err(|e| format!("{}: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_path_writes_nested_keys_and_keeps_the_rest() {
        let home = std::env::temp_dir().join(format!("axio-set-path-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(
            home.join("settings.json"),
            r#"{"effort": "high", "subagents": {"nesting": 1}}"#,
        )
        .unwrap();
        set_path(&home, &["subagents", "parallel"], serde_json::json!(2)).unwrap();
        set_path(&home, &["fastMode"], serde_json::json!(true)).unwrap();
        let s = Settings::load(&home, None).unwrap();
        assert_eq!(
            (s.subagents.parallel, s.subagents.nesting),
            (Some(2), Some(1))
        );
        assert_eq!(
            (s.effort.as_deref(), s.fast_mode),
            (Some("high"), Some(true))
        );
    }

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("axio-settings-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn project_settings_overlay_global_ones() {
        let home = tmp("home");
        let root = tmp("proj");
        std::fs::write(
            home.join("settings.json"),
            r#"{"model": "anthropic/a", "effort": "high", "unknownKey": 1}"#,
        )
        .unwrap();
        std::fs::create_dir_all(root.join(".axio")).unwrap();
        std::fs::write(root.join(".axio/settings.json"), r#"{"model": "openai/b"}"#).unwrap();
        let s = Settings::load(&home, Some(&root)).unwrap();
        assert_eq!(
            (s.model.as_deref(), s.effort.as_deref()),
            (Some("openai/b"), Some("high"))
        );
        let s = Settings::load(&home, None).unwrap();
        assert_eq!(s.model.as_deref(), Some("anthropic/a"));
        assert_eq!(project_config(&root), [root.join(".axio/settings.json")]);
        std::fs::write(home.join("settings.json"), "{").unwrap();
        assert!(
            Settings::load(&home, None).is_err(),
            "a broken settings file is reported"
        );
        for d in [home, root] {
            std::fs::remove_dir_all(d).unwrap();
        }
    }

    #[test]
    fn finds_the_project_root_above_cwd() {
        let root = tmp("root");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        assert_eq!(project_root(&root.join("a/b")), root);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn remembers_trust_decisions() {
        let home = tmp("trust");
        let mut t = Trust::load(&home);
        let p = Path::new("/work/p");
        assert_eq!(t.decision(p), Decision::Unknown);
        t.record(&home, p, false).unwrap();
        assert_eq!(Trust::load(&home).decision(p), Decision::Declined);
        t.record(&home, p, true).unwrap();
        let reloaded = Trust::load(&home);
        assert_eq!(reloaded.decision(p), Decision::Trusted);
        assert!(reloaded.declined.is_empty());
        std::fs::remove_dir_all(home).unwrap();
    }
}
