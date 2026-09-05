//! What the window remembers about itself, and where.
//!
//! Two files, two owners. `~/.axio/config.toml` is axio's — the model, the
//! budget, the permission rules — and is shared with the command line, so the
//! window edits it through `axio_core::config::edit`, which touches the keys it
//! is asked to and leaves every other byte alone. `~/.axio/app.toml` is this
//! window's alone: fonts, sizes, and what each hosted agent is started with.
//! Nothing in the command line reads it, so the window may rewrite it whole.
//!
//! Rust owns both. The webview is handed a view and sends back a whole
//! settings value; it never persists a byte itself, which is what keeps a
//! restart from being a negotiation between two stores.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::AppError;

/// Everything under the window's control, as one value.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(default, rename_all = "camelCase")]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct AppSettings {
    pub appearance: Appearance,
    pub terminal: TerminalSettings,
    /// What "open in editor" runs, with the path appended. Empty means the
    /// platform's default opener.
    pub editor: String,
    /// Keyed by harness executable — `claude`, `codex`, `pi`, `axio`.
    pub agents: BTreeMap<String, AgentSettings>,
}

/// The interface's own type and density. Empty font means the stylesheet's
/// stack; the scale multiplies every size token.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(default, rename_all = "camelCase")]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct Appearance {
    pub ui_font: String,
    pub ui_scale: f64,
    /// `comfortable` or `compact`. Row heights and paddings, not type.
    pub density: String,
    /// `dark`, `light` or `system`. The glass is tinted either way; light
    /// inverts the slate the content sits on.
    pub theme: String,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            ui_font: String::new(),
            ui_scale: 1.0,
            density: "comfortable".to_owned(),
            theme: "dark".to_owned(),
        }
    }
}

/// How hosted terminals render. Empty font means the stylesheet's mono stack.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(default, rename_all = "camelCase")]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct TerminalSettings {
    pub font: String,
    pub font_size: u32,
    /// 1.0 by default: block-drawing glyphs in a Nerd Font stop being
    /// contiguous at anything else, and provider interfaces are full of them.
    pub line_height: f64,
    pub scrollback: u32,
    /// How a terminal opened in its own tab is shown until it is told
    /// otherwise: `card` (the group card: state, a typed follow-up, the
    /// branch), `tui` (the real terminal under that card's header), or
    /// `plain` (the terminal edge to edge, no chrome). Each terminal's own
    /// choice, made from its header menu, overrides this for that terminal.
    pub view: String,
    /// Start every remembered terminal again when the window opens, each in
    /// its own worktree with its tool asked to continue. Off, they are
    /// listed as ended until resumed one by one.
    pub resume_on_launch: bool,
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self {
            font: String::new(),
            font_size: 13,
            line_height: 1.0,
            scrollback: 10_000,
            view: "card".to_owned(),
            resume_on_launch: true,
        }
    }
}

/// What one hosted agent is started with.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(default, rename_all = "camelCase")]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct AgentSettings {
    /// Extra arguments, split the way a shell would. Appended to whatever a
    /// launch asks for.
    pub args: String,
    /// Named Axio Local profile; used only for hosted Axio terminals.
    pub local_profile: String,
}

/// The default model, from axio's own configuration file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct ModelDefault {
    pub provider: String,
    pub name: String,
}

/// What the settings surface is shown.
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct SettingsView {
    pub settings: AppSettings,
    pub model: ModelDefault,
    /// Every provider name axio knows, for a picker. Whether each has a
    /// credential is not known here; the session says so when it starts.
    pub providers: Vec<String>,
    /// Where each lives, so a person can find the file the window is editing.
    pub app_path: String,
    pub config_path: String,
}

/// The two files, by path. Built once, from the axio home.
#[derive(Debug, Clone)]
pub struct Settings {
    app: PathBuf,
    config: PathBuf,
}

impl Settings {
    pub fn in_home(home: &Path) -> Self {
        Self {
            app: home.join("app.toml"),
            config: home.join("config.toml"),
        }
    }

    /// The directory both files live in.
    pub fn home(&self) -> PathBuf {
        self.app.parent().map(Path::to_path_buf).unwrap_or_default()
    }

    /// Where the window keeps the terminals it hosts, beside its settings —
    /// its own record, since no other surface has hosted terminals to list.
    pub fn terminals(&self) -> PathBuf {
        self.app.with_file_name("terminals.json")
    }

