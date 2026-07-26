//! `glob` and `grep`.
//!
//! Both walk the workspace with `.gitignore` respected, because an agent that
//! searches `target/` and `node_modules/` wastes its context on build output
//! and finds the wrong copy of everything.

use axio_core::protocol::Preview;
use axio_core::tool::{Effects, Plan, Tool, ToolCx, ToolError, ToolOutput};
use globset::GlobBuilder;
use serde_json::Value;

use crate::schema;

/// Enough to be useful, few enough to stay readable. A search that would return
/// more than this is a search that needs narrowing, and saying so is more
/// helpful than burying the answer.
const MAX_RESULTS: usize = 500;

// ---------------------------------------------------------------- glob

/// Refuse a pattern that points outside the workspace, the way `read` does.
///
/// The walker is always rooted at the workspace, so an escaping pattern used to
/// come back as a clean "no matches" — a factual-sounding claim the model
/// believes, so it keeps searching instead of changing approach. A false
/// negative costs more than an error, and the two file tools disagreeing about
/// how an out-of-workspace path is reported is its own bug.
fn reject_escaping(pattern: &str) -> Result<(), ToolError> {
    let looks_absolute = pattern.starts_with('/')
        || pattern.starts_with('\\')
        || (pattern.len() >= 2
            && pattern.as_bytes()[1] == b':'
            && pattern.as_bytes()[0].is_ascii_alphabetic());
    if looks_absolute {
        return Err(ToolError::BadInput(format!(
            "pattern must be relative to the workspace root: {pattern}"
        )));
    }
    if pattern.split(['/', '\\']).any(|c| c == "..") {
        return Err(ToolError::BadInput(format!(
            "pattern escapes the workspace root: {pattern}"
        )));
    }
    Ok(())
}

pub struct GlobTool {
    schema: Value,
}

impl Default for GlobTool {
    fn default() -> Self {
        Self::new()
    }
}

impl GlobTool {
    pub fn new() -> Self {
        Self {
            schema: schema::object(
                &[
                    ("pattern", schema::string("A glob such as `src/**/*.rs`")),
                    (
                        "hidden",
                        schema::boolean("Include dot-files and ignored files"),
                    ),
                ],
                &["pattern"],
            ),
        }
    }
}

struct GlobPlan {
    pattern: String,
    hidden: bool,
}

#[async_trait::async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str {
        "glob"
    }
    fn description(&self) -> &str {
        include_str!("glob.md")
    }
    fn schema(&self) -> &Value {
        &self.schema
    }

    async fn plan(&self, args: &Value, _cx: &ToolCx) -> Result<Plan, ToolError> {
        let pattern = schema::str_arg(args, "pattern")?;
        reject_escaping(pattern)?;
        // Compile now so a bad pattern is a planning error rather than an empty
        // result the model reads as "no matches".
        GlobBuilder::new(pattern)
            .literal_separator(true)
            .build()
            .map_err(|e| ToolError::BadInput(format!("bad pattern `{pattern}`: {e}")))?;

        Ok(Plan::new(format!("glob:{pattern}"), Effects::READ_ONLY)
            .with_preview(Preview::Text {
                text: format!("glob {pattern}"),
            })
            .with_payload(GlobPlan {
                pattern: pattern.to_owned(),
                hidden: schema::bool_arg(args, "hidden").unwrap_or(false),
            }))
    }

    async fn run(&self, plan: Plan, cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        let p = *plan.take_payload::<GlobPlan>()?;
        let root = cx.workspace.root().to_path_buf();
        let cancel = cx.cancel.clone();

        let matcher = GlobBuilder::new(&p.pattern)
            .literal_separator(true)
            .build()
            .map_err(|e| ToolError::BadInput(format!("bad pattern: {e}")))?
            .compile_matcher();

        // The walker is blocking, so it goes on the blocking pool rather than
        // stalling a runtime worker — including the one driving the SSE stream.
        let found = tokio::task::spawn_blocking(move || {
            let mut out = Vec::new();
            let walker = ignore::WalkBuilder::new(&root)
                .hidden(!p.hidden)
                .git_ignore(!p.hidden)
                .build();
            for entry in walker {
                if cancel.is_cancelled() {
                    break;
                }
                let Ok(entry) = entry else { continue };
                if !entry.file_type().is_some_and(|t| t.is_file()) {
                    continue;
                }
                let Ok(rel) = entry.path().strip_prefix(&root) else {
                    continue;
                };
                if matcher.is_match(rel) {
                    out.push(rel.to_string_lossy().replace('\\', "/"));
                }
                if out.len() >= MAX_RESULTS {
                    break;
                }
            }
            out.sort();
            out
        })
        .await
        .map_err(|e| ToolError::Internal(format!("walk failed: {e}")))?;

        if cx.cancel.is_cancelled() {
            return Err(ToolError::Cancelled);
        }
        if found.is_empty() {
            return Ok(ToolOutput::text(format!("no files match `{}`", p.pattern)));
        }
        let capped = found.len() >= MAX_RESULTS;
        let mut text = found.join("\n");
        if capped {
            text.push_str(&format!(
                "\n\n[stopped at {MAX_RESULTS} results; narrow the pattern]"
            ));
        }
        Ok(ToolOutput::text(text))
    }
}

// ---------------------------------------------------------------- grep

pub struct Grep {
    schema: Value,
}

