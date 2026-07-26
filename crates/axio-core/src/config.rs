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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::agent::RuntimeConfig;
use crate::policy::Policy;
use crate::protocol::{Notice, NoticeLevel};
use crate::provider::{Effort, ReasoningDisplay};
use crate::tool::ToolLimits;

/// Which layer supplied a value. Ordered weakest to strongest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Layer {
    Default,
    UserFile(PathBuf),
    ProjectFile(PathBuf),
    Env(String),
    Flag,
}

impl Layer {
    pub fn describe(&self) -> String {
        match self {
            Layer::Default => "built-in default".to_owned(),
            Layer::UserFile(p) => format!("user config ({})", p.display()),
            Layer::ProjectFile(p) => format!("project config ({})", p.display()),
            Layer::Env(k) => format!("environment ({k})"),
            Layer::Flag => "command-line flag".to_owned(),
        }
    }
}

// ---------------------------------------------------------------- the schema

/// The resolved configuration. Every field concrete.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub model: ModelSection,
    pub budget: BudgetSection,
    pub tools: ToolsSection,
    pub permissions: PermissionsSection,
    pub output: OutputSection,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModelSection {
    pub name: String,
    pub effort: Effort,
    pub max_tokens: u32,
    /// Which dialect to speak. Two impls, chosen by name — deliberately not a
    /// registry: a second provider is a second implementation, not an
    /// extension point, until something needs it to be.
    pub provider: String,
    /// Override the endpoint. Mostly for talking to a compatible host.
    pub base_url: Option<String>,
}

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BudgetSection {
    /// Stops a turn once the spend so far exceeds this. `None` is no ceiling.
    pub max_usd_per_turn: Option<f64>,
    pub max_steps: u32,
}

