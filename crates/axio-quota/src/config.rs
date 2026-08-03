//! Config file, schema-compatible with upstream CodexBar's `config.json`.
//!
//! Compatibility means a config written by CodexBar on macOS loads here and vice versa.
//! The two apps know different provider sets and different per-provider settings, so both
//! the top level and each provider entry keep an `extra` catch-all: fields we do not
//! understand are preserved verbatim and written back out. Without that, saving a config
//! on Windows would silently delete every macOS-only setting in it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::ProbeError;
use crate::model::ProviderId;
use crate::paths::{Env, app_data_dir, non_empty};

pub const CURRENT_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub version: u32,
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
    /// Fields owned by other CodexBar-family clients. Round-tripped untouched.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            version: CURRENT_VERSION,
            providers: ProviderId::ALL
                .iter()
                .map(|id| ProviderConfig::new(*id))
                .collect(),
            extra: Map::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    /// Kept as a raw string, not a `ProviderId`, so an entry for a provider only the macOS
    /// app supports survives a load/save round trip here instead of being dropped.
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cookie_header: Option<String>,
    /// The team, project or workspace a key is scoped to, where the provider puts that in
    /// the URL rather than in the credential. `workspaceID` on the wire, which is the
    /// spelling CodexBar's config uses — the two files stay interchangeable.
    #[serde(rename = "workspaceID", skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl ProviderConfig {
    pub fn new(id: ProviderId) -> Self {
        ProviderConfig {
            id: id.as_str().to_string(),
            enabled: Some(true),
            api_key: None,
            cookie_header: None,
            workspace_id: None,
            extra: Map::new(),
        }
    }

    pub fn provider_id(&self) -> Option<ProviderId> {
        ProviderId::parse(&self.id)
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.unwrap_or(true)
    }
}

impl Config {
    /// `%APPDATA%\axio\quota\config.json`, overridable with `AXIO_QUOTA_CONFIG` for tests
    /// and for users who keep dotfiles somewhere specific.
    ///
    /// Nested under `axio\` rather than flat, so a second axio product does not have to
    /// choose between colliding with this one and inventing a third convention.
    pub fn default_path(env: &Env) -> PathBuf {
        if let Some(explicit) = non_empty(env, "AXIO_QUOTA_CONFIG") {
            return PathBuf::from(explicit);
        }
        app_data_dir(env)
            .join("axio")
            .join("quota")
            .join("config.json")
    }

    /// Loads the config, or returns the default when the file does not exist yet.
    ///
    /// A missing file is not an error: first run has no config, and the tray must still
    /// start. A *malformed* file is an error — silently replacing it with defaults would
    /// throw away the user's API keys.
    pub fn load(path: &Path) -> Result<Config, ProbeError> {
        let raw = match std::fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
            Err(err) => {
                return Err(ProbeError::Io {
                    path: path.display().to_string(),
                    detail: err.to_string(),
                });
            }
        };
        serde_json::from_str(&raw).map_err(|err| ProbeError::decode("config file", err))
    }

    pub fn save(&self, path: &Path) -> Result<(), ProbeError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| ProbeError::Io {
                path: parent.display().to_string(),
                detail: err.to_string(),
            })?;
        }
        let raw = serde_json::to_string_pretty(self)
            .map_err(|err| ProbeError::decode("config file", err))?;
        std::fs::write(path, raw).map_err(|err| ProbeError::Io {
            path: path.display().to_string(),
            detail: err.to_string(),
        })
    }

    pub fn provider(&self, id: ProviderId) -> Option<&ProviderConfig> {
        self.providers
            .iter()
            .find(|entry| entry.provider_id() == Some(id))
    }

    /// The provider entry, or a default one, so callers never have to handle "absent".
    pub fn provider_or_default(&self, id: ProviderId) -> ProviderConfig {
        self.provider(id)
            .cloned()
            .unwrap_or_else(|| ProviderConfig::new(id))
    }

    pub fn enabled_providers(&self) -> Vec<ProviderId> {
        ProviderId::ALL
            .iter()
            .copied()
            .filter(|id| {
                self.provider(*id)
                    .map(ProviderConfig::is_enabled)
                    .unwrap_or(false)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_providers_and_fields_survive_a_round_trip() {
        // Shape written by upstream CodexBar on macOS, including a provider we do not know
        // and a per-provider field we do not model.
        let raw = r#"{
            "version": 1,
            "hooks": { "enabled": false },
            "providers": [
                { "id": "codex", "enabled": true, "codexActiveSource": "cli" },
                { "id": "cursor", "enabled": true, "cookieHeader": "session=abc" }
            ]
        }"#;

        let config: Config = serde_json::from_str(raw).expect("parses");
        let written = serde_json::to_value(&config).expect("serializes");

        assert_eq!(written["hooks"]["enabled"], serde_json::json!(false));
        assert_eq!(written["providers"][0]["codexActiveSource"], "cli");
        assert_eq!(written["providers"][1]["id"], "cursor");
        assert_eq!(written["providers"][1]["cookieHeader"], "session=abc");
    }

    #[test]
    fn missing_enabled_flag_defaults_to_on() {
        let raw = r#"{ "version": 1, "providers": [{ "id": "claude" }] }"#;
        let config: Config = serde_json::from_str(raw).expect("parses");
        assert!(config.provider(ProviderId::Claude).unwrap().is_enabled());
        assert_eq!(config.enabled_providers(), vec![ProviderId::Claude]);
    }

    #[test]
    fn explicit_config_path_overrides_appdata() {
        let env: Env = [("AXIO_QUOTA_CONFIG".to_string(), r"D:\cfg.json".to_string())]
            .into_iter()
            .collect();
        assert_eq!(Config::default_path(&env), PathBuf::from(r"D:\cfg.json"));
    }
}
