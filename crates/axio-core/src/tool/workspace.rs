//! The only path constructor in axio.
//!
//! Lexical rejection first, then a walk of every component looking for a
//! symlink that leaves the root. Both, in that order: canonicalising first
//! would resolve a link that should have been refused, and a lexical check
//! alone would miss one pointing out.

use super::*;

/// The only path constructor in axio.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    root: PathBuf,
    /// Absolute directories a **read** may reach outside the root.
    ///
    /// One caller and one purpose: the spill directory, where the loop parks
    /// output too large to send. Without this the truncation marker names a
    /// path and then the only tool that could open it refuses every absolute
    /// path, so the model's documented way to recover the rest of a large
    /// output does not exist.
    readable: Vec<PathBuf>,
}

impl Workspace {
    /// The root is canonicalised once, so every later comparison is against a
    /// real path rather than a spelling of one.
    pub fn new(root: impl AsRef<Path>) -> std::io::Result<Self> {
        Ok(Self {
            root: root.as_ref().canonicalize()?,
            readable: Vec::new(),
        })
    }

    /// Let reads reach one directory outside the root. Reads only — `resolve`,
    /// which every write goes through, is untouched.
    ///
    /// Both spellings of the directory are kept. The path the model is given
    /// comes from the truncation marker, which prints the directory as
    /// configured, while `canonicalize` returns the real one — and on macOS
    /// those differ (`/var` is a symlink to `/private/var`, and the state
    /// directory lives under the temp directory), as they do on Windows, where
    /// canonicalising adds a `\\?\` prefix. Matching only the canonical form
    /// refuses the exact path axio itself just told the model to read.
    pub fn with_readable(mut self, dir: impl AsRef<Path>) -> Self {
        let dir = dir.as_ref();
        // It may not exist yet; the lexical prefix check is what matters, and
        // opening a path that is not there fails on its own terms.
        if let Ok(real) = dir.canonicalize()
            && real != dir
        {
            self.readable.push(real);
        }
        self.readable.push(dir.to_path_buf());
        self
    }

    /// A root that could not be canonicalised — a deleted or unreadable
    /// directory.
    ///
    /// The lexical half of `resolve` still applies, so `..` and absolute paths
    /// are still refused; only the symlink check degrades, because there is
    /// nothing to resolve against. Callers should prefer [`Workspace::new`] and
    /// treat this as the last resort it is.
    pub fn unchecked(root: PathBuf) -> Self {
        Self {
            root,
            readable: Vec::new(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// [`Workspace::resolve`], plus the directories [`Workspace::with_readable`]
    /// opened up. For reads; nothing that writes may call this.
    ///
    /// An absolute path is accepted only when it is lexically inside a readable
    /// root **and** contains no `..`, so the prefix cannot be walked back out
    /// of.
    pub fn resolve_readable(&self, p: &str) -> Result<PathBuf, ToolError> {
        let candidate = Path::new(p);
        if candidate.is_absolute()
            && !candidate
                .components()
                .any(|c| matches!(c, Component::ParentDir))
            && self.readable.iter().any(|dir| candidate.starts_with(dir))
        {
            return Ok(candidate.to_path_buf());
        }
        self.resolve(p)
    }

    /// Lexically reject `..`, absolute paths, UNC and drive-relative forms
    /// first, then canonicalise to close the symlink escape. Both steps, in that
    /// order — canonicalising first would resolve a symlink we should have
    /// refused, and rejecting only lexically would miss a link pointing out.
    pub fn resolve(&self, p: &str) -> Result<PathBuf, ToolError> {
        if p.is_empty() {
            return Err(ToolError::BadInput("empty path".into()));
        }
        let candidate = Path::new(p);

        if candidate.is_absolute() {
            return Err(ToolError::BadInput(format!(
                "path must be relative to the workspace root: {p}"
            )));
        }
        // Windows drive-relative (`C:foo`) and UNC (`\\server\share`) forms are
        // not `is_absolute()` on unix, so reject their spellings explicitly.
        if p.starts_with(r"\\") || p.starts_with("//") {
            return Err(ToolError::BadInput(format!("UNC paths are refused: {p}")));
        }
        if p.len() >= 2 && p.as_bytes()[1] == b':' && p.as_bytes()[0].is_ascii_alphabetic() {
            return Err(ToolError::BadInput(format!(
                "drive-relative paths are refused: {p}"
            )));
        }
        for component in candidate.components() {
            match component {
                Component::ParentDir => {
                    return Err(ToolError::BadInput(format!(
                        "path escapes the workspace root: {p}"
                    )));
                }
                Component::RootDir | Component::Prefix(_) => {
                    return Err(ToolError::BadInput(format!(
                        "path must be relative to the workspace root: {p}"
                    )));
                }
                _ => {}
            }
        }

        let joined = self.root.join(candidate);

        // Walk every component from the root. Checking only the deepest
        // *existing* ancestor is not enough: a symlink whose target does not
        // exist yet fails to canonicalise, so an ancestor-only check treats it
        // as "a path being created" and falls back to the root, which trivially
        // passes — and the write then follows the link out of the workspace.
        // A dangling link is still a real directory entry.
        let mut current = self.root.clone();
        for component in candidate.components() {
            let Component::Normal(name) = component else {
                // CurDir is harmless; everything else was rejected above.
                continue;
            };
            current.push(name);

            let meta = match std::fs::symlink_metadata(&current) {
                Ok(meta) => meta,
                // This component does not exist, so neither do any below it,
                // and a path that does not exist cannot be a symlink.
                Err(_) => break,
            };

            if meta.file_type().is_symlink() {
                let target = std::fs::read_link(&current).map_err(|e| {
                    ToolError::BadInput(format!("cannot read symlink {}: {e}", current.display()))
                })?;
                let absolute = if target.is_absolute() {
                    target
                } else {
                    current.parent().unwrap_or(&self.root).join(target)
                };
                // The target may not exist yet either, so canonicalise as far
                // as it goes and judge that.
                let real = deepest_existing(&absolute).ok_or_else(|| {
                    ToolError::BadInput(format!("cannot resolve symlink target for: {p}"))
                })?;
                if !real.starts_with(&self.root) {
                    return Err(ToolError::BadInput(format!(
                        "path escapes the workspace root via a symlink: {p}"
                    )));
                }
            }
        }

        // Belt and braces: the deepest existing ancestor of the whole path must
        // also land inside the root.
        let real = deepest_existing(&joined)
            .ok_or_else(|| ToolError::BadInput(format!("cannot resolve path: {p}")))?;
        if !real.starts_with(&self.root) {
            return Err(ToolError::BadInput(format!(
                "path escapes the workspace root: {p}"
            )));
        }
        Ok(joined)
    }
}

/// Canonicalise the deepest ancestor of `path` that exists.
///
/// Returns `None` only if nothing in the chain resolves, which means the path
/// is not judgeable and must be refused rather than assumed safe.
fn deepest_existing(path: &Path) -> Option<PathBuf> {
    let mut probe = path;
    loop {
        if let Ok(real) = probe.canonicalize() {
            return Some(real);
        }
        probe = probe.parent()?;
    }
}
