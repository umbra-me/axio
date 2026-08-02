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
    let mut foreign: Vec<String> = Vec::new();

    for (name, value) in parsed {
        // Not ours: leave it alone. It belongs to another tool, or to a newer
        // axio, or it is a typo — and none of those is this file being damaged.
        if !is_known_section(&name) {
            foreign.push(name);
            continue;
        }
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

    // One line, not one per section. A typo is still findable in it; thirty
    // warnings before the first useful word are not a diagnostic, they are a
    // wall someone has to read past every single run.
    if !foreign.is_empty() {
        const SHOWN: usize = 6;
        let shown = foreign
            .iter()
            .take(SHOWN)
            .map(|n| format!("[{n}]"))
            .collect::<Vec<_>>()
            .join(" ");
        let rest = foreign.len().saturating_sub(SHOWN);
        let tail = if rest > 0 {
            format!(" and {rest} more")
        } else {
            String::new()
        };
        notices.push(Notice {
            level: NoticeLevel::Info,
            message: format!(
                "ignoring {} section{} in {} that axio does not use: {shown}{tail}",
                foreign.len(),
                if foreign.len() == 1 { "" } else { "s" },
                path.display(),
            ),
        });
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

/// The sections this build knows about.
///
/// A section outside this list is not axio's, and the difference from a section
/// that is axio's and will not parse is the whole point. Both used to return
/// `false` from one function, and the caller treated every `false` as damage:
/// running axio against a `.axio/config.toml` belonging to some other tool
/// printed a warning per section — thirty-three of them, ahead of anything
/// useful — and then wrote a `.corrupt-<ts>` copy of a file that was in perfect
/// health. Reported by someone who simply ran it.
pub(super) fn is_known_section(name: &str) -> bool {
    matches!(
        name,
        "model" | "budget" | "tools" | "permissions" | "output" | "sandbox"
    )
}

/// Whether one of *our* sections parses. Only meaningful for a known name.
pub(super) fn section_is_valid(name: &str, value: &toml::Value) -> bool {
    let v = value.clone();
    match name {
        "model" => v.try_into::<ModelSection>().is_ok(),
        "budget" => v.try_into::<BudgetSection>().is_ok(),
        "tools" => v.try_into::<ToolsSection>().is_ok(),
        "permissions" => v.try_into::<PermissionsSection>().is_ok(),
        "output" => v.try_into::<OutputSection>().is_ok(),
        "sandbox" => v.try_into::<SandboxSection>().is_ok(),
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
/// a parent of `boundary`, and never returning the user's own config.
///
/// The boundary matters: walking to the filesystem root would pick up a
/// `.axio/config.toml` in a home directory or in `/tmp` and apply it to an
/// unrelated project.
///
/// `excluded` matters for a sharper reason. The user's configuration lives in a
/// directory of the same name — `~/.axio/config.toml` — so a session running
/// anywhere beneath the home directory would find it on the way up and load it
/// a second time, as a *project*. That layer may only add restrictions, so a
/// person's own settings would come back as a restriction-only copy of
/// themselves. One file cannot be two layers, and the check is against the
/// paths rather than the boundary because `AXIO_HOME` can put it anywhere. The
/// list includes both the active home and the canonical `~/.axio`: relocating
/// Axio must not turn a previous user config into a project config.
pub fn find_project_config(
    start: &Path,
    boundary: Option<&Path>,
    excluded: &[PathBuf],
) -> Option<PathBuf> {
    let mut dir = Some(start);
    while let Some(current) = dir {
        let candidate = current.join(".axio").join("config.toml");
        if candidate.is_file() && !excluded.iter().any(|path| path == &candidate) {
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

#[cfg(test)]
mod foreign_config_tests {
    use super::*;

    /// Reported from a Windows machine: `axio.exe` against a `.axio/config.toml`
    /// belonging to another tool printed thirty-three warnings before anything
    /// useful, and wrote a `.corrupt-<ts>` copy of a file that was fine.
    #[test]
    fn another_tools_config_is_ignored_quietly_and_left_alone() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("config.toml");
        let sections = [
            "agents",
            "appearance",
            "approval",
            "artifacts",
            "checkpoints",
            "code_mode",
            "connectors",
            "diagnostics",
            "git",
            "interface",
            "keybindings",
            "lsp",
            "mcp",
            "memory",
            "multi_agent",
            "network",
            "notifications",
            "plugins",
            "reasoning",
            "rendering",
            "routing",
            "rules",
            "skills",
            "terminal",
            "updates",
            "voice",
            "worktrees",
        ];
        let text: String = sections
            .iter()
            .map(|s| format!("[{s}]\nsomething = true\n\n"))
            .collect();
        std::fs::write(&path, &text).expect("the file");

        let (kept, notices) = load_file(&path, false);
        assert!(
            kept.expect("a table").is_empty(),
            "nothing of ours was in it"
        );

        let warnings: Vec<&Notice> = notices
            .iter()
            .filter(|n| matches!(n.level, NoticeLevel::Warn))
            .collect();
        assert!(
            warnings.is_empty(),
            "a foreign config is not damage: {warnings:?}"
        );
        assert_eq!(
            notices.len(),
            1,
            "one line, not one per section: {notices:?}"
        );
        assert!(
            notices[0].message.contains("27 sections"),
            "{:?}",
            notices[0]
        );

        let stray: Vec<_> = std::fs::read_dir(dir.path())
            .expect("the dir")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains("corrupt"))
            .collect();
        assert!(
            stray.is_empty(),
            "it copied a file that was never corrupt: {stray:?}"
        );
        assert_eq!(std::fs::read_to_string(&path).expect("still there"), text);
    }

    /// The other half: a section that *is* ours and will not parse is damage,
    /// and still says so and still preserves what it replaced.
    #[test]
    fn one_of_our_sections_failing_to_parse_is_still_reported_and_backed_up() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[model]\neffort = 12345\n\n[budget]\nmax_steps = 7\n",
        )
        .expect("the file");

        let (kept, notices) = load_file(&path, false);
        let kept = kept.expect("a table");
        assert!(kept.contains_key("budget"), "the good section survives");
        assert!(!kept.contains_key("model"), "the bad one is reset");
        assert!(
            notices
                .iter()
                .any(|n| matches!(n.level, NoticeLevel::Warn) && n.message.contains("[model]")),
            "{notices:?}"
        );
        let backups: Vec<_> = std::fs::read_dir(dir.path())
            .expect("the dir")
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains("corrupt"))
            .collect();
        assert_eq!(backups.len(), 1, "the replaced file is preserved");
    }
}
