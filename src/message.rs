use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum Message {
    User { content: Vec<Block> },
    Assistant { content: Vec<Block> },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    Text {
        text: String,
    },
    ToolCall {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        id: String,
        output: String,
        #[serde(default)]
        is_error: bool,
    },
    /// A provider-specific block (Anthropic thinking) that must be sent back
    /// byte-for-byte to the provider that produced it and dropped for others.
    Opaque {
        provider: String,
        value: Value,
    },
}

impl Message {
    pub fn user_text(text: impl Into<String>) -> Self {
        Message::User {
            content: vec![Block::Text { text: text.into() }],
        }
    }

    pub fn tool_calls(&self) -> Vec<(&str, &str, &Value)> {
        match self {
            Message::Assistant { content } => content
                .iter()
                .filter_map(|b| match b {
                    Block::ToolCall { id, name, input } => {
                        Some((id.as_str(), name.as_str(), input))
                    }
                    _ => None,
                })
                .collect(),
            Message::User { .. } => Vec::new(),
        }
    }

    pub fn text(&self) -> String {
        let content = match self {
            Message::User { content } | Message::Assistant { content } => content,
        };
        content
            .iter()
            .filter_map(|b| match b {
                Block::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }
}
