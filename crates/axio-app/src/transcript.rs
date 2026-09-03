//! What each session has said and done, as this process saw it.
//!
//! Built from the supervisor's event stream rather than read back from the
//! session file on every paint, for the same reason the hosted terminal keeps
//! a ring: the window asks many times a second while a turn is running, and a
//! JSONL file re-parsed each time is the same work done again with a slower
//! answer. The file is still the durable record — it seeds a transcript this
//! process did not watch from the start, and it is what a closed session is
//! read from.
//!
//! Nothing in here links Tauri. It is the state behind the transcript pane,
//! and it is tested the way `state` is: with events, and no webview.

use std::collections::BTreeMap;
use std::sync::Mutex;

use axio_core::protocol::{
    Delta, Event, EventKind, Item, ItemBody, ItemId, NoticeLevel, Preview, SessionId, ToolStatus,
    TurnOutcome,
};

use crate::model::{PreviewView, TranscriptEntry, TranscriptView};

/// One session's transcript, in the order it happened.
#[derive(Debug, Default)]
pub struct Transcript {
    entries: Vec<TranscriptEntry>,
    /// Where each item sits, so a delta or a replacement finds its row.
    at: BTreeMap<ItemId, usize>,
    model: Option<String>,
    cost_usd: f64,
    /// Set once the session file has been folded in, so a resumed session's
    /// history is read exactly once and never re-prepended.
    seeded: bool,
}

impl Transcript {
    fn reindex(&mut self) {
        self.at.clear();
        for (n, entry) in self.entries.iter().enumerate() {
            if let Some(id) = item_id(entry) {
                self.at.insert(id, n);
            }
        }
    }

    fn upsert(&mut self, item: &Item, streaming: bool) {
        let entry = entry_of(item, streaming);
        match self.at.get(&item.id) {
            Some(&n) => self.entries[n] = entry,
            None => {
                self.entries.push(entry);
                self.at.insert(item.id, self.entries.len() - 1);
            }
        }
    }

    fn apply(&mut self, event: &Event) {
        match &event.kind {
            EventKind::SessionStarted { model, .. } => self.model = Some(model.clone()),
            EventKind::ItemStarted { item } => self.upsert(item, true),
            EventKind::ItemUpdated { item } => self.upsert(item, true),
            EventKind::ItemCompleted { item } => self.upsert(item, false),
            EventKind::ItemDiscarded { id, .. } => {
                if let Some(&n) = self.at.get(id) {
                    self.entries.remove(n);
                    self.reindex();
                }
            }
            EventKind::ItemDelta { id, delta } => {
                if let Some(&n) = self.at.get(id) {
                    append_delta(&mut self.entries[n], delta);
                }
            }
            EventKind::Notice { level, message } => self.entries.push(TranscriptEntry::Notice {
                id: format!("n{}", event.seq),
                level: level_name(*level).to_owned(),
                message: message.clone(),
            }),
            EventKind::Compacted {
                tokens_before,
                tokens_after,
                ..
            } => self.entries.push(TranscriptEntry::Notice {
                id: format!("n{}", event.seq),
                level: "info".to_owned(),
                message: format!("context compacted from {tokens_before} to {tokens_after} tokens"),
            }),
            EventKind::TurnEnded {
                outcome, cost_usd, ..
            } => {
                self.cost_usd += cost_usd;
                let (name, detail) = outcome_of(outcome);
                self.entries.push(TranscriptEntry::Turn {
                    id: format!("t{}", event.seq),
                    outcome: name.to_owned(),
                    detail,
                    cost_usd: *cost_usd,
                });
            }
            EventKind::TurnStarted
            | EventKind::ApprovalRequested { .. }
            | EventKind::ApprovalResolved { .. }
            | EventKind::Usage(_) => {}
        }
    }

