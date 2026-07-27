//! `plan` / `run`, and a closed context.
//!
//! `Tool::run` is the only execution path in axio. Approval is a pre-flight over
//! the `Plan`, never an interception of a named tool inside the loop — so what
//! was previewed is exactly what is applied, and there is no second code path to
//! keep in step.

mod plan;
mod workspace;

pub use plan::{Effects, Plan, ToolOutput};
pub use workspace::Workspace;

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

impl std::fmt::Debug for Plan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Plan")
            .field("subject", &self.subject)
            .field("effects", &self.effects)
            .field("preview", &self.preview)
            .finish_non_exhaustive()
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

/// Reject arguments a tool's schema does not declare.
///
/// Every schema says `additionalProperties: false`, and nothing enforced it. A
/// model that invents a parameter — `{"command": "...", "yes": true}`, a
/// self-approval argument it hoped existed — planned and ran without
/// contradiction, and went on sending it for the rest of the session. A
/// declared contract nobody checks cannot correct a misunderstanding; one
/// error ends it.
///
/// Applied in the loop rather than in each tool, for the same reason output
/// capping is: a new tool inherits it without knowing it exists.
pub fn reject_unknown_arguments(
    args: &serde_json::Value,
    schema: &serde_json::Value,
    tool: &str,
) -> Result<(), ToolError> {
    let (Some(given), Some(declared)) = (
        args.as_object(),
        schema.get("properties").and_then(|p| p.as_object()),
    ) else {
        return Ok(());
    };

    let unknown: Vec<&str> = given
        .keys()
        .filter(|k| !declared.contains_key(*k))
        .map(String::as_str)
        .collect();
    if unknown.is_empty() {
        return Ok(());
    }

    Err(ToolError::BadInput(format!(
        "unknown argument{} {} — `{tool}` takes {}",
        if unknown.len() == 1 { "" } else { "s" },
        unknown
            .iter()
            .map(|k| format!("`{k}`"))
            .collect::<Vec<_>>()
            .join(", "),
        declared
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    )))
}

/// Reject a call that leaves out an argument the schema requires.
///
/// The mirror of [`reject_unknown_arguments`], and it exists because the two
/// halves were not saying the same quality of thing. An unrecognised argument
/// produced "unknown argument `cmd` — `bash` takes command, timeout_secs",
/// which is enough to fix the call. A missing one produced "`path` is required
/// and must be a string", from deep inside whichever tool happened to ask for
/// it first: no tool named, nothing else it takes, and no way to tell which of
/// several calls in flight it belonged to. A model that has just guessed wrong
/// needs the same help either way.
pub fn reject_missing_arguments(
    args: &serde_json::Value,
    schema: &serde_json::Value,
    tool: &str,
) -> Result<(), ToolError> {
    let (Some(given), Some(required)) = (
        args.as_object(),
        schema.get("required").and_then(|r| r.as_array()),
    ) else {
        return Ok(());
    };

    let missing: Vec<&str> = required
        .iter()
        .filter_map(serde_json::Value::as_str)
        .filter(|k| !given.contains_key(*k))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }

    let declared = schema
        .get("properties")
        .and_then(|p| p.as_object())
        .map(|p| p.keys().map(String::as_str).collect::<Vec<_>>().join(", "))
        .unwrap_or_default();

    Err(ToolError::BadInput(format!(
        "missing required argument{} {} — `{tool}` takes {declared}",
        if missing.len() == 1 { "" } else { "s" },
        missing
            .iter()
            .map(|k| format!("`{k}`"))
            .collect::<Vec<_>>()
            .join(", "),
    )))
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

    /// The truncation marker names the spill file by absolute path and tells
    /// the model to read it. Every absolute path was refused, so the documented
    /// way to recover a large output did not exist — the model either burned
    /// steps guessing or escaped to `bash`.
    #[test]
    fn a_read_reaches_the_spill_directory_and_nothing_else() {
        let (_dir, ws) = workspace();
        let spill = tempdir::TempDir::new();
        std::fs::write(spill.path().join("out.txt"), "the rest").unwrap();
        let ws = ws.with_readable(spill.path());

        // Both spellings, because they differ on macOS and on Windows and the
        // one the model is handed is the uncanonicalised one.
        for base in [
            spill.path().to_path_buf(),
            spill.path().canonicalize().unwrap(),
        ] {
            let named = base.join("out.txt");
            let resolved = ws
                .resolve_readable(&named.display().to_string())
                .unwrap_or_else(|e| panic!("{} is not reachable: {e}", named.display()));
            assert_eq!(std::fs::read_to_string(resolved).unwrap(), "the rest");
        }
        let named = spill.path().join("out.txt");

        // Everything else absolute is still refused, and the prefix cannot be
        // walked back out of.
        assert!(ws.resolve_readable("/etc/passwd").is_err());
        let escape = format!("{}/../../etc/passwd", spill.path().display());
        assert!(ws.resolve_readable(&escape).is_err());
        // And a write, which does not go through this door, is refused too.
        assert!(ws.resolve(&named.display().to_string()).is_err());
    }

    /// Regression. Every schema says `additionalProperties: false` and nothing
    /// checked it, so a model that invented `"yes": true` — hoping for a
    /// self-approval argument — was never contradicted and kept sending it.
    #[test]
    fn an_argument_the_schema_does_not_declare_is_refused_by_name() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "command": {}, "timeout_secs": {} },
        });
        let err = reject_unknown_arguments(
            &serde_json::json!({"command": "ls", "yes": true}),
            &schema,
            "bash",
        )
        .unwrap_err();
        let text = err.to_string();
        assert!(text.contains("`yes`"), "{text}");
        assert!(
            text.contains("command"),
            "it must say what is allowed: {text}"
        );

        assert!(
            reject_unknown_arguments(&serde_json::json!({"command": "ls"}), &schema, "bash")
                .is_ok()
        );
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

#[cfg(test)]
mod missing_argument_tests {
    use super::*;

    fn schema() -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string"},
                "glob": {"type": "string"},
                "hidden": {"type": "boolean"},
            },
            "required": ["pattern"],
        })
    }

    #[test]
    fn a_missing_argument_names_the_tool_and_what_it_takes() {
        // The message a model gets has to be enough to fix the call without
        // guessing which tool asked or what else it wanted.
        let err = reject_missing_arguments(&serde_json::json!({"glob": "*.rs"}), &schema(), "grep")
            .expect_err("a missing required argument");
        let text = err.to_string();
        assert!(text.contains("`pattern`"), "{text}");
        assert!(text.contains("`grep`"), "{text}");
        assert!(text.contains("pattern, glob, hidden"), "{text}");
    }

    #[test]
    fn a_complete_call_passes() {
        assert!(
            reject_missing_arguments(&serde_json::json!({"pattern": "x"}), &schema(), "grep")
                .is_ok()
        );
    }

    #[test]
    fn a_schema_without_required_arguments_rejects_nothing() {
        let open = serde_json::json!({"type": "object", "properties": {}});
        assert!(reject_missing_arguments(&serde_json::json!({}), &open, "anything").is_ok());
    }
}
