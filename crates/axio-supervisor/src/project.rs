//! The repositories under supervision.
//!
//! Sessions are grouped by repository rather than by window, because the thing
//! a person actually asks is "what is running on *this* project" — and the
//! answer has to survive a restart. So the id is derived from the repository's
//! own path rather than minted, and the same checkout is the same project
//! every time.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Result, SupervisorError};
use crate::git;

/// A stable name for a repository.
///
/// Derived, not generated: a minted id would need a registry file to survive a
/// restart, and a registry file that disagrees with the disk is a bug with no
/// good repair. Deriving means there is nothing to disagree with.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ProjectId(String);

impl ProjectId {
    /// Hash the canonical path. Case is folded on Windows, where two spellings
    /// of one directory are one directory and would otherwise be two projects
    /// with two sets of worktrees.
    pub fn of(root: &Path) -> Self {
        let text = root.to_string_lossy();
        let text = if cfg!(windows) {
            text.to_lowercase()
        } else {
            text.into_owned()
        };
        Self(format!("{:016x}", fnv1a(text.as_bytes())))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ProjectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// FNV-1a, hand-rolled.
///
/// `DefaultHasher` is deterministic within one Rust version and explicitly not
/// promised across them, and this id is written to disk — a toolchain upgrade
/// would silently orphan every worktree directory. Sixty-four bits of a
/// non-adversarial path is not a collision anyone will meet, and the failure if
/// they did is two repositories sharing a directory name, not a security
/// boundary.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// One repository under supervision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: ProjectId,
    /// The repository root, canonicalised.
    pub root: PathBuf,
    /// The directory name. For display only — two projects may share one.
    pub name: String,
}

impl Project {
    /// Find the repository containing `path` and describe it.
    ///
    /// The root comes from git rather than from a walk looking for `.git`,
    /// because a worktree's `.git` is a file pointing elsewhere and a submodule's
    /// is too. Asking git is the only way to be right about both.
    pub async fn open(path: &Path) -> Result<Self> {
        let root = git::toplevel(path).await?;
        let root = root.canonicalize().unwrap_or(root);
        let name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.to_string_lossy().into_owned());
        Ok(Self {
            id: ProjectId::of(&root),
            root,
            name,
        })
    }
}

/// Every repository the supervisor has been pointed at.
///
/// Deliberately not persisted. It is rebuilt from the session index on start,
/// so the set of projects is a consequence of the work that exists rather than
/// a second list that can drift from it.
#[derive(Debug, Default)]
pub struct Projects {
    by_id: BTreeMap<ProjectId, Project>,
}

impl Projects {
    pub fn insert(&mut self, project: Project) -> Project {
        self.by_id
            .entry(project.id.clone())
            .or_insert(project)
            .clone()
    }

    pub fn get(&self, id: &ProjectId) -> Result<Project> {
        self.by_id
            .get(id)
            .cloned()
            .ok_or_else(|| SupervisorError::NoSuchProject(id.to_string()))
    }

    /// Every project, ordered by name then id, so a list does not reshuffle
    /// itself between reads.
    pub fn all(&self) -> Vec<Project> {
        let mut out: Vec<Project> = self.by_id.values().cloned().collect();
        out.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        out
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_path_is_always_the_same_project() {
        let a = ProjectId::of(Path::new("/home/me/work/thing"));
        let b = ProjectId::of(Path::new("/home/me/work/thing"));
        assert_eq!(a, b);
        assert_ne!(a, ProjectId::of(Path::new("/home/me/work/other")));
    }

    /// The id is written to disk and names a worktree directory, so it has to
    /// be the same after a toolchain upgrade. Pinning the value is the only
    /// way a change to the hash cannot pass silently.
    #[test]
    fn the_derivation_is_pinned() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a(b"foobar"), 0x85944171f73967e8);
    }

    #[test]
    fn projects_list_in_a_stable_order() {
        let mut projects = Projects::default();
        for name in ["zeta", "alpha", "middle"] {
            projects.insert(Project {
                id: ProjectId::of(Path::new(name)),
                root: PathBuf::from(name),
                name: name.to_owned(),
            });
        }
        let names: Vec<String> = projects.all().into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["alpha", "middle", "zeta"]);
    }

    #[test]
    fn inserting_the_same_project_twice_keeps_one() {
        let mut projects = Projects::default();
        let project = Project {
            id: ProjectId::of(Path::new("/w")),
            root: PathBuf::from("/w"),
            name: "w".to_owned(),
        };
        projects.insert(project.clone());
        projects.insert(project);
        assert_eq!(projects.all().len(), 1);
    }
}
