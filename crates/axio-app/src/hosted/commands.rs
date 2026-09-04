//! What a hosted agent's own interface answers to, for a composer that is
//! not that interface.
//!
//! The card's follow-up box types into the agent's prompt, so anything the
//! agent's slash menu would take, it takes — but the box had no menu, and a
//! person typing `/` into it was typing blind. The list is the harness's
//! built-in set plus what this machine and this repository add to it: Claude
//! Code's `.claude/commands/*.md` and `.claude/skills/*/SKILL.md`, in the home
//! directory and in the repository the terminal works on.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use axio_pty::Harness;
use serde::{Deserialize, Serialize};

use super::Hosted;
use crate::model::AppError;

/// One slash command, as a menu shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../ui/src/generated/")]
pub struct SlashCommand {
    /// With its slash: `/model`.
    pub name: String,
    /// A few words. Empty for a command found on disk, which carries none.
    pub detail: String,
}

impl Hosted {
    /// The commands the terminal's agent answers to: built in, then found.
    pub fn commands(&self, id: &str) -> Result<Vec<SlashCommand>, AppError> {
        let (harness, repo) = {
            let held = self
                .sessions
                .lock()
                .expect("no lock is held across an await");
            let entry = held
                .get(id)
                .ok_or_else(|| AppError::NoSuchSession(format!("no hosted session {id}")))?;
            (entry.harness, entry.place.repo.clone())
        };
        let mut out: Vec<SlashCommand> = harness
            .slash_commands()
            .iter()
            .map(|(name, detail)| SlashCommand {
                name: (*name).to_owned(),
                detail: (*detail).to_owned(),
            })
            .collect();
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from);
        for name in found(harness, home.as_deref(), &repo) {
            if !out.iter().any(|c| c.name == name) {
                out.push(SlashCommand {
                    name,
                    detail: String::new(),
                });
            }
        }
        Ok(out)
    }
}

/// Commands a person added, by the tool's own conventions. Only Claude Code's
/// are known well enough to list; the other harnesses get their built-ins.
fn found(harness: Harness, home: Option<&Path>, repo: &Path) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    if harness != Harness::Claude {
        return names;
    }
    let roots: Vec<PathBuf> = home
        .map(|h| h.join(".claude"))
        .into_iter()
        .chain(std::iter::once(repo.join(".claude")))
        .collect();
    for root in roots {
        // `commands/<name>.md`, and `commands/<dir>/<name>.md` as `/dir:name`.
        collect_commands(&root.join("commands"), "", &mut names);
        // `skills/<name>/SKILL.md` as `/name`.
        if let Ok(entries) = std::fs::read_dir(root.join("skills")) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.join("SKILL.md").is_file()
                    && let Some(name) = path.file_name().and_then(|n| n.to_str())
                {
                    names.insert(format!("/{name}"));
                }
            }
        }
    }
    names
}

fn collect_commands(dir: &Path, prefix: &str, names: &mut BTreeSet<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if path.is_dir() {
            collect_commands(&path, &format!("{prefix}{stem}:"), names);
        } else if path.extension().is_some_and(|e| e == "md") {
            names.insert(format!("/{prefix}{stem}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_commands_and_skills_are_found_in_home_and_repository() {
        let home = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let cmds = home.path().join(".claude/commands");
        std::fs::create_dir_all(cmds.join("git")).unwrap();
        std::fs::write(cmds.join("fix.md"), "").unwrap();
        std::fs::write(cmds.join("git/commit.md"), "").unwrap();
        std::fs::write(cmds.join("notes.txt"), "").unwrap();
        let skill = repo.path().join(".claude/skills/deploy");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(skill.join("SKILL.md"), "").unwrap();

        let names = found(Harness::Claude, Some(home.path()), repo.path());
        let got: Vec<&str> = names.iter().map(String::as_str).collect();
        assert_eq!(got, vec!["/deploy", "/fix", "/git:commit"]);
        assert!(found(Harness::Codex, Some(home.path()), repo.path()).is_empty());
    }

    #[test]
    fn every_built_in_starts_with_a_slash_and_repeats_nowhere() {
        for harness in Harness::ALL {
            let list = harness.slash_commands();
            let set: BTreeSet<&str> = list.iter().map(|(n, _)| *n).collect();
            assert_eq!(set.len(), list.len(), "{harness:?} repeats a command");
            assert!(
                list.iter()
                    .all(|(n, d)| n.starts_with('/') && !d.is_empty())
            );
        }
    }
}
