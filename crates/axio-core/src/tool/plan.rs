//! What a tool intends to do, before it is allowed to do it.
//!
//! The plan carries the policy subject, the effects, the preview shown for
//! approval, and an opaque payload only its own tool can read back — so what
//! was previewed is exactly what runs.

use super::*;

pub struct Plan {
    /// Canonical policy match key: `bash:git`, `edit:src/lib.rs`, `read:.env`.
    pub subject: String,
    pub effects: Effects,
    pub preview: Option<Preview>,
    /// Concrete paths this action would touch, when the subject cannot carry
    /// them.
    ///
    /// `read:.env` says everything about itself; `bash:cat` does not, and the
    /// built-in deny list matches on paths. Without these, `cat .env` is
    /// classified as `cat` and a list written to stop `.env` reaching the model
    /// can never fire on the one tool that can read any byte on the machine.
    pub paths: Vec<String>,
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
            paths: Vec::new(),
            payload: Box::new(()),
        }
    }

    pub fn with_preview(mut self, preview: Preview) -> Self {
        self.preview = Some(preview);
        self
    }

    /// Declare the paths the built-in deny list should test, for a subject that
    /// cannot express them itself.
    pub fn with_paths(mut self, paths: Vec<String>) -> Self {
        self.paths = paths;
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
