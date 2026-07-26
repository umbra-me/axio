//! `plan` / `run`, and a closed context.
//!
//! `Tool::run` is the only execution path in axio. Approval is a pre-flight over
//! the `Plan`, never an interception of a named tool inside the loop — so what
//! was previewed is exactly what is applied, and there is no second code path to
//! keep in step.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::protocol::{Delta, ItemId, Preview};

#[async_trait::async_trait]
pub trait Tool: Send + Sync + 'static {
    /// `&str`, not `&'static str`, and the registry is keyed by `String` — so a
    /// tool can be registered at runtime later without a trait-wide change.
    fn name(&self) -> &str;
    /// Typically `include_str!` of a sibling `.md` file, so prompt tuning is not
    /// a code change.
    fn description(&self) -> &str;
    /// Deterministic bytes. Asserted stable by a test that serialises the whole
    /// tool set repeatedly and compares.
    fn schema(&self) -> &serde_json::Value;

    /// Pure pre-flight. Async because it reads the filesystem to build a diff,
    /// and doing that on a sync method would block a tokio worker — including
    /// the in-flight SSE stream — on every write approval.
    ///
    /// Must not mutate anything.
    async fn plan(&self, args: &serde_json::Value, cx: &ToolCx) -> Result<Plan, ToolError>;

    /// The only place side effects happen. Takes the `Plan` back, so what was
    /// previewed and approved is what is applied.
    async fn run(&self, plan: Plan, cx: &ToolCx) -> Result<ToolOutput, ToolError>;
}

pub struct Plan {
    /// Canonical policy match key: `bash:git`, `edit:src/lib.rs`, `read:.env`.
    pub subject: String,
    pub effects: Effects,
    pub preview: Option<Preview>,
    /// Opaque hand-off from `plan` to `run`. `Box<dyn Any>` rather than a
    /// core-side enum, so adding a tool never edits a type in this crate. The
    /// producer and consumer are the same `&dyn Tool`, so a downcast miss is an
    /// internal bug.
    pub payload: Box<dyn std::any::Any + Send>,
}

impl Plan {
    pub fn new(subject: impl Into<String>, effects: Effects) -> Self {
        Self {
            subject: subject.into(),
            effects,
            preview: None,
            payload: Box::new(()),
        }
    }

    pub fn with_preview(mut self, preview: Preview) -> Self {
        self.preview = Some(preview);
        self
    }

    pub fn with_payload<T: Send + 'static>(mut self, payload: T) -> Self {
        self.payload = Box::new(payload);
        self
    }

    /// Recover the payload a tool's own `plan` produced.
    pub fn take_payload<T: Send + 'static>(self) -> Result<Box<T>, ToolError> {
        self.payload
            .downcast::<T>()
            .map_err(|_| ToolError::Internal("plan payload had an unexpected type".into()))
    }
}

impl std::fmt::Debug for Plan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Plan")
            .field("subject", &self.subject)
            .field("effects", &self.effects)
            .field("preview", &self.preview)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Effects {
    pub reads: bool,
    pub writes: bool,
    pub executes: bool,
    pub network: bool,
}

impl Effects {
    pub const READ_ONLY: Effects = Effects {
        reads: true,
        writes: false,
        executes: false,
        network: false,
    };

    pub fn read_only(&self) -> bool {
        !self.writes && !self.executes && !self.network
    }

    /// Read-only calls run concurrently; everything else runs serially in the
    /// model's call order, so two edits to one path can never race.
    pub fn parallel_safe(&self) -> bool {
        self.read_only()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutput {
    pub content: String,
    pub truncated: bool,
    pub spill: Option<PathBuf>,
}

impl ToolOutput {
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            truncated: false,
            spill: None,
        }
    }
}