    /// The window's own settings. A missing file is the defaults; a file that
    /// will not parse is an error, so a typo made by hand is reported rather
    /// than silently replaced by defaults on the next save.
    pub fn load(&self) -> Result<AppSettings, AppError> {
        match std::fs::read_to_string(&self.app) {
            Ok(text) => toml::from_str(&text).map_err(|e| {
                AppError::Supervisor(format!("{} could not be read: {e}", self.app.display()))
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(AppSettings::default()),
            Err(e) => Err(AppError::Supervisor(format!(
                "{} could not be read: {e}",
                self.app.display()
            ))),
        }
    }

    /// Write the window's settings, whole. This file has one author.
    pub fn save(&self, settings: &AppSettings) -> Result<(), AppError> {
        let text = toml::to_string_pretty(settings)
            .map_err(|e| AppError::Supervisor(format!("settings could not be encoded: {e}")))?;
        if let Some(parent) = self.app.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                AppError::Supervisor(format!("{} could not be created: {e}", parent.display()))
            })?;
        }
        std::fs::write(&self.app, text).map_err(|e| {
            AppError::Supervisor(format!("{} could not be written: {e}", self.app.display()))
        })
    }

    /// The default model as `config.toml` states it, or axio's built-in
    /// default where the file is silent. Read from the file rather than from a
    /// resolved config, because a resolved config folds in environment and
    /// flags — and what the settings surface edits is the file.
    pub fn model(&self) -> ModelDefault {
        let built_in = axio_core::config::ModelSection::default();
        let text = std::fs::read_to_string(&self.config).unwrap_or_default();
        let table: toml::Table = toml::from_str(&text).unwrap_or_default();
        let model = table.get("model").and_then(toml::Value::as_table);
        let pick = |key: &str, fallback: String| {
            model
                .and_then(|m| m.get(key))
                .and_then(toml::Value::as_str)
                .map(str::to_owned)
                .unwrap_or(fallback)
        };
        ModelDefault {
            provider: pick("provider", built_in.provider),
            name: pick("name", built_in.name),
        }
    }

    /// Set the default model in `config.toml`, preserving everything else in
    /// it. The same edit `/model` makes from the terminal.
    pub fn set_model(&self, model: &ModelDefault) -> Result<(), AppError> {
        let provider = model.provider.trim();
        let name = model.name.trim();
        if provider.is_empty() || name.is_empty() {
            return Err(AppError::NoRepository(
                "a default model needs both a provider and a name".to_owned(),
            ));
        }
        let text = std::fs::read_to_string(&self.config).unwrap_or_default();
        let edited = axio_core::config::edit::set_model(&text, provider, name);
        if let Some(parent) = self.config.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                AppError::Supervisor(format!("{} could not be created: {e}", parent.display()))
            })?;
        }
        std::fs::write(&self.config, edited).map_err(|e| {
            AppError::Supervisor(format!(
                "{} could not be written: {e}",
                self.config.display()
            ))
        })
    }

    pub fn view(&self) -> Result<SettingsView, AppError> {
        Ok(SettingsView {
            settings: self.load()?,
            model: self.model(),
            providers: axio_core::auth::PROVIDERS
                .iter()
                .map(|p| p.to_string())
                .collect(),
            app_path: self.app.display().to_string(),
            config_path: self.config.display().to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_is_the_defaults_and_a_save_round_trips() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let settings = Settings::in_home(dir.path());
        assert_eq!(settings.load().expect("loaded"), AppSettings::default());

        let mut wanted = AppSettings::default();
        wanted.appearance.ui_scale = 1.15;
        wanted.terminal.font = "Iosevka".into();
        wanted.agents.insert(
            "claude".into(),
            AgentSettings {
                args: "--verbose".into(),
                ..Default::default()
            },
        );
        settings.save(&wanted).expect("saved");
        assert_eq!(settings.load().expect("loaded"), wanted);
    }

    /// A file somebody edited by hand and broke must be reported, not replaced.
    #[test]
    fn a_broken_file_is_an_error_not_silently_the_defaults() {
        let dir = tempfile::tempdir().expect("a temp dir");
        std::fs::write(dir.path().join("app.toml"), "appearance = [oops").expect("written");
        let settings = Settings::in_home(dir.path());
        assert!(settings.load().is_err());
    }

    /// The model lives in axio's file, and setting it leaves the rest of that
    /// file — a comment, another section — exactly as it was.
    #[test]
    fn the_default_model_is_read_from_and_written_to_config_toml() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let config = dir.path().join("config.toml");
        std::fs::write(&config, "# mine\n[budget]\nmax_steps = 7\n").expect("written");
        let settings = Settings::in_home(dir.path());

        let before = settings.model();
        assert_eq!(before.provider, "anthropic");

        settings
            .set_model(&ModelDefault {
                provider: "openai-codex".into(),
                name: "gpt-5.4-mini".into(),
            })
            .expect("set");
        let after = settings.model();
        assert_eq!(after.provider, "openai-codex");
        assert_eq!(after.name, "gpt-5.4-mini");
        let text = std::fs::read_to_string(&config).expect("read");
        assert!(text.starts_with("# mine\n"), "{text}");
        assert!(text.contains("max_steps = 7"), "{text}");

        assert!(
            settings
                .set_model(&ModelDefault {
                    provider: String::new(),
                    name: "x".into()
                })
                .is_err()
        );
    }
}
