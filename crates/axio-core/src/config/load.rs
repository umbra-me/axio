//! Reading a file, and salvaging what is left when part of it will not parse.
//!
//! A section that fails to parse is reset on its own; the rest of the file
//! survives. A configuration that is wholly valid is never backed up.

use super::sections::{
    BudgetSection, ModelSection, OutputSection, PermissionsSection, ToolsSection,
};
use super::*;

/// Read one file, validating section by section.
///
/// Returns the sections that survived. A section that fails to deserialise is
/// dropped and reported; the rest of the file is kept. A backup is written only
/// when something was actually lost.
pub(super) fn load_file(path: &Path, project: bool) -> (Option<toml::Table>, Vec<Notice>) {
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
pub(super) fn section_is_valid(name: &str, value: &toml::Value) -> bool {
    let v = value.clone();
    match name {
        "model" => v.try_into::<ModelSection>().is_ok(),
        "budget" => v.try_into::<BudgetSection>().is_ok(),
        "tools" => v.try_into::<ToolsSection>().is_ok(),
        "permissions" => v.try_into::<PermissionsSection>().is_ok(),
        "output" => v.try_into::<OutputSection>().is_ok(),
        "sandbox" => v.try_into::<SandboxSection>().is_ok(),
        // An unknown section is a typo or a newer axio; either way it is not
        // ours to reset, and dropping it silently is the friendlier failure.
        _ => false,
    }
}

/// Copy a file aside before its broken sections are ignored.
///
/// Once per *content*, not once per load. axio never repairs the file, so the
/// "something was lost" condition stays true forever — and a time-stamped name
/// meant every subsequent invocation, including the report-and-exit modes,
/// dropped another identical copy into the user's `.axio/` directory.
pub(super) fn backup(path: &Path, notices: &mut Vec<Notice>) {
    let Ok(current) = std::fs::read(path) else {
        return;
    };
    if let Some(dir) = path.parent()
        && let Ok(entries) = std::fs::read_dir(dir)
    {
        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
        let prefix = format!("{stem}.corrupt-");
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(&prefix)
                && std::fs::read(entry.path()).is_ok_and(|prior| prior == current)
            {
                // This exact content is already preserved, which was the point.
                return;
            }
        }
    }

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

/// Clamp values that would make the agent misbehave rather than fail.
pub(super) fn validate(mut config: Config, notices: &mut Vec<Notice>) -> Config {
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
