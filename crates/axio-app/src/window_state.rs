//! What the window remembers about how it was looking at things.
//!
//! Layouts, card sizes, each terminal's view, and the open tabs are facts
//! about the window, not the work, and they are the window's to keep. They
//! were lost on every restart, which made every restart a tidy-up. They are
//! kept here, as one JSON file beside the settings, written whole whenever
//! they change and read once at start.
//!
//! The file is read leniently, field by field: a layout tree that will not
//! parse costs that pane its layout and nothing else. A persisted record is
//! the window's own past output, and the window's past self is allowed to
//! have been wrong about one thing without being wrong about everything.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::AppError;

/// The state as the webview holds it. Layouts and sizes are opaque here —
/// `serde_json::Value` — because their shape is the webview's business and
/// Rust only carries them across a restart.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(default, rename_all = "camelCase")]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct WindowState {
    /// By scope key (`group:<id>`, `project:<id>`): a layout tree, or null
    /// for the grid.
    #[ts(type = "Record<string, unknown>")]
    pub layouts: serde_json::Map<String, serde_json::Value>,
    /// By member id: a size preset name or `{w, h}`.
    #[ts(type = "Record<string, unknown>")]
    pub card_sizes: serde_json::Map<String, serde_json::Value>,
    /// By terminal id: `card`, `tui` or `plain`.
    #[ts(type = "Record<string, string>")]
    pub terminal_views: serde_json::Map<String, serde_json::Value>,
    /// The open tabs, as `kind:id` keys, in order, and which was in front.
    pub tabs: Vec<String>,
    pub active: Option<String>,
}

/// Where the file is.
#[derive(Debug, Clone)]
pub struct WindowStateFile {
    path: PathBuf,
}

impl WindowStateFile {
    pub fn in_home(home: &Path) -> Self {
        Self {
            path: home.join("window.json"),
        }
    }

    /// The remembered state, salvaged field by field. A missing file is the
    /// default; so is a file that is not an object at all, said on stderr.
    pub fn load(&self) -> WindowState {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return WindowState::default(),
            Err(e) => {
                eprintln!("axio-app: {} could not be read: {e}", self.path.display());
                return WindowState::default();
            }
        };
        let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(&text)
        else {
            eprintln!(
                "axio-app: {} is not the window's state",
                self.path.display()
            );
            return WindowState::default();
        };
        salvage(map)
    }

    pub fn save(&self, state: &WindowState) -> Result<(), AppError> {
        let text = serde_json::to_string_pretty(state)
            .map_err(|e| AppError::Supervisor(format!("window state could not be encoded: {e}")))?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                AppError::Supervisor(format!("{} could not be created: {e}", parent.display()))
            })?;
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, text)
            .and_then(|()| std::fs::rename(&tmp, &self.path))
            .map_err(|e| {
                AppError::Supervisor(format!("{} could not be written: {e}", self.path.display()))
            })
    }
}

/// Each field taken if it has the right shape, left default if not.
fn salvage(mut map: serde_json::Map<String, serde_json::Value>) -> WindowState {
    let mut out = WindowState::default();
    let object = |v: Option<serde_json::Value>| match v {
        Some(serde_json::Value::Object(m)) => m,
        _ => serde_json::Map::new(),
    };
    out.layouts = object(map.remove("layouts"));
    out.card_sizes = object(map.remove("cardSizes"));
    out.terminal_views = object(map.remove("terminalViews"))
        .into_iter()
        .filter(|(_, v)| {
            v.as_str()
                .is_some_and(|s| matches!(s, "card" | "tui" | "plain"))
        })
        .collect();
    out.tabs = match map.remove("tabs") {
        Some(serde_json::Value::Array(items)) => items
            .into_iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    };
    out.active = map
        .remove("active")
        .and_then(|v| v.as_str().map(str::to_owned));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bad_field_costs_only_itself() {
        let dir = tempfile::tempdir().unwrap();
        let file = WindowStateFile::in_home(dir.path());
        std::fs::write(
            dir.path().join("window.json"),
            r#"{"layouts": "not an object", "cardSizes": {"a": "wide"},
                "terminalViews": {"h1": "card", "h2": "sideways"},
                "tabs": ["session:x", 7, "terminal:h1"], "active": "terminal:h1"}"#,
        )
        .unwrap();
        let state = file.load();
        assert!(state.layouts.is_empty());
        assert_eq!(
            state.card_sizes.get("a").and_then(|v| v.as_str()),
            Some("wide")
        );
        assert_eq!(state.terminal_views.len(), 1);
        assert_eq!(state.tabs, vec!["session:x", "terminal:h1"]);
        assert_eq!(state.active.as_deref(), Some("terminal:h1"));

        // Written and read back whole.
        file.save(&state).unwrap();
        assert_eq!(file.load(), state);
        // Missing is the default, not an error.
        assert_eq!(
            WindowStateFile::in_home(&dir.path().join("nope")).load(),
            WindowState::default()
        );
    }
}
