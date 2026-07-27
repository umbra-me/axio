//! Merging the five layers, and remembering which one won.
//!
//! Provenance is the point: `--explain` can only name a layer if the merge
//! recorded it while the value was being overwritten.

use super::load::section_is_valid;
use super::*;

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
pub(super) fn env_layer(
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

pub(super) fn to_table<T: Serialize>(value: &T) -> toml::Table {
    match toml::Value::try_from(value) {
        Ok(toml::Value::Table(t)) => t,
        _ => toml::Table::new(),
    }
}

/// Deep merge, where `over` wins leaf by leaf.
pub(super) fn merge(base: &mut toml::Table, over: &toml::Table) {
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
pub(super) fn record(
    into: &mut BTreeMap<String, Layer>,
    table: &toml::Table,
    layer: &Layer,
    prefix: &str,
) {
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

pub(super) fn set(table: &mut toml::Table, dotted: &str, value: toml::Value) {
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

/// Keys whose type is `Option`, and which serde therefore omits from a
/// serialised default. Listed so `--explain` knows they exist.
pub(super) const OPTIONAL_KEYS: &[&str] = &["model.base_url", "budget.max_usd_per_turn"];