/// Five fields. All concrete, none `Option`, none `dyn`.
///
/// Hard rule, enforced by `scripts/limits.sh`: this struct never gains an
/// `Option<Arc<dyn HostThing>>` field. If a tool needs something not here, the
/// tool does not ship.
pub struct ToolCx {
    pub workspace: Arc<Workspace>,
    pub cancel: CancellationToken,
    pub progress: ProgressSink,
    pub limits: ToolLimits,
    pub env: Arc<ToolEnv>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolLimits {
    pub max_output_bytes: usize,
    pub timeout: std::time::Duration,
    pub max_file_bytes: u64,
}

impl Default for ToolLimits {
    fn default() -> Self {
        Self {
            max_output_bytes: 64 * 1024,
            timeout: std::time::Duration::from_secs(120),
            max_file_bytes: 8 * 1024 * 1024,
        }
    }
}

/// The sanitised child environment. Built once; a credential never reaches it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolEnv {
    pub vars: Vec<(String, String)>,
}

/// `try_send` only: a full channel drops an update rather than stalling a tool.
#[derive(Clone)]
pub struct ProgressSink {
    tx: Option<mpsc::Sender<(ItemId, Delta)>>,
}

impl ProgressSink {
    pub fn new(tx: mpsc::Sender<(ItemId, Delta)>) -> Self {
        Self { tx: Some(tx) }
    }

    /// A sink that discards everything, for tests and non-streaming callers.
    pub fn null() -> Self {
        Self { tx: None }
    }

    pub fn send(&self, item: ItemId, delta: Delta) {
        if let Some(tx) = &self.tx {
            let _ = tx.try_send((item, delta));
        }
    }
}

impl std::fmt::Debug for ProgressSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProgressSink")
    }
}

/// The only path constructor in axio.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    root: PathBuf,
}

impl Workspace {
    /// The root is canonicalised once, so every later comparison is against a
    /// real path rather than a spelling of one.
    pub fn new(root: impl AsRef<Path>) -> std::io::Result<Self> {
        Ok(Self {
            root: root.as_ref().canonicalize()?,
        })
    }

