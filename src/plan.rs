//! The `plan` module's TodoWrite tool: the model keeps a short task list,
//! which the terminal shows after each update.

use serde_json::{Value, json};

pub fn spec() -> crate::tools::ToolSpec {
    crate::tools::ToolSpec {
        name: "TodoWrite".into(),
        description: "Keep a short plan for multi-step work. Send the whole list each time; mark one item in_progress while you work on it and completed as soon as it is done. Skip it for simple tasks.".into(),
        schema: json!({
            "type": "object",
            "properties": {
                "todos": {"type": "array", "items": {
                    "type": "object",
                    "properties": {
                        "content": {"type": "string"},
                        "status": {"type": "string", "enum": ["pending", "in_progress", "completed"]},
                        "activeForm": {"type": "string"}
                    },
                    "required": ["content", "status"]
                }}
            },
            "required": ["todos"],
            "additionalProperties": false
        }),
    }
}

/// Validates the list and renders it for the model and the terminal.
pub fn write(input: &Value) -> Result<String, String> {
    let todos = input["todos"].as_array().ok_or("todos must be a list")?;
    let mut lines = Vec::new();
    for t in todos {
        let content = t["content"].as_str().ok_or("each todo needs content")?;
        let mark = match t["status"].as_str() {
            Some("completed") => "[x]",
            Some("in_progress") => "[>]",
            Some("pending") => "[ ]",
            other => return Err(format!("unknown todo status: {other:?}")),
        };
        lines.push(format!("{mark} {content}"));
    }
    if lines.is_empty() {
        return Ok("plan cleared".into());
    }
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_and_validates() {
        let plan = json!({"todos": [
            {"content": "read the code", "status": "completed"},
            {"content": "fix the bug", "status": "in_progress", "activeForm": "Fixing the bug"},
            {"content": "run tests", "status": "pending"}
        ]});
        assert_eq!(
            write(&plan).unwrap(),
            "[x] read the code\n[>] fix the bug\n[ ] run tests"
        );
        assert!(write(&json!({"todos": [{"content": "x", "status": "done"}]})).is_err());
        assert_eq!(write(&json!({"todos": []})).unwrap(), "plan cleared");
    }
}
