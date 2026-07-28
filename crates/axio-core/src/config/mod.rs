//! Layered configuration, per-section salvage, and provenance.
//!
//! Two properties matter more than the schema.
//!
//! **A broken section must not reset the file.** The predecessor project failed
//! a whole parse on one renamed enum variant, returned defaults wholesale, and
//! then auto-saved those defaults over the user's config. Here each section is
//! validated independently: one unparseable table resets that table and nothing
//! else, and a backup is written only when something was actually lost.
//!
//! **Config is read once.** [`RuntimeConfig`] can only be produced by
//! [`Resolved::runtime`], so no code path can re-read configuration inside the
//! loop. A rule in a document is not an enforcement mechanism; a type that
//! cannot be constructed any other way is.

pub mod edit;
mod layers;
mod load;
mod sections;

pub use layers::Layer;
pub use load::find_project_config;
pub use sections::{
    BudgetSection, Config, ModelSection, OutputSection, PermissionsSection, SandboxSection,
    ToolsSection,
};

use layers::{OPTIONAL_KEYS, env_layer, merge, record, set, to_table};
use load::{load_file, validate};

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::agent::RuntimeConfig;
use crate::policy::Policy;
use crate::protocol::{Notice, NoticeLevel};
use crate::provider::{Effort, ReasoningDisplay};
use crate::tool::ToolLimits;

// ---------------------------------------------------------------- the schema

impl Default for ModelSection {
    fn default() -> Self {
        Self {
            name: "claude-opus-5".to_owned(),
            effort: Effort::default(),
            max_tokens: 64_000,
            provider: "anthropic".to_owned(),
            base_url: None,
        }
    }
}

impl Default for BudgetSection {
    fn default() -> Self {
        Self {
            max_usd_per_turn: None,
            max_steps: 50,
        }
    }
}

impl Default for ToolsSection {
    fn default() -> Self {
        Self {
            max_output_bytes: 64 * 1024,
            timeout_secs: 120,
            max_file_bytes: 8 * 1024 * 1024,
        }
    }
}

impl Default for OutputSection {
    fn default() -> Self {
        Self {
            show_reasoning: false,
            show_cost: true,
        }
    }
}

// ---------------------------------------------------------------- resolution

/// A resolved configuration, its provenance, and everything that went wrong
/// while producing it.
#[derive(Debug, Clone)]
pub struct Resolved {
    config: Config,
    provenance: BTreeMap<String, Layer>,
    notices: Vec<Notice>,
}

impl Resolved {
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Notices to replay onto the event stream after `SessionStarted`.
    ///
    /// Never printed directly: `--json` promises one object per line with
    /// `session_started` first, and a stray `eprintln!` from config loading
    /// would sit outside that contract.
    pub fn notices(&self) -> &[Notice] {
        &self.notices
    }

    /// Add something a surface learned after resolution, so it reaches the
    /// event stream through the same counter as everything else.
    pub fn push_notice(&mut self, notice: Notice) {
        self.notices.push(notice);
    }

    /// Which layer won a key, for `axio config --explain`.
    pub fn explain(&self, key: &str) -> Option<&Layer> {
        self.provenance.get(key)
    }

    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.provenance.keys()
    }

    /// The only way to produce a [`RuntimeConfig`].
    pub fn runtime(&self) -> RuntimeConfig {
        RuntimeConfig::from_resolved(self)
    }

    /// The permission engine this configuration describes.
    ///
    /// Rules that fail to compile are dropped with a notice rather than
    /// failing the run: a typo in one rule should not make axio unusable, and
    /// silently ignoring it would be worse than either.
    pub fn policy(&self, unattended_allow: bool) -> (Policy, Vec<Notice>) {
        let mut notices = Vec::new();
        let mut policy = Policy::new();

        for pattern in &self.config.permissions.deny {
            match policy.clone().deny_rule(pattern) {
                Ok(next) => policy = next,
                Err(e) => notices.push(Notice {
                    level: NoticeLevel::Warn,
                    message: format!("ignoring deny rule: {e}"),
                }),
            }
        }
        for pattern in &self.config.permissions.allow {
            match policy.clone().allow_rule(pattern) {
                Ok(next) => policy = next,
                Err(e) => notices.push(Notice {
                    level: NoticeLevel::Warn,
                    message: format!("ignoring allow rule: {e}"),
                }),
            }
        }
        if unattended_allow {
            policy = policy.unattended_allow();
        }
        (policy, notices)
    }
}

/// Where the configuration files live.
#[derive(Debug, Clone, Default)]
pub struct Paths {
    /// `~/.config/axio/config.toml`, or the platform equivalent.
    pub user: Option<PathBuf>,
    /// The nearest `.axio/config.toml` at or above the working directory.
    pub project: Option<PathBuf>,
}

