//! One billable message, however it was logged.
//!
//! Every source parser produces these and nothing else, so the aggregator, the pricing
//! layer and both surfaces know one shape. The fields that are `Option` are the ones some
//! agents genuinely do not record; a parser must never invent a value to fill them.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::tokens::TokenBreakdown;

/// The agent that wrote the log — *not* the company whose model was billed.
///
/// These are separate on purpose. A local Claude Code transcript here contains
/// `gpt-5.6-terra`, `deepseek-v4-flash` and `glm-5.2` alongside `claude-opus-5`, because
/// the CLI was pointed at a proxy. Attributing cost to "Claude" because the directory is
/// named `.claude` would bill Anthropic for OpenAI's tokens. The client answers *which
/// tool did I run*; the model answers *who charged me*.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClientId(pub String);

impl ClientId {
    pub fn new(raw: impl Into<String>) -> Self {
        ClientId(raw.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ClientId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One assistant response, normalized.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostMessage {
    /// Which agent produced this line.
    pub client: ClientId,
    /// Model id exactly as the log spelled it. Normalization happens in pricing, so the
    /// raw string survives for `--diagnose` when a price lookup misses.
    pub model: String,
    /// The session this belongs to, as the source identifies it.
    pub session_id: String,
    /// Project or workspace, when the agent records one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
    pub tokens: TokenBreakdown,
    /// Identity of the underlying API call, when the log carries one.
    ///
    /// `None` means the source gives nothing to deduplicate on and the message is taken
    /// at face value. That is a real state, not a failure: several agents write one line
    /// per completed response and repeat nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dedup_key: Option<String>,
    /// True when this response opened a user turn, for counting turns rather than
    /// API calls.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub turn_start: bool,
    /// Cost in dollars as reported by the agent itself.
    ///
    /// A few agents - Grok among them - write what the turn actually cost into the
    /// transcript. That is the vendor's own arithmetic against the plan the user is
    /// really on, so it beats any table this crate could carry: no model-id
    /// normalization to get wrong, no rate to go stale, no discount to miss.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported_cost: Option<f64>,
}

impl CostMessage {
    /// Whether this message can be priced at all.
    ///
    /// Claude Code writes `<synthetic>` for lines it generated locally — an interrupt
    /// notice, a hook result — which cost nothing and have no model to look up. Nine of
    /// them appear in the local logs. They are dropped rather than priced at zero so a
    /// missing price stays distinguishable from a genuine zero.
    pub fn is_billable(&self) -> bool {
        !self.model.is_empty()
            && !self.model.starts_with('<')
            && !self.tokens.is_empty()
    }
}

/// Collapses repeated reports of one message, keeping the most complete counts.
///
/// Sources that stream write the same `(message id, request id)` several times as the
/// response fills in. Without this the input is billed once per chunk. With it, the
/// merge is a field-wise maximum — see [`TokenBreakdown::merge_cumulative`] for why that
/// is right rather than merely convenient.
///
/// Messages with no `dedup_key` pass through untouched, in order.
#[derive(Debug, Default)]
pub struct DedupLedger {
    seen: std::collections::HashMap<String, usize>,
    messages: Vec<CostMessage>,
}

impl DedupLedger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(capacity: usize) -> Self {
        DedupLedger {
            seen: std::collections::HashMap::with_capacity(capacity),
            messages: Vec::with_capacity(capacity),
        }
    }

    /// Adds a message, merging it into an earlier one when they are the same API call.
    ///
    /// Returns `true` when the message was new. The timestamp of the first sighting wins:
    /// a response belongs to the moment it was requested, and later chunks of the same
    /// response would otherwise drag it across a day boundary in a daily rollup.
    pub fn push(&mut self, message: CostMessage) -> bool {
        let Some(key) = message.dedup_key.clone() else {
            self.messages.push(message);
            return true;
        };
        match self.seen.get(&key) {
            Some(&index) => {
                let existing = &mut self.messages[index];
                existing.tokens.merge_cumulative(&message.tokens);
                // A later chunk may name the model the earlier one had not resolved yet.
                if existing.model.is_empty() {
                    existing.model = message.model;
                }
                false
            }
            None => {
                self.seen.insert(key, self.messages.len());
                self.messages.push(message);
                true
            }
        }
    }

    pub fn extend(&mut self, messages: impl IntoIterator<Item = CostMessage>) {
        for message in messages {
            self.push(message);
        }
    }

    pub fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    pub fn into_messages(self) -> Vec<CostMessage> {
        self.messages
    }

    pub fn messages(&self) -> &[CostMessage] {
        &self.messages
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn message(key: Option<&str>, input: u64, output: u64) -> CostMessage {
        CostMessage {
            client: ClientId::new("claude-code"),
            model: "claude-opus-5".into(),
            session_id: "s1".into(),
            workspace: None,
            timestamp: datetime!(2026-08-02 10:00 UTC),
            tokens: TokenBreakdown {
                input,
                output,
                ..Default::default()
            },
            dedup_key: key.map(str::to_string),
            turn_start: false,
            reported_cost: None,
        }
    }

    #[test]
    fn a_streamed_message_is_billed_once() {
        let mut ledger = DedupLedger::new();
        assert!(ledger.push(message(Some("msg1:req1"), 1_006, 0)));
        assert!(!ledger.push(message(Some("msg1:req1"), 1_006, 11)));

        assert_eq!(ledger.len(), 1);
        let only = &ledger.messages()[0];
        assert_eq!(only.tokens.input, 1_006, "not 2012");
        assert_eq!(only.tokens.output, 11);
    }

    #[test]
    fn distinct_calls_are_kept_apart() {
        let mut ledger = DedupLedger::new();
        ledger.push(message(Some("msg1:req1"), 100, 1));
        ledger.push(message(Some("msg2:req2"), 200, 2));
        assert_eq!(ledger.len(), 2);
    }

    /// Several agents log one line per finished response and repeat nothing. Those must
    /// not collapse into each other just because they carry no key.
    #[test]
    fn messages_without_a_key_are_never_merged() {
        let mut ledger = DedupLedger::new();
        ledger.push(message(None, 100, 1));
        ledger.push(message(None, 100, 1));
        assert_eq!(ledger.len(), 2);
    }

    #[test]
    fn the_first_sighting_keeps_its_timestamp() {
        let mut ledger = DedupLedger::new();
        ledger.push(message(Some("k"), 10, 0));
        let mut later = message(Some("k"), 10, 5);
        later.timestamp = datetime!(2026-08-03 02:00 UTC);
        ledger.push(later);

        assert_eq!(
            ledger.messages()[0].timestamp,
            datetime!(2026-08-02 10:00 UTC),
            "a response belongs to the day it was asked for"
        );
    }

    #[test]
    fn synthetic_models_are_not_billable() {
        let mut synthetic = message(None, 10, 1);
        synthetic.model = "<synthetic>".into();
        assert!(!synthetic.is_billable());
        assert!(message(None, 10, 1).is_billable());
    }

    #[test]
    fn a_message_with_no_tokens_is_not_billable() {
        assert!(!message(None, 0, 0).is_billable());
    }
}