    /// Fold in what the session file holds, ahead of anything seen live.
    ///
    /// Items already present — because the live stream delivered them — are
    /// left where they are; only the ones this process never saw are added,
    /// and they go first, because the file is the past.
    fn seed(&mut self, model: Option<String>, items: &[Item]) {
        if self.seeded {
            return;
        }
        self.seeded = true;
        if self.model.is_none() {
            self.model = model;
        }
        let missing: Vec<TranscriptEntry> = items
            .iter()
            .filter(|item| !self.at.contains_key(&item.id))
            .map(|item| entry_of(item, false))
            .collect();
        if !missing.is_empty() {
            let mut entries = missing;
            entries.append(&mut self.entries);
            self.entries = entries;
            self.reindex();
        }
    }

    fn view(&self, from_record: bool) -> TranscriptView {
        TranscriptView {
            entries: self.entries.clone(),
            model: self.model.clone(),
            cost_usd: self.cost_usd,
            from_record,
        }
    }
}

/// Every session's transcript, keyed by session.
#[derive(Debug, Default)]
pub struct Transcripts {
    by_session: Mutex<BTreeMap<SessionId, Transcript>>,
}

impl Transcripts {
    pub fn observe(&self, event: &Event) {
        self.lock().entry(event.session).or_default().apply(event);
    }

    /// Whether anything has been seen for this session, live or seeded.
    pub fn knows(&self, session: SessionId) -> bool {
        self.lock().contains_key(&session)
    }

    pub fn seed(&self, session: SessionId, model: Option<String>, items: &[Item]) {
        self.lock().entry(session).or_default().seed(model, items);
    }

    pub fn view(&self, session: SessionId, from_record: bool) -> TranscriptView {
        self.lock()
            .get(&session)
            .map(|t| t.view(from_record))
            .unwrap_or(TranscriptView {
                from_record,
                ..Default::default()
            })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<SessionId, Transcript>> {
        self.by_session
            .lock()
            .expect("no transcript lock is held across an await")
    }
}

fn item_id(entry: &TranscriptEntry) -> Option<ItemId> {
    let id = match entry {
        TranscriptEntry::User { id, .. }
        | TranscriptEntry::Agent { id, .. }
        | TranscriptEntry::Reasoning { id, .. }
        | TranscriptEntry::Tool { id, .. }
        | TranscriptEntry::Interrupted { id, .. }
        | TranscriptEntry::Elision { id, .. } => id,
        TranscriptEntry::Turn { .. } | TranscriptEntry::Notice { .. } => return None,
    };
    id.parse().ok()
}

fn entry_of(item: &Item, streaming: bool) -> TranscriptEntry {
    let id = item.id.to_string();
    match &item.body {
        ItemBody::UserMessage { text } => TranscriptEntry::User {
            id,
            text: text.clone(),
        },
        ItemBody::AgentMessage { text } => TranscriptEntry::Agent {
            id,
            text: text.clone(),
            streaming,
        },
        ItemBody::Reasoning { text, .. } => TranscriptEntry::Reasoning {
            id,
            text: text.clone(),
        },
        ItemBody::ToolCall {
            name,
            subject,
            preview,
            status,
            ..
        } => {
            let (status, output, truncated, ms) = match status {
                ToolStatus::Pending => ("pending", String::new(), false, 0),
                ToolStatus::AwaitingApproval => ("awaitingApproval", String::new(), false, 0),
                ToolStatus::Running => ("running", String::new(), false, 0),
                ToolStatus::Ok {
                    output,
                    truncated,
                    ms,
                    ..
                } => ("ok", output.clone(), *truncated, *ms),
                ToolStatus::Failed { message } => ("failed", message.clone(), false, 0),
                ToolStatus::Denied { message } => ("denied", message.clone(), false, 0),
                ToolStatus::Cancelled => ("cancelled", String::new(), false, 0),
            };
            TranscriptEntry::Tool {
                id,
                name: name.clone(),
                subject: subject.clone(),
                status: status.to_owned(),
                output,
                truncated,
                preview: preview.as_ref().map(preview_of),
                ms,
            }
        }
        ItemBody::Interrupted { after_steps } => TranscriptEntry::Interrupted {
            id,
            after_steps: *after_steps,
        },
        ItemBody::ContextElision { dropped_items } => TranscriptEntry::Elision {
            id,
            dropped_items: *dropped_items,
        },
    }
}

fn append_delta(entry: &mut TranscriptEntry, delta: &Delta) {
    match (entry, delta) {
        (
            TranscriptEntry::Agent {
                text, streaming, ..
            },
            Delta::Text { text: more },
        ) => {
            text.push_str(more);
            *streaming = true;
        }
        (TranscriptEntry::Reasoning { text, .. }, Delta::Reasoning { text: more }) => {
            text.push_str(more);
        }
        (TranscriptEntry::Tool { output, .. }, Delta::ToolOutput { text: more, .. }) => {
            output.push_str(more);
        }
        // Argument fragments are parsed once at block end and arrive whole in
        // the completed item; a row forming character by character is noise.
        _ => {}
    }
}

fn outcome_of(outcome: &TurnOutcome) -> (&'static str, String) {
    match outcome {
        TurnOutcome::Completed => ("completed", String::new()),
        TurnOutcome::Refused { text, .. } => ("refused", text.clone()),
        TurnOutcome::Interrupted => ("interrupted", String::new()),
        TurnOutcome::StepLimit { steps } => ("step_limit", format!("after {steps} steps")),
        TurnOutcome::BudgetExceeded {
            spent_usd,
            limit_usd,
        } => (
            "budget_exceeded",
            format!("${spent_usd:.2} spent of a ${limit_usd:.2} limit"),
        ),
        TurnOutcome::Failed { message } => ("failed", message.clone()),
    }
}

fn level_name(level: NoticeLevel) -> &'static str {
    match level {
        NoticeLevel::Info => "info",
        NoticeLevel::Warn => "warn",
        NoticeLevel::Error => "error",
    }
}

