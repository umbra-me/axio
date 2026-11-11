//! Keep-tail compaction: older turns become a summary, recent turns stay.
//!
//! Anthropic binds each thinking block to the history before it, so retained
//! turns lose their thinking blocks; text and tool calls stay. Compaction only
//! runs between prompts, never inside a tool round.

use crate::message::{Block, Message};

/// Roughly how many tokens of recent conversation stay verbatim.
pub const KEEP_RECENT_TOKENS: usize = 20_000;
const RESULT_CHARS: usize = 2_000;

pub const INSTRUCTION: &str = "Summarize the transcript inside <summary></summary> tags. Include relevant information in the summary such that this conversation will be continued by a new context window without needing to redo work or be reprovided with relevant constraints or context. Be sure to preserve: (1) any difficulties or problems that came up, and how they were handled or resolved; (2) any possibilities, options, or approaches that were raised, tried, or set aside, and why; (3) anything that was asked for, decided, agreed, ruled out, or established as a preference, constraint, or boundary - stated exactly; (4) exactly where things stand now - what has been covered, settled, or completed so far; (5) anything still open, unresolved, promised, or expected to happen next; (6) specific details that would be hard to reconstruct - names, numbers, dates, exact wording, links or references - kept exactly. Be complete on these even at the cost of length; keep everything else concise. Weight the two voices differently: keep what the user said, asked for, shared, or established carefully and close to their own words; your own explanations and reasoning can be condensed much further, to what they concluded or produced - as long as nothing in the six items above is dropped. Do not call any tools while writing this summary; respond with text only.";

fn estimate(m: &Message) -> usize {
    serde_json::to_string(m).map_or(0, |s| s.len() / 4)
}

fn is_prompt(m: &Message) -> bool {
    matches!(m, Message::User { content }
        if !content.iter().any(|b| matches!(b, Block::ToolResult { .. })))
}

/// The index of the first kept message: a user prompt, so no tool result is
/// separated from its call. `None` when there is nothing older to summarize.
pub fn cut(messages: &[Message], keep_tokens: usize) -> Option<usize> {
    let mut kept = 0;
    let mut target = messages.len();
    for (i, m) in messages.iter().enumerate().rev() {
        kept += estimate(m);
        target = i;
        if kept >= keep_tokens {
            break;
        }
    }
    let at = (target..messages.len())
        .find(|&i| is_prompt(&messages[i]))
        .or_else(|| (1..target).rev().find(|&i| is_prompt(&messages[i])))?;
    (at > 0).then_some(at)
}

/// The older part of the conversation as plain text for the summarizer.
pub fn transcript(messages: &[Message]) -> String {
    let mut out = String::new();
    for m in messages {
        let (who, content) = match m {
            Message::User { content } => ("User", content),
            Message::Assistant { content } => ("Assistant", content),
        };
        for b in content {
            match b {
                Block::Text { text } => out.push_str(&format!("{who}: {text}\n\n")),
                Block::ToolCall { name, input, .. } => {
                    out.push_str(&format!("Assistant called {name}: {input}\n\n"))
                }
                Block::ToolResult {
                    output, is_error, ..
                } => {
                    let mut text: String = output.chars().take(RESULT_CHARS).collect();
                    if text.len() < output.len() {
                        text.push_str(" […]");
                    }
                    let kind = if *is_error { "error" } else { "result" };
                    out.push_str(&format!("Tool {kind}: {text}\n\n"));
                }
                Block::Opaque { .. } => {}
            }
        }
    }
    out
}

pub fn summary_text(reply: &str) -> String {
    let inner = reply
        .split_once("<summary>")
        .map(|(_, rest)| rest.split("</summary>").next().unwrap_or(rest))
        .unwrap_or(reply);
    inner.trim().to_string()
}