/// Values supplied on the command line. Strongest layer.
#[derive(Debug, Clone, Default)]
pub struct Flags {
    pub model: Option<String>,
    pub effort: Option<Effort>,
}

/// Resolve every layer into one configuration.
pub fn resolve(paths: &Paths, env: &[(String, String)], flags: &Flags) -> Resolved {
    let mut notices = Vec::new();
    let mut provenance: BTreeMap<String, Layer> = BTreeMap::new();
    let mut merged = toml::Table::new();

    // Defaults first, so every key has a provenance entry even when no file
    // mentions it — `--explain` on an untouched key should say "built-in
    // default", not "unknown key".
    let defaults = to_table(&Config::default());
    record(&mut provenance, &defaults, &Layer::Default, "");
    merge(&mut merged, &defaults);

    // An `Option` field defaulting to `None` is omitted by serde, so seeding
    // provenance from a serialised default left `model.base_url` and
    // `budget.max_usd_per_turn` with no entry at all — and `--explain` then
    // reported the two keys a confused user is most likely to ask about
    // ("where is this endpoint coming from?", "did my spend cap take effect?")
    // as not existing, with an authoritative-looking list of known keys that
    // omitted them.
    for key in OPTIONAL_KEYS {
        provenance
            .entry((*key).to_owned())
            .or_insert(Layer::Default);
    }

    let file_layers: Vec<(PathBuf, Layer)> = [
        paths
            .user
            .as_ref()
            .map(|p| (p.clone(), Layer::UserFile(p.clone()))),
        paths
            .project
            .as_ref()
            .map(|p| (p.clone(), Layer::ProjectFile(p.clone()))),
    ]
    .into_iter()
    .flatten()
    .collect();

    for (path, layer) in file_layers {
        let is_project = matches!(layer, Layer::ProjectFile(_));
        let (table, mut file_notices) = load_file(&path, is_project);
        notices.append(&mut file_notices);
        if let Some(table) = table {
            record(&mut provenance, &table, &layer, "");
            merge(&mut merged, &table);
        }
    }

    let (env_table, env_keys) = env_layer(env, &merged, &mut notices);
    for (key, var) in env_keys {
        provenance.insert(key, Layer::Env(var));
    }
    merge(&mut merged, &env_table);

    let mut flag_table = toml::Table::new();
    if let Some(model) = &flags.model {
        set(
            &mut flag_table,
            "model.name",
            toml::Value::String(model.clone()),
        );
        provenance.insert("model.name".into(), Layer::Flag);
    }
    if let Some(effort) = flags.effort {
        set(
            &mut flag_table,
            "model.effort",
            toml::Value::String(effort.as_wire().to_owned()),
        );
        provenance.insert("model.effort".into(), Layer::Flag);
    }
    merge(&mut merged, &flag_table);

    // Every section was validated on the way in, so this cannot normally fail;
    // if it somehow does, defaults plus a loud notice beats refusing to start.
    let config = match toml::Value::Table(merged).try_into::<Config>() {
        Ok(config) => config,
        Err(e) => {
            notices.push(Notice {
                level: NoticeLevel::Error,
                message: format!("configuration could not be assembled ({e}); using defaults"),
            });
            Config::default()
        }
    };

    let config = validate(config, &mut notices);

    Resolved {
        config,
        provenance,
        notices,
    }
}

