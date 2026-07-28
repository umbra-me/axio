//! `read`: the only tool here that does not mutate.
//!
//! `plan` resolves the path and reads what it needs to build the preview, and
//! never mutates anything — a preview that changes the thing being previewed is
//! how you get a diff that no longer matches by the time it is approved.

use std::path::PathBuf;

use axio_core::protocol::Preview;
use axio_core::tool::{Effects, Plan, Tool, ToolCx, ToolError, ToolOutput};
use serde_json::Value;

use crate::schema;

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
        // The spill directory is outside the workspace, and the truncation
        // marker tells the model to read a file in it by absolute path.
        let path = cx.workspace.resolve_readable(rel)?;
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