/// Replaces `messages[..keep_from]` with the summary. Used live and when a
/// session is loaded, so both produce the same history.
pub fn apply(messages: &mut Vec<Message>, keep_from: usize, summary: &str) {
    let keep_from = keep_from.min(messages.len());
    let mut kept: Vec<Message> = messages.drain(keep_from..).collect();
    let calls: Vec<String> = kept
        .iter()
        .flat_map(|m| m.tool_calls().into_iter().map(|(id, _, _)| id.to_string()))
        .collect();
    for m in &mut kept {
        match m {
            Message::Assistant { content } => {
                content.retain(|b| !matches!(b, Block::Opaque { .. }))
            }
            // A result whose call was summarized away would be rejected.
            Message::User { content } => content
                .retain(|b| !matches!(b, Block::ToolResult { id, .. } if !calls.contains(id))),
        }
    }
    kept.retain(|m| !matches!(m, Message::User { content } if content.is_empty()));
    messages.clear();
    messages.push(Message::user_text(format!(
        "This session was compacted. Summary of the earlier conversation:\n<summary>\n{summary}\n</summary>"
    )));
    for m in kept {
        append(messages, m);
    }
}

/// Appends a message, folding consecutive user messages into one, which an
/// interrupted turn or a compaction can otherwise leave behind.
pub fn append(messages: &mut Vec<Message>, m: Message) {
    if let (Some(Message::User { content: last }), Message::User { content }) =
        (messages.last_mut(), &m)
    {
        last.extend(content.iter().cloned());
        return;
    }
    messages.push(m);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(id: &str) -> Message {
        Message::Assistant {
            content: vec![
                Block::Opaque {
                    provider: "anthropic".into(),
                    value: json!({"type": "thinking"}),
                },
                Block::ToolCall {
                    id: id.into(),
                    name: "bash".into(),
                    input: json!({"command": "ls"}),
                },
            ],
        }
    }

    fn result(id: &str, size: usize) -> Message {
        Message::User {
            content: vec![Block::ToolResult {
                id: id.into(),
                output: "x".repeat(size),
                is_error: false,
            }],
        }
    }

    fn answer() -> Message {
        Message::Assistant {
            content: vec![Block::Text {
                text: "done".into(),
            }],
        }
    }

    fn history() -> Vec<Message> {
        vec![
            Message::user_text("first"),
            call("a"),
            result("a", 40_000),
            answer(),
            Message::user_text("second"),
            call("b"),
            result("b", 40_000),
            answer(),
        ]
    }

    #[test]
    fn cuts_only_at_a_user_prompt() {
        let h = history();
        assert_eq!(cut(&h, 5_000), Some(4));
        assert_eq!(cut(&h, 1), Some(4), "never splits a tool round");
        assert_eq!(cut(&h, 1_000_000), None, "everything fits");
        assert_eq!(cut(&h[..4], 1), None, "a single turn has nothing older");
    }

    #[test]
    fn apply_summarizes_and_strips_retained_thinking() {
        let mut h = history();
        apply(&mut h, 4, "they listed files");
        assert_eq!(h.len(), 4);
        let Message::User { content } = &h[0] else {
            panic!()
        };
        assert_eq!(
            content.len(),
            2,
            "summary and the kept prompt share one message"
        );
        assert!(matches!(&content[0], Block::Text { text } if text.contains("they listed files")));
        assert_eq!(
            content[1],
            Block::Text {
                text: "second".into()
            }
        );
        assert_eq!(h[1].tool_calls().len(), 1);
        let Message::Assistant { content } = &h[1] else {
            panic!()
        };
        assert!(!content.iter().any(|b| matches!(b, Block::Opaque { .. })));
    }

    #[test]
    fn transcript_drops_thinking_and_trims_results() {
        let t = transcript(&history()[..4]);
        assert!(t.starts_with("User: first\n\nAssistant called bash: {\"command\":\"ls\"}"));
        assert!(t.contains("[…]"));
        assert!(t.len() < 3_000);
    }

    #[test]
    fn extracts_summary_tags() {
        assert_eq!(summary_text("x <summary>\n kept \n</summary> y"), "kept");
        assert_eq!(summary_text(" plain "), "plain");
    }
}
