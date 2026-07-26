//! `read`, `write` and `edit`.
//!
//! Each one follows the same shape: `plan` resolves the path, reads whatever it
//! needs to build a preview, and hands `run` a payload. `plan` never mutates
//! anything — a preview that changes the thing being previewed is how you get a
//! diff that no longer matches by the time it is approved.

use std::path::PathBuf;

use axio_core::protocol::Preview;
use axio_core::tool::{Effects, Plan, Tool, ToolCx, ToolError, ToolOutput};
use serde_json::Value;

use crate::schema;

const WRITE_EFFECTS: Effects = Effects {
    reads: true,
    writes: true,
    executes: false,
    network: false,
};

// ---------------------------------------------------------------- read

pub struct Read {
    schema: Value,
}

impl Default for Read {
    fn default() -> Self {
        Self::new()
    }
}

impl Read {
    pub fn new() -> Self {
        Self {
            schema: schema::object(
                &[
                    (
                        "path",
                        schema::string("Path relative to the workspace root"),
                    ),
                    ("offset", schema::integer("First line to return, 1-based")),
                    ("limit", schema::integer("How many lines to return")),
                ],
                &["path"],
            ),
        }
    }
}

struct ReadPlan {
    path: PathBuf,
    offset: usize,
    limit: usize,
}

#[async_trait::async_trait]
impl Tool for Read {
    fn name(&self) -> &str {
        "read"
    }
    fn description(&self) -> &str {
        include_str!("read.md")
    }
    fn schema(&self) -> &Value {
        &self.schema
    }

    async fn plan(&self, args: &Value, cx: &ToolCx) -> Result<Plan, ToolError> {
        let rel = schema::str_arg(args, "path")?;
        let path = cx.workspace.resolve(rel)?;
        Ok(Plan::new(format!("read:{rel}"), Effects::READ_ONLY)
            .with_preview(Preview::Text {
                text: format!("read {rel}"),
            })
            .with_payload(ReadPlan {
                path,
                offset: schema::usize_arg(args, "offset").unwrap_or(1).max(1),
                limit: schema::usize_arg(args, "limit").unwrap_or(2_000),
            }))
    }

    async fn run(&self, plan: Plan, cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        let p = *plan.take_payload::<ReadPlan>()?;

        let meta = tokio::fs::metadata(&p.path)
            .await
            .map_err(|e| ToolError::Failed(format!("{}: {e}", p.path.display())))?;
        if meta.is_dir() {
            return Err(ToolError::BadInput(format!(
                "{} is a directory; use glob to list it",
                p.path.display()
            )));
        }
        if meta.len() > cx.limits.max_file_bytes {
            return Err(ToolError::Failed(format!(
                "{} is {} bytes, over the {} byte limit; read it in ranges with offset and limit",
                p.path.display(),
                meta.len(),
                cx.limits.max_file_bytes
            )));
        }

        let body = tokio::fs::read(&p.path)
            .await
            .map_err(|e| ToolError::Failed(format!("{}: {e}", p.path.display())))?;
        let text = match String::from_utf8(body) {
            Ok(text) => text,
            // A binary file is a legitimate thing to try to read; saying so is
            // more useful than a decoding error the model cannot act on.
            Err(_) => {
                return Ok(ToolOutput::text(format!(
                    "{} is not valid UTF-8 ({} bytes); it looks like a binary file",
                    p.path.display(),
                    meta.len()
                )));
            }
        };

        let numbered: String = text
            .lines()
            .enumerate()
            .skip(p.offset - 1)
            .take(p.limit)
            .map(|(i, line)| format!("{:>6}\t{line}\n", i + 1))
            .collect();

        if numbered.is_empty() {
            let total = text.lines().count();
            return Ok(ToolOutput::text(format!(
                "no lines in range (the file has {total} line(s))"
            )));
        }
        Ok(ToolOutput::text(numbered))
    }
}

// ---------------------------------------------------------------- write

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