    /// A root that could not be canonicalised — a deleted or unreadable
    /// directory.
    ///
    /// The lexical half of `resolve` still applies, so `..` and absolute paths
    /// are still refused; only the symlink check degrades, because there is
    /// nothing to resolve against. Callers should prefer [`Workspace::new`] and
    /// treat this as the last resort it is.
    pub fn unchecked(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
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

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("invalid arguments: {0}")]
    BadInput(String),
    #[error("{0}")]
    Failed(String),
    #[error("cancelled")]
    Cancelled,
    #[error("internal: {0}")]
    Internal(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> (tempdir::TempDir, Workspace) {
        let dir = tempdir::TempDir::new();
        let ws = Workspace::new(dir.path()).unwrap();
        (dir, ws)
    }

    /// A dependency-free temp directory, so the core crate stays dependency-free
    /// in its dev-dependencies too.
    mod tempdir {
        use std::path::{Path, PathBuf};
        use std::sync::atomic::{AtomicU32, Ordering};

        static COUNTER: AtomicU32 = AtomicU32::new(0);

        pub struct TempDir(PathBuf);

        impl TempDir {
            #[allow(clippy::new_without_default)]
            pub fn new() -> Self {
                let n = COUNTER.fetch_add(1, Ordering::Relaxed);
                let path = std::env::temp_dir().join(format!(
                    "axio-test-{}-{}-{n}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                ));
                std::fs::create_dir_all(&path).unwrap();
                Self(path)
            }
            pub fn path(&self) -> &Path {
                &self.0
            }
        }

        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    #[test]
    fn accepts_a_path_inside_the_root() {
        let (dir, ws) = workspace();
        std::fs::write(dir.path().join("a.rs"), "fn main() {}").unwrap();
        let resolved = ws.resolve("a.rs").unwrap();
        assert!(resolved.ends_with("a.rs"));
    }

    #[test]
    fn accepts_a_path_that_does_not_exist_yet() {
        let (_dir, ws) = workspace();
        assert!(ws.resolve("new/nested/file.rs").is_ok());
    }

    #[test]
    fn rejects_parent_traversal() {
        let (_dir, ws) = workspace();
        assert!(ws.resolve("../etc/passwd").is_err());
        assert!(ws.resolve("a/../../etc/passwd").is_err());
    }

    #[test]
    fn rejects_absolute_paths() {
        let (_dir, ws) = workspace();
        assert!(ws.resolve("/etc/passwd").is_err());
    }

    #[test]
    fn rejects_windows_spellings() {
        let (_dir, ws) = workspace();
        assert!(ws.resolve(r"C:\Windows\system32").is_err());
        assert!(ws.resolve(r"C:temp").is_err());
        assert!(ws.resolve(r"\\server\share\x").is_err());
    }

    #[test]
    fn rejects_empty() {
        let (_dir, ws) = workspace();
        assert!(ws.resolve("").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_symlink_pointing_outside_the_root() {
        let (dir, ws) = workspace();
        let outside = std::env::temp_dir().join(format!("axio-outside-{}", std::process::id()));
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret"), "s").unwrap();
        std::os::unix::fs::symlink(&outside, dir.path().join("link")).unwrap();

        assert!(
            ws.resolve("link/secret").is_err(),
            "a symlink out of the root must be refused after canonicalisation"
        );
        let _ = std::fs::remove_dir_all(&outside);
    }

    /// Regression. A symlink whose target does not exist yet cannot be
    /// canonicalised, so a check that looks only at the deepest *existing*
    /// ancestor falls back to the root, passes, and hands back a path that a
    /// write then follows straight out of the workspace. The link is a real
    /// directory entry even though its target is not.
    #[cfg(unix)]
    #[test]
    fn rejects_a_dangling_symlink_pointing_outside_the_root() {
        let (dir, ws) = workspace();
        let outside = std::env::temp_dir().join(format!(
            "axio-dangle-{}-{:?}",
            std::process::id(),
            dir.path().file_name().unwrap()
        ));
        std::fs::create_dir_all(&outside).unwrap();
        let target = outside.join("does-not-exist-yet");
        std::os::unix::fs::symlink(&target, dir.path().join("link")).unwrap();

        let resolved = ws.resolve("link");
        assert!(
            resolved.is_err(),
            "a dangling symlink out of the root must be refused; got {resolved:?}"
        );
        assert!(
            !target.exists(),
            "nothing should have been created outside the root"
        );

        // A dangling symlink pointing *inside* the root is still fine.
        std::os::unix::fs::symlink(dir.path().join("later.rs"), dir.path().join("inner")).unwrap();
        assert!(ws.resolve("inner").is_ok());

        let _ = std::fs::remove_dir_all(&outside);
    }

    /// The same escape one level up: a dangling directory link with a child.
    #[cfg(unix)]
    #[test]
    fn rejects_a_child_of_a_dangling_directory_symlink() {
        let (dir, ws) = workspace();
        let outside = std::env::temp_dir().join(format!("axio-dangle2-{}", std::process::id()));
        std::os::unix::fs::symlink(outside.join("nodir"), dir.path().join("dlink")).unwrap();
        assert!(ws.resolve("dlink/child").is_err());
    }

    #[test]
    fn effects_gate_parallelism() {
        assert!(Effects::READ_ONLY.parallel_safe());
        let write = Effects {
            reads: true,
            writes: true,
            executes: false,
            network: false,
        };
        assert!(!write.parallel_safe());
        assert!(!write.read_only());
    }

    #[test]
    fn payload_round_trips_and_a_miss_is_internal() {
        let plan = Plan::new("read:a.rs", Effects::READ_ONLY).with_payload(42u32);
        let got = plan.take_payload::<u32>().unwrap();
        assert_eq!(*got, 42);

        let plan = Plan::new("read:a.rs", Effects::READ_ONLY).with_payload(42u32);
        let err = plan.take_payload::<String>().unwrap_err();
        assert!(matches!(err, ToolError::Internal(_)));
    }

    #[test]
    fn a_full_progress_channel_drops_rather_than_stalls() {
        let (tx, rx) = mpsc::channel(1);
        let sink = ProgressSink::new(tx);
        // Second send has nowhere to go; must not panic or block.
        sink.send(ItemId::nil(), Delta::Text { text: "one".into() });
        sink.send(ItemId::nil(), Delta::Text { text: "two".into() });
        drop(rx);
        sink.send(
            ItemId::nil(),
            Delta::Text {
                text: "after close".into(),
            },
        );
    }
}