impl Default for BudgetSection {
    fn default() -> Self {
        Self {
            max_usd_per_turn: None,
            max_steps: 50,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ToolsSection {
    pub max_output_bytes: usize,
    pub timeout_secs: u64,
    pub max_file_bytes: u64,
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

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PermissionsSection {
    pub allow: Vec<String>,
    pub deny: Vec<String>,
}

/// Hand-written `Default`, not derived.
///
/// This is the trap decision #22 exists for: `#[serde(default = "…")]` fires
/// when a *field* is absent from a table that is present, and never when the
/// whole table is absent. A derived `Default` then yields `false` for a bool
/// whose documented default is `true`, and the only symptom is a feature
/// quietly switching itself off for anyone who never wrote the section.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OutputSection {
    /// Ask the provider for a readable summary of its reasoning.
    pub show_reasoning: bool,
    /// Print what the turn cost. Defaults **on**: an agent that spends money
    /// without saying so is a week-one complaint.
    pub show_cost: bool,
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

/// Find the nearest project config at or above `start`, without escaping into
/// a parent of `boundary`.
///
/// The boundary matters: walking to the filesystem root would pick up a
/// `.axio/config.toml` in a home directory or in `/tmp` and apply it to an
/// unrelated project.
pub fn find_project_config(start: &Path, boundary: Option<&Path>) -> Option<PathBuf> {
    let mut dir = Some(start);
    while let Some(current) = dir {
        let candidate = current.join(".axio").join("config.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
        if boundary == Some(current) {
            return None;
        }
        dir = current.parent();
    }
    None
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

/// Clamp values that would make the agent misbehave rather than fail.
fn validate(mut config: Config, notices: &mut Vec<Notice>) -> Config {
    if config.budget.max_steps == 0 {
        notices.push(Notice {
            level: NoticeLevel::Warn,
            message: "[budget] max_steps = 0 would end every turn before it began; using 1".into(),
        });
        config.budget.max_steps = 1;
    }
    if let Some(limit) = config.budget.max_usd_per_turn
        && (!limit.is_finite() || limit <= 0.0)
    {
        notices.push(Notice {
            level: NoticeLevel::Warn,
            message: format!(
                "[budget] max_usd_per_turn = {limit} is not a spendable amount; ignoring"
            ),
        });
        config.budget.max_usd_per_turn = None;
    }
    if config.tools.max_output_bytes < 1024 {
        notices.push(Notice {
            level: NoticeLevel::Warn,
            message: "[tools] max_output_bytes below 1024 leaves no room for a marker; using 1024"
                .into(),
        });
        config.tools.max_output_bytes = 1024;
    }
    config
}

/// Read one file, validating section by section.
///
/// Returns the sections that survived. A section that fails to deserialise is
/// dropped and reported; the rest of the file is kept. A backup is written only
/// when something was actually lost.
fn load_file(path: &Path, project: bool) -> (Option<toml::Table>, Vec<Notice>) {
    let mut notices = Vec::new();

    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (None, notices),
        Err(e) => {
            notices.push(Notice {
                level: NoticeLevel::Warn,
                message: format!("cannot read {}: {e}", path.display()),
            });
            return (None, notices);
        }
    };

    let parsed: toml::Table = match text.parse() {
        Ok(table) => table,
        Err(e) => {
            // Not valid TOML at all: nothing to salvage section by section.
            notices.push(Notice {
                level: NoticeLevel::Error,
                message: format!("{} is not valid TOML ({e}); ignoring it", path.display()),
            });
            backup(path, &mut notices);
            return (None, notices);
        }
    };

    let mut kept = toml::Table::new();
    let mut lost = false;

    for (name, value) in parsed {
        if !section_is_valid(&name, &value) {
            notices.push(Notice {
                level: NoticeLevel::Warn,
                message: format!(
                    "[{name}] in {} could not be understood; that section is using defaults",
                    path.display()
                ),
            });
            lost = true;
            continue;
        }
        kept.insert(name, value);
    }

    if project {
        // A project config may only make axio ask more, never less. A cloned
        // repository that can grant itself shell access is remote code
        // execution by `cd`, on a tool that ships no sandbox.
        if let Some(toml::Value::Table(perms)) = kept.get_mut("permissions")
            && perms.remove("allow").is_some()
        {
            notices.push(Notice {
                level: NoticeLevel::Warn,
                message: format!(
                    "ignoring [permissions] allow in {}: a project config may only \
                     add restrictions, never remove them",
                    path.display()
                ),
            });
        }
    }

    if lost {
        backup(path, &mut notices);
    }
    (Some(kept), notices)
}

/// Does this section deserialise into its typed shape?
fn section_is_valid(name: &str, value: &toml::Value) -> bool {
    let v = value.clone();
    match name {
        "model" => v.try_into::<ModelSection>().is_ok(),
        "budget" => v.try_into::<BudgetSection>().is_ok(),
        "tools" => v.try_into::<ToolsSection>().is_ok(),
        "permissions" => v.try_into::<PermissionsSection>().is_ok(),
        "output" => v.try_into::<OutputSection>().is_ok(),
        // An unknown section is a typo or a newer axio; either way it is not
        // ours to reset, and dropping it silently is the friendlier failure.
        _ => false,
    }
}

/// Copy a file aside before its broken sections are ignored.
fn backup(path: &Path, notices: &mut Vec<Notice>) {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let backup = path.with_extension(format!("corrupt-{stamp}"));
    match std::fs::copy(path, &backup) {
        Ok(_) => notices.push(Notice {
            level: NoticeLevel::Info,
            message: format!("a copy of the previous file is at {}", backup.display()),
        }),
        Err(e) => notices.push(Notice {
            level: NoticeLevel::Warn,
            message: format!("could not back up {}: {e}", path.display()),
        }),
    }
}

/// The environment layer.
///
/// Deliberately a short explicit list rather than a generic `AXIO_<SECTION>_<KEY>`
/// scheme: an environment variable is the layer most likely to be set by
/// accident in a shell profile and forgotten, so every one of them should be a
/// deliberate decision by someone reading this list.
///
/// `below` is everything merged so far, and every value is validated against
/// the section it would land in before it goes anywhere near the whole-config
/// deserialise. A file gets that protection from `section_is_valid`; without
/// the same check here, one typo in one variable fails the final `try_into`,
/// falls back to `Config::default()`, and silently discards the entire
/// configuration — including budget limits from a section the typo never
/// touched.
fn env_layer(
    env: &[(String, String)],
    below: &toml::Table,
    notices: &mut Vec<Notice>,
) -> (toml::Table, Vec<(String, String)>) {
    let mut table = toml::Table::new();
    let mut keys = Vec::new();

    for (var, key) in [
        ("AXIO_MODEL", "model.name"),
        ("AXIO_EFFORT", "model.effort"),
        ("AXIO_MAX_STEPS", "budget.max_steps"),
        ("AXIO_MAX_USD_PER_TURN", "budget.max_usd_per_turn"),
        ("AXIO_PROVIDER", "model.provider"),
        ("AXIO_BASE_URL", "model.base_url"),
    ] {
        let Some((_, raw)) = env.iter().find(|(k, _)| k == var) else {
            continue;
        };
        if raw.trim().is_empty() {
            continue;
        }
        let value = match key {
            "budget.max_steps" => raw.parse::<i64>().ok().map(toml::Value::Integer),
            "budget.max_usd_per_turn" => raw.parse::<f64>().ok().map(toml::Value::Float),
            _ => Some(toml::Value::String(raw.clone())),
        };
        let value = value.filter(|value| {
            let (section, _) = key.split_once('.').expect("every key names a section");
            let mut candidate = below
                .get(section)
                .and_then(toml::Value::as_table)
                .cloned()
                .unwrap_or_default();
            let mut probe = toml::Table::new();
            set(&mut probe, key, value.clone());
            if let Some(toml::Value::Table(over)) = probe.get(section) {
                merge(&mut candidate, over);
            }
            section_is_valid(section, &toml::Value::Table(candidate))
        });

        match value {
            Some(value) => {
                set(&mut table, key, value);
                keys.push((key.to_owned(), var.to_owned()));
            }
            None => notices.push(Notice {
                level: NoticeLevel::Warn,
                message: format!("{var}={raw} is not a valid value; ignoring it"),
            }),
        }
    }
    (table, keys)
}

// ---------------------------------------------------------------- table utils

fn to_table<T: Serialize>(value: &T) -> toml::Table {
    match toml::Value::try_from(value) {
        Ok(toml::Value::Table(t)) => t,
        _ => toml::Table::new(),
    }
}

/// Deep merge, where `over` wins leaf by leaf.
fn merge(base: &mut toml::Table, over: &toml::Table) {
    for (key, value) in over {
        match (base.get_mut(key), value) {
            (Some(toml::Value::Table(existing)), toml::Value::Table(incoming)) => {
                merge(existing, incoming)
            }
            _ => {
                base.insert(key.clone(), value.clone());
            }
        }
    }
}

/// Record which layer supplied each leaf key, as a dotted path.
fn record(into: &mut BTreeMap<String, Layer>, table: &toml::Table, layer: &Layer, prefix: &str) {
    for (key, value) in table {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        match value {
            toml::Value::Table(inner) => record(into, inner, layer, &path),
            _ => {
                into.insert(path, layer.clone());
            }
        }
    }
}

fn set(table: &mut toml::Table, dotted: &str, value: toml::Value) {
    let mut parts = dotted.split('.').peekable();
    let mut current = table;
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            current.insert(part.to_owned(), value);
            return;
        }
        current = match current
            .entry(part.to_owned())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
        {
            toml::Value::Table(t) => t,
            other => {
                *other = toml::Value::Table(toml::Table::new());
                match other {
                    toml::Value::Table(t) => t,
                    _ => unreachable!("just assigned a table"),
                }
            }
        };
    }
}

impl Config {
    pub fn tool_limits(&self) -> ToolLimits {
        ToolLimits {
            max_output_bytes: self.tools.max_output_bytes,
            timeout: std::time::Duration::from_secs(self.tools.timeout_secs),
            max_file_bytes: self.tools.max_file_bytes,
        }
    }

    pub fn reasoning(&self) -> ReasoningDisplay {
        if self.output.show_reasoning {
            ReasoningDisplay::Summarized
        } else {
            ReasoningDisplay::Omitted
        }
    }
}

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
