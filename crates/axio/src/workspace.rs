//! What the launch directory turns out to be.
//!
//! The workspace root is wherever axio was started, and every tool is confined
//! to it — a path outside is refused rather than searched. That is the right
//! rule and it has one bad case: started in a directory that merely *contains*
//! projects, the workspace is the whole tree. Nothing is broken, so nothing
//! reports anything; a glob walks every repository, takes seconds, and returns
//! matches from three projects that have nothing to do with each other.
//!
//! Cheap to detect and worth one line at startup, because the alternative is
//! finding out from a slow search whose results look almost right.

use std::path::Path;

/// Below this it is not worth saying. A directory holding one project beside
/// some loose files is an ordinary place to work.
const CROWDED: usize = 2;

/// A one-line notice when the launch directory is a shelf of projects rather
/// than a project.
///
/// `None` when the directory is itself a repository: a repository with
/// submodules or vendored checkouts inside it is exactly where someone means
/// to be, and saying so every time is noise that teaches people to skip
/// startup lines.
pub(crate) fn crowded_notice(cwd: &Path) -> Option<String> {
    if cwd.join(".git").exists() {
        return None;
    }
    let found = repositories(cwd);
    if found < CROWDED {
        return None;
    }
    Some(format!(
        "this directory holds {found} git repositories and is the workspace root, \
         so searches cover all of them — start axio inside one to narrow that"
    ))
}

/// Immediate children that are repositories.
///
/// One level only, and one `read_dir`. This runs before every session, and a
/// deep walk to answer a question about the shape of a directory would cost
/// more than the searches it is warning about.
fn repositories(cwd: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(cwd) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().join(".git").exists())
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(root: &Path, name: &str) {
        std::fs::create_dir_all(root.join(name).join(".git")).expect("a repository");
    }

    #[test]
    fn a_shelf_of_projects_is_worth_saying() {
        let dir = tempfile::tempdir().expect("a temp dir");
        repo(dir.path(), "one");
        repo(dir.path(), "two");
        repo(dir.path(), "three");

        let said = crowded_notice(dir.path()).expect("a notice");
        assert!(said.contains('3'), "{said}");
        // The advice has to be actionable, not just an observation.
        assert!(said.contains("start axio inside one"), "{said}");
    }

    #[test]
    fn one_project_beside_some_files_is_ordinary() {
        let dir = tempfile::tempdir().expect("a temp dir");
        repo(dir.path(), "only");
        std::fs::write(dir.path().join("notes.md"), "hello").expect("a file");
        assert!(crowded_notice(dir.path()).is_none());
    }

    /// A repository with submodules or vendored checkouts is precisely where
    /// someone means to be working, and a warning there is noise that teaches
    /// people to stop reading startup lines.
    #[test]
    fn a_repository_containing_repositories_says_nothing() {
        let dir = tempfile::tempdir().expect("a temp dir");
        std::fs::create_dir_all(dir.path().join(".git")).expect("the outer repository");
        repo(dir.path(), "vendored");
        repo(dir.path(), "also-vendored");
        assert!(crowded_notice(dir.path()).is_none());
    }

    #[test]
    fn an_empty_or_unreadable_directory_is_not_an_error() {
        let dir = tempfile::tempdir().expect("a temp dir");
        assert!(crowded_notice(dir.path()).is_none());
        assert_eq!(repositories(Path::new("nowhere-at-all")), 0);
    }
}