// ---------------------------------------------------------------- table utils

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, body).unwrap();
        path
    }

    fn resolve_with(user: Option<PathBuf>, project: Option<PathBuf>) -> Resolved {
        resolve(&Paths { user, project }, &[], &Flags::default())
    }

    #[test]
    fn a_provider_can_be_chosen_without_a_registry() {
        let r = resolve(
            &Paths::default(),
            &[
                ("AXIO_PROVIDER".into(), "ollama".into()),
                ("AXIO_MODEL".into(), "gpt-oss:120b".into()),
            ],
            &Flags::default(),
        );
        assert_eq!(r.config().model.provider, "ollama");
        assert_eq!(r.config().model.name, "gpt-oss:120b");
    }

    #[test]
    fn defaults_apply_when_nothing_is_configured() {
        let r = resolve_with(None, None);
        assert_eq!(r.config().model.name, "claude-opus-5");
        assert_eq!(r.config().model.effort, Effort::XHigh);
        assert_eq!(r.config().budget.max_steps, 50);
        assert!(r.notices().is_empty());
    }

    /// The named regression for decision #22's trap. `#[serde(default = "…")]`
    /// fires when a field is missing from a table that exists, and never when
    /// the whole table is missing — so a derived `Default` yields `false` for a
    /// bool documented as `true`, and nothing errors.
    #[test]
    fn a_bool_defaulting_true_survives_an_absent_section() {
        let dir = tempfile::tempdir().unwrap();
        // A file that mentions no [output] section at all.
        let path = write(
            dir.path(),
            "config.toml",
            "[model]\nname = \"claude-opus-5\"\n",
        );
        let r = resolve_with(Some(path), None);
        assert!(
            r.config().output.show_cost,
            "show_cost defaults to true and an absent section must not flip it"
        );
    }

    #[test]
    fn later_layers_win_field_by_field() {
        let dir = tempfile::tempdir().unwrap();
        let user = write(
            dir.path(),
            "user.toml",
            "[model]\nname = \"from-user\"\nmax_tokens = 1000\n",
        );
        let project = write(
            dir.path(),
            "project.toml",
            "[model]\nname = \"from-project\"\n",
        );
        let r = resolve_with(Some(user), Some(project));
        assert_eq!(r.config().model.name, "from-project");
        // Not clobbered by the layer that did not mention it.
        assert_eq!(r.config().model.max_tokens, 1000);
    }

    #[test]
    fn flags_beat_every_file() {
        let dir = tempfile::tempdir().unwrap();
        let user = write(dir.path(), "user.toml", "[model]\nname = \"from-user\"\n");
        let r = resolve(
            &Paths {
                user: Some(user),
                project: None,
            },
            &[("AXIO_MODEL".into(), "from-env".into())],
            &Flags {
                model: Some("from-flag".into()),
                effort: None,
            },
        );
        assert_eq!(r.config().model.name, "from-flag");
        assert_eq!(r.explain("model.name"), Some(&Layer::Flag));
    }

    #[test]
    fn env_beats_a_file_and_is_named_in_the_explanation() {
        let dir = tempfile::tempdir().unwrap();
        let user = write(dir.path(), "user.toml", "[model]\nname = \"from-user\"\n");
        let r = resolve(
            &Paths {
                user: Some(user),
                project: None,
            },
            &[("AXIO_MODEL".into(), "from-env".into())],
            &Flags::default(),
        );
        assert_eq!(r.config().model.name, "from-env");
        assert_eq!(
            r.explain("model.name"),
            Some(&Layer::Env("AXIO_MODEL".into()))
        );
    }

    #[test]
    fn explain_names_the_winning_layer_for_every_layer() {
        let dir = tempfile::tempdir().unwrap();
        let user = write(
            dir.path(),
            "user.toml",
            "[model]\nmax_tokens = 4096\n[tools]\ntimeout_secs = 30\n",
        );
        let project = write(dir.path(), "project.toml", "[budget]\nmax_steps = 7\n");
        let r = resolve(
            &Paths {
                user: Some(user.clone()),
                project: Some(project.clone()),
            },
            &[("AXIO_EFFORT".into(), "low".into())],
            &Flags {
                model: Some("m".into()),
                effort: None,
            },
        );
        assert_eq!(r.explain("model.name"), Some(&Layer::Flag));
        assert_eq!(
            r.explain("model.effort"),
            Some(&Layer::Env("AXIO_EFFORT".into()))
        );
        assert_eq!(
            r.explain("budget.max_steps"),
            Some(&Layer::ProjectFile(project))
        );
        assert_eq!(
            r.explain("tools.timeout_secs"),
            Some(&Layer::UserFile(user))
        );
        // A key nobody set still explains itself.
        assert_eq!(r.explain("output.show_cost"), Some(&Layer::Default));
        assert_eq!(r.explain("nonsense.key"), None);
    }

    /// Regression. `section_is_valid` protects a file from one broken table,
    /// and the env layer bypassed it entirely: a typo in `AXIO_EFFORT` failed
    /// the whole-config deserialise, fell back to `Config::default()`, and
    /// silently discarded the model, the provider and the budget — including
    /// sections the typo never touched. The failure then surfaced as "no
    /// credential for `anthropic`" to someone who had selected `ollama`.
    #[test]
    fn one_bad_environment_value_does_not_reset_the_configuration() {
        let dir = tempfile::tempdir().unwrap();
        let user = write(
            dir.path(),
            "user.toml",
            "[model]\nprovider = \"ollama\"\nname = \"kept-model\"\n[budget]\nmax_steps = 9\n",
        );
        let r = resolve(
            &Paths {
                user: Some(user),
                project: None,
            },
            &[
                ("AXIO_PROVIDER".into(), "ollama".into()),
                ("AXIO_EFFORT".into(), "bogus".into()),
            ],
            &Flags {
                model: None,
                effort: None,
            },
        );
        let c = r.config();
        assert_eq!(c.model.name, "kept-model");
        assert_eq!(c.model.provider, "ollama");
        assert_eq!(c.budget.max_steps, 9, "another section must not be reset");
        assert_ne!(c.model.effort.as_wire(), "bogus");
        assert!(
            r.notices()
                .iter()
                .any(|n| n.message.contains("AXIO_EFFORT")),
            "the dropped value must be named: {:?}",
            r.notices()
        );
    }

    #[test]
    fn one_broken_section_does_not_reset_the_others() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "config.toml",
            "[model]\nname = \"kept\"\nmax_tokens = 2048\n\n\
             [tools]\nmax_output_bytes = \"not a number\"\n\n\
             [budget]\nmax_steps = 9\n",
        );
        let r = resolve_with(Some(path.clone()), None);

        assert_eq!(r.config().model.name, "kept", "a valid section was lost");
        assert_eq!(r.config().budget.max_steps, 9);
        assert_eq!(
            r.config().tools.max_output_bytes,
            ToolsSection::default().max_output_bytes,
            "the broken section resets to defaults"
        );
        assert!(r.notices().iter().any(|n| n.message.contains("[tools]")));

        // And a backup exists, because something really was lost.
        let backups: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().contains("corrupt-"))
            .collect();
        assert_eq!(backups.len(), 1, "exactly one backup for one damaged load");
    }

    #[test]
    fn a_valid_file_writes_no_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "config.toml", "[model]\nname = \"fine\"\n");
        let r = resolve_with(Some(path), None);
        assert!(r.notices().is_empty());
        assert!(
            !std::fs::read_dir(dir.path())
                .unwrap()
                .filter_map(Result::ok)
                .any(|e| e.file_name().to_string_lossy().contains("corrupt-")),
            "a healthy config must not litter backups on every load"
        );
    }

    #[test]
    fn a_file_that_is_not_toml_at_all_is_reported_and_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "config.toml", "this is not = = toml [[[");
        let r = resolve_with(Some(path), None);
        assert_eq!(r.config().model.name, "claude-opus-5");
        assert!(r.notices().iter().any(|n| n.level == NoticeLevel::Error));
    }

    #[test]
    fn a_project_config_may_restrict_but_never_grant() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "project.toml",
            "[permissions]\nallow = [\"bash:*\"]\ndeny = [\"bash:curl\"]\n",
        );
        let r = resolve_with(None, Some(path));
        assert!(
            r.config().permissions.allow.is_empty(),
            "a cloned repository must not be able to grant itself shell access"
        );
        assert_eq!(r.config().permissions.deny, ["bash:curl"]);
        assert!(r.notices().iter().any(|n| n.message.contains("allow")));
    }

    #[test]
    fn a_user_config_may_grant() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "user.toml",
            "[permissions]\nallow = [\"bash:git*\"]\n",
        );
        let r = resolve_with(Some(path), None);
        assert_eq!(r.config().permissions.allow, ["bash:git*"]);
    }

    #[test]
    fn nonsense_values_are_clamped_with_an_explanation() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "config.toml",
            "[budget]\nmax_steps = 0\nmax_usd_per_turn = -1.0\n",
        );
        let r = resolve_with(Some(path), None);
        assert_eq!(r.config().budget.max_steps, 1);
        assert_eq!(r.config().budget.max_usd_per_turn, None);
        assert_eq!(r.notices().len(), 2);
    }

    #[test]
    fn the_project_walk_stops_at_its_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let nested = root.join("a/b/c");
        std::fs::create_dir_all(&nested).unwrap();
        write(&root, ".axio/config.toml", "[model]\nname = \"found\"\n");

        assert!(find_project_config(&nested, Some(&root)).is_some());
        // Boundary respected: starting below it but bounded at `a` finds
        // nothing, rather than reaching into an unrelated parent.
        assert!(find_project_config(&nested, Some(&root.join("a"))).is_none());
    }

    #[test]
    fn a_bad_rule_is_reported_and_the_rest_still_apply() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            "user.toml",
            "[permissions]\ndeny = [\"\", \"bash:curl\"]\n",
        );
        let r = resolve_with(Some(path), None);
        let (_policy, notices) = r.policy(false);
        assert_eq!(notices.len(), 1, "one bad rule, one notice");
    }

    #[test]
    fn effort_round_trips_through_a_config_file() {
        // The wire spelling is what a user would write, and it must load.
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "config.toml", "[model]\neffort = \"xhigh\"\n");
        let r = resolve_with(Some(path), None);
        assert_eq!(r.config().model.effort, Effort::XHigh);
    }
}
