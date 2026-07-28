//! `write` and `edit`: the two that mutate.
//!
//! Both follow the same shape as `read` — `plan` resolves the path, builds the
//! preview and hands `run` a payload — and both carry the same effects, which
//! is what the approver is deciding about.

use std::path::PathBuf;

use axio_core::tool::{Effects, Plan, Tool, ToolCx, ToolError, ToolOutput};
use serde_json::Value;

use crate::schema;

const WRITE_EFFECTS: Effects = Effects {
    reads: true,
    writes: true,
    executes: false,
    network: false,
};

pub struct Write {
    schema: Value,
}

impl Default for Write {
    fn default() -> Self {
        Self::new()
    }
}

impl Write {
    pub fn new() -> Self {
        Self {
            schema: schema::object(
                &[
                    (
                        "path",
                        schema::string("Path relative to the workspace root"),
                    ),
                    ("content", schema::string("The complete new contents")),
                ],
                &["path", "content"],
            ),
        }
    }
}

struct WritePlan {
    path: PathBuf,
    rel: String,
    content: String,
}

#[async_trait::async_trait]
impl Tool for Write {
    fn name(&self) -> &str {
        "write"
    }
    fn description(&self) -> &str {
        include_str!("write.md")
    }
    fn schema(&self) -> &Value {
        &self.schema
    }

    async fn plan(&self, args: &Value, cx: &ToolCx) -> Result<Plan, ToolError> {
        let rel = schema::str_arg(args, "path")?;
        let content = schema::str_arg(args, "content")?;
        let path = cx.workspace.resolve(rel)?;

        // Read-only: the preview is a diff against what is there now, and
        // building it must not touch the file.
        let existing = tokio::fs::read_to_string(&path).await.unwrap_or_default();
        let preview = crate::diff::unified(rel, &existing, content);

        Ok(Plan::new(format!("write:{rel}"), WRITE_EFFECTS)
            .with_preview(preview)
            .with_payload(WritePlan {
                path,
                rel: rel.to_owned(),
                content: content.to_owned(),
            }))
    }

    async fn run(&self, plan: Plan, _cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        let p = *plan.take_payload::<WritePlan>()?;
        if let Some(parent) = p.path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ToolError::Failed(format!("{}: {e}", parent.display())))?;
        }
        tokio::fs::write(&p.path, &p.content)
            .await
            .map_err(|e| ToolError::Failed(format!("{}: {e}", p.path.display())))?;
        Ok(ToolOutput::text(format!(
            "wrote {} ({} bytes)",
            p.rel,
            p.content.len()
        )))
    }
}

// ---------------------------------------------------------------- edit

pub struct Edit {
    schema: Value,
}

impl Default for Edit {
    fn default() -> Self {
        Self::new()
    }
}

impl Edit {
    pub fn new() -> Self {
        Self {
            schema: schema::object(
                &[
                    (
                        "path",
                        schema::string("Path relative to the workspace root"),
                    ),
                    (
                        "old",
                        schema::string("Exact text to replace; must be unique"),
                    ),
                    ("new", schema::string("Replacement text")),
                ],
                &["path", "old", "new"],
            ),
        }
    }
}

struct EditPlan {
    path: PathBuf,
    rel: String,
    updated: String,
}

#[async_trait::async_trait]
impl Tool for Edit {
    fn name(&self) -> &str {
        "edit"
    }
    fn description(&self) -> &str {
        include_str!("edit.md")
    }
    fn schema(&self) -> &Value {
        &self.schema
    }

    async fn plan(&self, args: &Value, cx: &ToolCx) -> Result<Plan, ToolError> {
        let rel = schema::str_arg(args, "path")?;
        let old = schema::str_arg(args, "old")?;
        let new = schema::str_arg(args, "new")?;
        let path = cx.workspace.resolve(rel)?;

        if old.is_empty() {
            return Err(ToolError::BadInput(
                "`old` is empty; use write to create a file".into(),
            ));
        }

        let existing = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| ToolError::Failed(format!("{}: {e}", path.display())))?;

        // Ambiguity is refused rather than guessed: replacing the wrong one of
        // three identical lines is a silent corruption.
        match existing.matches(old).count() {
            0 => {
                return Err(ToolError::BadInput(format!(
                    "`old` does not appear in {rel}"
                )));
            }
            1 => {}
            n => {
                return Err(ToolError::BadInput(format!(
                    "`old` appears {n} times in {rel}; include surrounding lines to make it unique"
                )));
            }
        }

        let updated = existing.replacen(old, new, 1);
        let preview = crate::diff::unified(rel, &existing, &updated);

        Ok(Plan::new(format!("edit:{rel}"), WRITE_EFFECTS)
            .with_preview(preview)
            .with_payload(EditPlan {
                path,
                rel: rel.to_owned(),
                updated,
            }))
    }

    async fn run(&self, plan: Plan, _cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        // The whole updated file was computed during planning, so what is
        // written is exactly what was previewed and approved.
        let p = *plan.take_payload::<EditPlan>()?;
        tokio::fs::write(&p.path, &p.updated)
            .await
            .map_err(|e| ToolError::Failed(format!("{}: {e}", p.path.display())))?;
        Ok(ToolOutput::text(format!("edited {}", p.rel)))
    }
}
