//! Plugin folders: each subfolder of a plugins directory, named by its
//! `.claude-plugin/plugin.json` or its folder name.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct Plugin {
    pub name: String,
    pub version: Option<String>,
    pub root: PathBuf,
}

/// Plugins under `dir`, keeping only those `allowed` names (all when `None`
/// or when the list holds `*`).
pub fn list(dir: &Path, allowed: Option<&[String]>) -> Vec<Plugin> {
    let mut roots: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    roots.sort();
    roots
        .into_iter()
        .map(|root| {
            let manifest = std::fs::read_to_string(root.join(".claude-plugin/plugin.json"))
                .ok()
                .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok());
            let field = |k: &str| {
                manifest
                    .as_ref()
                    .and_then(|m| m[k].as_str().map(String::from))
            };
            Plugin {
                name: field("name").unwrap_or_else(|| {
                    root.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into()
                }),
                version: field("version"),
                root,
            }
        })
        .filter(|p| allowed.is_none_or(|a| a.iter().any(|n| n == "*" || *n == p.name)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_plugins_and_filters_them() {
        let dir = std::env::temp_dir().join(format!("axio-plugins-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("guard-dir/.claude-plugin")).unwrap();
        std::fs::write(
            dir.join("guard-dir/.claude-plugin/plugin.json"),
            r#"{"name": "guard", "version": "0.1.0"}"#,
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("plain")).unwrap();
        let all = list(&dir, None);
        assert_eq!(
            all.iter()
                .map(|p| (p.name.as_str(), p.version.as_deref()))
                .collect::<Vec<_>>(),
            [("guard", Some("0.1.0")), ("plain", None)]
        );
        assert_eq!(list(&dir, Some(&["plain".into()])).len(), 1);
        assert_eq!(list(&dir, Some(&["*".into()])).len(), 2);
        assert!(list(&dir, Some(&[])).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