impl Default for Grep {
    fn default() -> Self {
        Self::new()
    }
}

impl Grep {
    pub fn new() -> Self {
        Self {
            schema: schema::object(
                &[
                    ("pattern", schema::string("A regular expression")),
                    (
                        "glob",
                        schema::string("Restrict to files matching this glob"),
                    ),
                    (
                        "hidden",
                        schema::boolean("Include dot-files and ignored files"),
                    ),
                ],
                &["pattern"],
            ),
        }
    }
}

struct GrepPlan {
    pattern: String,
    glob: Option<String>,
    hidden: bool,
}

#[async_trait::async_trait]
impl Tool for Grep {
    fn name(&self) -> &str {
        "grep"
    }
    fn description(&self) -> &str {
        include_str!("grep.md")
    }
    fn schema(&self) -> &Value {
        &self.schema
    }

    async fn plan(&self, args: &Value, _cx: &ToolCx) -> Result<Plan, ToolError> {
        let pattern = schema::str_arg(args, "pattern")?;
        let glob = schema::opt_str_arg(args, "glob").map(str::to_owned);
        if let Some(g) = glob.as_deref() {
            reject_escaping(g)?;
        }

        Ok(Plan::new(format!("grep:{pattern}"), Effects::READ_ONLY)
            .with_preview(Preview::Text {
                text: format!("grep {pattern}"),
            })
            .with_payload(GrepPlan {
                pattern: pattern.to_owned(),
                glob,
                hidden: schema::bool_arg(args, "hidden").unwrap_or(false),
            }))
    }

    async fn run(&self, plan: Plan, cx: &ToolCx) -> Result<ToolOutput, ToolError> {
        let p = *plan.take_payload::<GrepPlan>()?;
        let root = cx.workspace.root().to_path_buf();
        let cancel = cx.cancel.clone();
        let max_bytes = cx.limits.max_file_bytes;

        let file_filter = match &p.glob {
            Some(g) => Some(
                GlobBuilder::new(g)
                    .literal_separator(true)
                    .build()
                    .map_err(|e| ToolError::BadInput(format!("bad glob `{g}`: {e}")))?
                    .compile_matcher(),
            ),
            None => None,
        };

        // A plain substring search: a full regular-expression engine is another
        // dependency and another way for a model-authored pattern to run away.
        let needle = p.pattern.clone();

        let hits = tokio::task::spawn_blocking(move || {
            let mut out: Vec<String> = Vec::new();
            let walker = ignore::WalkBuilder::new(&root)
                .hidden(!p.hidden)
                .git_ignore(!p.hidden)
                .build();

            for entry in walker {
                if cancel.is_cancelled() || out.len() >= MAX_RESULTS {
                    break;
                }
                let Ok(entry) = entry else { continue };
                if !entry.file_type().is_some_and(|t| t.is_file()) {
                    continue;
                }
                let Ok(rel) = entry.path().strip_prefix(&root) else {
                    continue;
                };
                if let Some(f) = &file_filter
                    && !f.is_match(rel)
                {
                    continue;
                }
                if entry
                    .metadata()
                    .map(|m| m.len() > max_bytes)
                    .unwrap_or(true)
                {
                    continue;
                }
                let Ok(body) = std::fs::read_to_string(entry.path()) else {
                    continue; // binary or unreadable; not an error worth reporting
                };
                let display = rel.to_string_lossy().replace('\\', "/");
                for (n, line) in body.lines().enumerate() {
                    if line.contains(&needle) {
                        // Long lines are usually minified output; the match
                        // matters, the rest is noise.
                        let shown: String = line.chars().take(200).collect();
                        out.push(format!("{display}:{}: {}", n + 1, shown.trim()));
                        if out.len() >= MAX_RESULTS {
                            break;
                        }
                    }
                }
            }
            out
        })
        .await
        .map_err(|e| ToolError::Internal(format!("search failed: {e}")))?;

        if cx.cancel.is_cancelled() {
            return Err(ToolError::Cancelled);
        }
        if hits.is_empty() {
            return Ok(ToolOutput::text(format!("no matches for `{}`", p.pattern)));
        }
        let capped = hits.len() >= MAX_RESULTS;
        let mut text = hits.join("\n");
        if capped {
            text.push_str(&format!(
                "\n\n[stopped at {MAX_RESULTS} matches; narrow the search]"
            ));
        }
        Ok(ToolOutput::text(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression. The walker is rooted at the workspace, so a pattern pointing
    /// outside it came back as a clean "no matches" — a factual-sounding claim
    /// the model believes, so it keeps searching instead of changing approach.
    /// A false negative costs more than an error.
    #[test]
    fn a_pattern_pointing_outside_the_workspace_is_an_error_not_an_empty_result() {
        for pattern in ["../other/**/*.txt", "/etc/*", "a/../../b", "..\\win\\*"] {
            assert!(
                reject_escaping(pattern).is_err(),
                "{pattern} should have been refused"
            );
        }
    }

    /// And the ordinary patterns still work: a guard that refuses `src/**` is
    /// worse than the bug it fixes.
    #[test]
    fn an_ordinary_pattern_is_accepted() {
        for pattern in ["**/*.rs", "src/**/mod.rs", "a..b/*.txt", "./src/*"] {
            assert!(
                reject_escaping(pattern).is_ok(),
                "{pattern} should have been accepted"
            );
        }
    }
}