pub(crate) fn preview_of(preview: &Preview) -> PreviewView {
    match preview {
        Preview::Diff {
            path,
            unified,
            added,
            removed,
        } => PreviewView::Diff {
            path: path.display().to_string(),
            unified: unified.clone(),
            added: *added,
            removed: *removed,
        },
        Preview::Command {
            program, raw, cwd, ..
        } => PreviewView::Command {
            program: program.clone(),
            raw: raw.clone(),
            cwd: cwd.display().to_string(),
        },
        Preview::Text { text } => PreviewView::Text { text: text.clone() },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axio_core::protocol::Usage;

    fn event(session: SessionId, seq: u64, kind: EventKind) -> Event {
        Event {
            seq,
            session,
            turn: None,
            at_ms: 0,
            kind,
        }
    }

    fn agent(text: &str) -> Item {
        Item::new(ItemBody::AgentMessage {
            text: text.to_owned(),
        })
    }

    /// Streamed text accumulates on one row, and the completed item replaces
    /// it rather than adding a second — which is what a renderer that dropped
    /// every delta would have shown all along.
    #[test]
    fn deltas_accumulate_and_the_completed_item_replaces_them() {
        let session = SessionId::generate();
        let store = Transcripts::default();
        let item = agent("");
        store.observe(&event(
            session,
            1,
            EventKind::ItemStarted { item: item.clone() },
        ));
        for (n, piece) in ["Hel", "lo"].iter().enumerate() {
            store.observe(&event(
                session,
                2 + n as u64,
                EventKind::ItemDelta {
                    id: item.id,
                    delta: Delta::Text {
                        text: (*piece).to_owned(),
                    },
                },
            ));
        }
        let view = store.view(session, false);
        assert_eq!(view.entries.len(), 1);
        match &view.entries[0] {
            TranscriptEntry::Agent {
                text, streaming, ..
            } => {
                assert_eq!(text, "Hello");
                assert!(streaming);
            }
            other => panic!("{other:?}"),
        }

        let done = Item {
            id: item.id,
            body: ItemBody::AgentMessage {
                text: "Hello.".to_owned(),
            },
        };
        store.observe(&event(session, 4, EventKind::ItemCompleted { item: done }));
        let view = store.view(session, false);
        assert_eq!(view.entries.len(), 1, "replaced, not appended");
        match &view.entries[0] {
            TranscriptEntry::Agent {
                text, streaming, ..
            } => {
                assert_eq!(text, "Hello.");
                assert!(!streaming);
            }
            other => panic!("{other:?}"),
        }
    }

    /// A retry discards the in-flight item. Keeping it would show the text
    /// twice, which is the exact bug the event exists to prevent.
    #[test]
    fn a_discarded_item_leaves_the_transcript() {
        let session = SessionId::generate();
        let store = Transcripts::default();
        let first = agent("partial");
        let second = agent("kept");
        store.observe(&event(
            session,
            1,
            EventKind::ItemStarted {
                item: first.clone(),
            },
        ));
        store.observe(&event(
            session,
            2,
            EventKind::ItemStarted {
                item: second.clone(),
            },
        ));
        store.observe(&event(
            session,
            3,
            EventKind::ItemDiscarded {
                id: first.id,
                reason: "retry".into(),
            },
        ));
        // The survivor must still be addressable after the reindex.
        store.observe(&event(
            session,
            4,
            EventKind::ItemDelta {
                id: second.id,
                delta: Delta::Text { text: "!".into() },
            },
        ));
        let view = store.view(session, false);
        assert_eq!(view.entries.len(), 1);
        assert!(matches!(&view.entries[0], TranscriptEntry::Agent { text, .. } if text == "kept!"));
    }

    #[test]
    fn a_turn_ending_adds_a_row_and_its_cost() {
        let session = SessionId::generate();
        let store = Transcripts::default();
        store.observe(&event(
            session,
            1,
            EventKind::TurnEnded {
                outcome: TurnOutcome::StepLimit { steps: 40 },
                usage: Usage::default(),
                cost_usd: 0.25,
                files_changed: vec![],
            },
        ));
        let view = store.view(session, false);
        assert_eq!(view.cost_usd, 0.25);
        match &view.entries[0] {
            TranscriptEntry::Turn {
                outcome, detail, ..
            } => {
                assert_eq!(outcome, "step_limit");
                assert!(detail.contains("40"));
            }
            other => panic!("{other:?}"),
        }
    }

    /// The file is the past: what it holds goes before what was seen live,
    /// and an item the stream already delivered is not shown twice.
    #[test]
    fn seeding_prepends_history_once_and_never_duplicates() {
        let session = SessionId::generate();
        let store = Transcripts::default();
        let old = Item::new(ItemBody::UserMessage {
            text: "earlier".into(),
        });
        let live = agent("now");
        store.observe(&event(
            session,
            1,
            EventKind::ItemCompleted { item: live.clone() },
        ));
        store.seed(session, Some("m".into()), &[old.clone(), live.clone()]);
        store.seed(session, None, std::slice::from_ref(&old));
        let view = store.view(session, false);
        assert_eq!(view.entries.len(), 2);
        assert!(matches!(&view.entries[0], TranscriptEntry::User { .. }));
        assert!(matches!(&view.entries[1], TranscriptEntry::Agent { .. }));
        assert_eq!(view.model.as_deref(), Some("m"));
    }

    #[test]
    fn a_tool_call_is_flattened_to_what_a_row_shows() {
        let item = Item::new(ItemBody::ToolCall {
            call_id: "c1".into(),
            name: "bash".into(),
            input: serde_json::json!({}),
            subject: "bash:git".into(),
            preview: Some(Preview::Command {
                program: "git".into(),
                argv: vec!["git".into(), "status".into()],
                raw: "git status".into(),
                cwd: "/w".into(),
            }),
            status: ToolStatus::Ok {
                output: "clean".into(),
                truncated: false,
                spill: None,
                ms: 12,
            },
        });
        match entry_of(&item, false) {
            TranscriptEntry::Tool {
                status,
                output,
                ms,
                preview,
                ..
            } => {
                assert_eq!(status, "ok");
                assert_eq!(output, "clean");
                assert_eq!(ms, 12);
                assert!(
                    matches!(preview, Some(PreviewView::Command { raw, .. }) if raw == "git status")
                );
            }
            other => panic!("{other:?}"),
        }
    }
}
