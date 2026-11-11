//! Compaction replaces the whole history with the user's recent requests,
//! verbatim, followed by a summary of everything else.
//!
//! No assistant turn or tool result survives, so no thinking block is replayed
//! against a changed prefix and no tool result loses its call. Compaction only
//! runs between prompts.

use crate::message::{Block, Message};

/// Roughly how many tokens of the user's own recent requests stay verbatim.
pub const KEEP_USER_TOKENS: usize = 20_000;
const RESULT_CHARS: usize = 2_000;
const SUMMARY_MARK: &str = "This session was compacted.";

/// Appended when the user stops a turn, so the model knows its last reply or
/// command was cut short.
pub const INTERRUPTED: &str = "[The user interrupted the previous turn on purpose. Any command that was stopped may have partially run.]";

pub const INSTRUCTION: &str = "Summarize the transcript inside <summary></summary> tags. Include relevant information in the summary such that this conversation will be continued by a new context window without needing to redo work or be reprovided with relevant constraints or context. Be sure to preserve: (1) any difficulties or problems that came up, and how they were handled or resolved; (2) any possibilities, options, or approaches that were raised, tried, or set aside, and why; (3) anything that was asked for, decided, agreed, ruled out, or established as a preference, constraint, or boundary - stated exactly; (4) exactly where things stand now - what has been covered, settled, or completed so far; (5) anything still open, unresolved, promised, or expected to happen next; (6) specific details that would be hard to reconstruct - names, numbers, dates, exact wording, links or references - kept exactly. Be complete on these even at the cost of length; keep everything else concise. Weight the two voices differently: keep what the user said, asked for, shared, or established carefully and close to their own words; your own explanations and reasoning can be condensed much further, to what they concluded or produced - as long as nothing in the six items above is dropped. Do not call any tools while writing this summary; respond with text only.";

/// Whether there is model output worth summarizing.
pub fn worthwhile(messages: &[Message]) -> bool {
    messages
        .iter()
        .any(|m| matches!(m, Message::Assistant { .. }))
}

/// The conversation as plain text for the summarizer.
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

/// The user's own requests, newest last, skipping earlier summaries and
/// interruption notes, cut to about `budget` tokens from the newest back.
fn recent_requests(messages: &[Message], budget: usize) -> Vec<String> {
    let mut picked = Vec::new();
    let mut left = budget * 4;
    let requests = messages.iter().rev().flat_map(|m| match m {
        Message::User { content } => content
            .iter()
            .rev()
            .filter_map(|b| match b {
                Block::Text { text } if !text.starts_with(SUMMARY_MARK) && text != INTERRUPTED => {
                    Some(text.as_str())
                }
                _ => None,
            })
            .collect(),
        Message::Assistant { .. } => Vec::new(),
    });
    for text in requests {
        if left == 0 {
            break;
        }
        if text.len() <= left {
            left -= text.len();
            picked.push(text.to_string());
        } else {
            // Keep the start of an oversized request rather than dropping it.
            let cut: String = text.chars().take(left).collect();
            left = 0;
            picked.push(format!("{cut} […]"));
        }
    }
    picked.reverse();
    picked
}

/// Replaces the history with recent requests and the summary. The session
/// replays this on load, so it must depend only on the messages and summary.
pub fn apply(messages: &mut Vec<Message>, summary: &str) {
    let mut content: Vec<Block> = recent_requests(messages, KEEP_USER_TOKENS)
        .into_iter()
        .map(|text| Block::Text { text })
        .collect();
    content.push(Block::Text {
        text: format!(
            "{SUMMARY_MARK} The user's recent requests are above, verbatim. Summary of the work so far:\n<summary>\n{summary}\n</summary>"
        ),
    });
    messages.clear();
    messages.push(Message::User { content });
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

    fn history() -> Vec<Message> {
        vec![
            Message::user_text("first request"),
            Message::Assistant {
                content: vec![
                    Block::Opaque {
                        provider: "anthropic".into(),
                        value: json!({"type": "thinking"}),
                    },
                    Block::ToolCall {
                        id: "a".into(),
                        name: "bash".into(),
                        input: json!({"command": "ls"}),
                    },
                ],
            },
            Message::User {
                content: vec![Block::ToolResult {
                    id: "a".into(),
                    output: "x".repeat(40_000),
                    is_error: false,
                }],
            },
            Message::Assistant {
                content: vec![Block::Text {
                    text: "done".into(),
                }],
            },
            Message::User {
                content: vec![
                    Block::Text {
                        text: "second request".into(),
                    },
                    Block::Text {
                        text: INTERRUPTED.into(),
                    },
                ],
            },
        ]
    }

    #[test]
    fn keeps_only_user_requests_and_the_summary() {
        let mut h = history();
        apply(&mut h, "they listed files");
        assert_eq!(h.len(), 1);
        let Message::User { content } = &h[0] else {
            panic!()
        };
        let texts: Vec<&str> = content
            .iter()
            .map(|b| match b {
                Block::Text { text } => text.as_str(),
                _ => panic!("only text survives"),
            })
            .collect();
        assert_eq!(texts[..2], ["first request", "second request"]);
        assert!(texts[2].starts_with(SUMMARY_MARK) && texts[2].contains("they listed files"));
    }

    #[test]
    fn a_second_compaction_drops_the_first_summary() {
        let mut h = history();
        apply(&mut h, "one");
        h.push(Message::Assistant {
            content: vec![Block::Text { text: "ok".into() }],
        });
        apply(&mut h, "two");
        assert_eq!(h[0].text().matches(SUMMARY_MARK).count(), 1);
        assert!(h[0].text().contains("two") && !h[0].text().contains("<summary>\none"));
    }

    #[test]
    fn keeps_the_newest_requests_within_budget() {
        let big = "y".repeat(KEEP_USER_TOKENS * 4 - 2);
        let h = vec![
            Message::user_text("oldest"),
            Message::user_text(big.clone()),
            Message::user_text("newest"),
        ];
        let kept = recent_requests(&h, KEEP_USER_TOKENS);
        assert_eq!(kept.len(), 2, "the oldest no longer fits");
        assert_eq!(kept[1], "newest");
        assert!(kept[0].starts_with("yyy") && kept[0].ends_with("[…]"));
    }

    #[test]
    fn transcript_drops_thinking_and_trims_results() {
        let t = transcript(&history()[..4]);
        assert!(
            t.starts_with("User: first request\n\nAssistant called bash: {\"command\":\"ls\"}")
        );
        assert!(t.contains("[…]"));
        assert!(t.len() < 3_000);
    }

    #[test]
    fn extracts_summary_tags() {
        assert_eq!(summary_text("x <summary>\n kept \n</summary> y"), "kept");
        assert_eq!(summary_text(" plain "), "plain");
    }

    #[test]
    fn only_worthwhile_once_the_model_has_replied() {
        assert!(!worthwhile(&[Message::user_text("hi")]));
        assert!(worthwhile(&history()));
    }
}
