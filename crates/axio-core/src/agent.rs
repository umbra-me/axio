//! The turn loop.
//!
//! One user turn in, one `TurnEnded` out — on every exit path, including
//! cancellation and failure. A surface that counts `TurnStarted` and
//! `TurnEnded` can rely on them balancing.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use futures_core::Stream;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::approver::Approver;
use crate::protocol::{
    Delta, Event, EventKind, Item, ItemBody, ItemId, NoticeLevel, PROTOCOL_VERSION, SessionId,
    ToolStatus, TurnId, TurnOutcome, Usage,
};
use crate::provider::{
    BlockKind, Effort, ModelRequest, Provider, ProviderError, ReasoningDisplay, StopReason,
    StreamEvent, SystemBlock, ToolInputAccumulator, ToolSpec,
};
use crate::session::Session;
use crate::tool::Tool;

/// Resolved once, at construction, and never re-read.
///
/// The predecessor project read config inside the loop on every turn and had to
/// carry a written rule forbidding other reads. A rule in a document is not an
/// enforcement mechanism; a value that can only be produced by the resolver is.
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    pub model: String,
    pub effort: Effort,
    pub reasoning: ReasoningDisplay,
    pub max_tokens: u32,
    pub max_steps: u32,
    /// Successive delays between retries. Running out of entries makes the
    /// error fatal, so the length is also the retry budget.
    pub backoff: Vec<Duration>,
    pub max_usd_per_turn: Option<f64>,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            model: "claude-opus-5".to_owned(),
            effort: Effort::default(),
            reasoning: ReasoningDisplay::default(),
            max_tokens: 64_000,
            max_steps: 50,
            backoff: vec![
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(5),
            ],
            max_usd_per_turn: None,
        }
    }
}

/// Why a turn stopped sampling.
enum TurnBreak {
    Cancelled,
    Overflow,
    Fatal(String),
}

/// One fully-buffered assistant message.
#[derive(Debug, Default)]
struct Sampled {
    blocks: Vec<ItemBody>,
    usage: Usage,
    stop: Option<StopReason>,
}

impl Sampled {
    fn text(&self) -> String {
        self.blocks
            .iter()
            .filter_map(|b| match b {
                ItemBody::AgentMessage { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    fn tool_calls(&self) -> Vec<(String, String, serde_json::Value)> {
        self.blocks
            .iter()
            .filter_map(|b| match b {
                ItemBody::ToolCall {
                    call_id,
                    name,
                    input,
                    ..
                } => Some((call_id.clone(), name.clone(), input.clone())),
                _ => None,
            })
            .collect()
    }
}

pub struct Agent {
    provider: Arc<dyn Provider>,
    tools: BTreeMap<String, Arc<dyn Tool>>,
    #[allow(dead_code)] // wired to the policy engine when tools land
    approver: Arc<dyn Approver>,
    session: Session,
    cfg: RuntimeConfig,
    system: Arc<[SystemBlock]>,
    /// Unbounded deliberately: a bounded send from inside the loop can deadlock
    /// against a surface that is slow to drain.
    events: mpsc::UnboundedSender<Event>,
    seq: u64,
}

impl Agent {
    pub fn new(
        provider: Arc<dyn Provider>,
        approver: Arc<dyn Approver>,
        session: Session,
        cfg: RuntimeConfig,
        system: Vec<SystemBlock>,
        events: mpsc::UnboundedSender<Event>,
    ) -> Self {
        Self {
            provider,
            tools: BTreeMap::new(),
            approver,
            session,
            cfg,
            system: Arc::from(system),
            events,
            seq: 0,
        }
    }

    pub fn register_tool(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_owned(), tool);
    }

    pub fn session_id(&self) -> SessionId {
        self.session.id()
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    /// Emitted once, before any turn, so a consumer can refuse a stream it does
    /// not understand before it has to interpret one.
    pub fn announce(&mut self, resumed: bool) {
        let kind = EventKind::SessionStarted {
            protocol: PROTOCOL_VERSION,
            session: self.session.id(),
            model: self.cfg.model.clone(),
            cwd: self.session.cwd().clone(),
            effort: self.cfg.effort,
            resumed,
        };
        self.emit(None, kind);
    }

    /// One user turn. Every exit path emits exactly one `TurnEnded`.
    pub async fn run_turn(&mut self, input: String, cancel: CancellationToken) -> TurnOutcome {
        let turn = TurnId::generate();
        self.session.push_user(&input);
        self.emit(Some(turn), EventKind::TurnStarted);

        let mut usage = Usage::default();
        let mut steps = 0u32;

        let outcome = 'turn: {
            while steps < self.cfg.max_steps {
                steps += 1;

                let req = self.build_request();
                let sampled = match self.sample(turn, req, &cancel).await {
                    Ok(s) => s,
                    Err(TurnBreak::Cancelled) => {
                        self.session
                            .push(ItemBody::Interrupted { after_steps: steps });
                        break 'turn TurnOutcome::Interrupted;
                    }
                    Err(TurnBreak::Overflow) => {
                        break 'turn TurnOutcome::Failed {
                            message: "context exhausted; nothing left to elide".into(),
                        };
                    }
                    Err(TurnBreak::Fatal(m)) => break 'turn TurnOutcome::Failed { message: m },
                };
                usage.add(&sampled.usage);

                if let Some(outcome) = self.check_budget(&usage) {
                    break 'turn outcome;
                }

                if let Some(StopReason::Refusal { category }) = &sampled.stop {
                    break 'turn TurnOutcome::Refused {
                        category: category.clone(),
                        text: sampled.text(),
                    };
                }

                // Append the full assistant content — reasoning included,
                // verbatim — before anything executes. It is wire state and must
                // be echoed back unchanged on the next request.
                for block in &sampled.blocks {
                    self.session.push(block.clone());
                }

                let calls = sampled.tool_calls();
                if calls.is_empty() {
                    break 'turn TurnOutcome::Completed;
                }

                self.dispatch(turn, calls, &cancel).await;

                if cancel.is_cancelled() {
                    self.session
                        .push(ItemBody::Interrupted { after_steps: steps });
                    break 'turn TurnOutcome::Interrupted;
                }
            }
            TurnOutcome::StepLimit { steps }
        };

        let files_changed = self.session.take_files_changed();
        self.emit(
            Some(turn),
            EventKind::TurnEnded {
                outcome: outcome.clone(),
                usage,
                files_changed,
            },
        );
        outcome
    }

    fn build_request(&self) -> ModelRequest {
        let mut req = ModelRequest::new(&self.cfg.model);
        req.system = self.system.clone();
        req.messages = self.session.wire_messages(&self.cfg.model);
        req.tools = Arc::from(
            self.tools
                .values()
                .map(|t| ToolSpec {
                    name: t.name().to_owned(),
                    description: t.description().to_owned(),
                    input_schema: t.schema().clone(),
                })
                .collect::<Vec<_>>(),
        );
        req.max_tokens = self.cfg.max_tokens;
        req.effort = self.cfg.effort;
        req.reasoning = self.cfg.reasoning;
        req
    }

    fn check_budget(&self, usage: &Usage) -> Option<TurnOutcome> {
        let limit = self.cfg.max_usd_per_turn?;
        let spent = self.provider.model_info(&self.cfg.model).cost_usd(usage);
        (spent > limit).then_some(TurnOutcome::BudgetExceeded {
            spent_usd: spent,
            limit_usd: limit,
        })
    }

    /// Sample one assistant message, retrying a retryable failure.
    async fn sample(
        &mut self,
        turn: TurnId,
        req: ModelRequest,
        cancel: &CancellationToken,
    ) -> Result<Sampled, TurnBreak> {
        let mut backoff = self.cfg.backoff.clone().into_iter();
        loop {
            // A child token so cancelling the turn cancels the in-flight
            // request, while a per-request failure never poisons the turn's own
            // token.
            let child = cancel.child_token();
            let item = ItemId::generate();
            match self.stream_once(turn, item, req.clone(), child).await {
                Ok(sampled) => return Ok(sampled),

                Err(ProviderError::Cancelled) => return Err(TurnBreak::Cancelled),
                Err(ProviderError::ContextOverflow) => return Err(TurnBreak::Overflow),

                Err(e) if e.retryable() => {
                    // Tell every renderer to drop the partial before retrying;
                    // otherwise the retry double-prints text already on screen.
                    self.emit(
                        Some(turn),
                        EventKind::ItemDiscarded {
                            id: item,
                            reason: format!("{e}; retrying"),
                        },
                    );
                    let Some(base) = backoff.next() else {
                        return Err(TurnBreak::Fatal(e.to_string()));
                    };
                    // The server's own hint beats our table: it knows when it
                    // will accept work again and we do not.
                    let delay = e.backoff_hint().unwrap_or(base);
                    self.notice(
                        turn,
                        NoticeLevel::Warn,
                        format!("{e}; retrying in {}s", delay.as_secs()),
                    );
                    tokio::select! {
                        _ = tokio::time::sleep(delay) => {}
                        _ = cancel.cancelled() => return Err(TurnBreak::Cancelled),
                    }
                }
                Err(e) => return Err(TurnBreak::Fatal(e.to_string())),
            }
        }
    }

    /// Buffer one complete assistant message, streaming deltas as they arrive.
    ///
    /// The whole message is buffered before any tool runs: executing a tool
    /// mid-stream can idle the connection past its read timeout, and a
    /// half-drained stream leaves content blocks unterminated. The latency cost
    /// is real and accepted — deltas still reach the surface meanwhile.
    async fn stream_once(
        &mut self,
        turn: TurnId,
        _item: ItemId,
        req: ModelRequest,
        cancel: CancellationToken,
    ) -> Result<Sampled, ProviderError> {
        let mut stream = self.provider.stream(req, cancel.clone()).await?;
        let mut sampled = Sampled::default();
        let mut acc = ToolInputAccumulator::new();

        // Per-block state, keyed by the provider's block index.
        let mut kinds: BTreeMap<u32, BlockKind> = BTreeMap::new();
        let mut items: BTreeMap<u32, ItemId> = BTreeMap::new();
        let mut texts: BTreeMap<u32, String> = BTreeMap::new();
        let mut signatures: BTreeMap<u32, String> = BTreeMap::new();

        loop {
            let next = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(ProviderError::Cancelled),
                next = std::future::poll_fn(|cx| std::pin::Pin::new(&mut stream).poll_next(cx)) => next,
            };
            let Some(event) = next else { break };
            match event? {
                StreamEvent::MessageStart { .. } => {}
                StreamEvent::BlockStart { index, kind } => {
                    let id = ItemId::generate();
                    items.insert(index, id);
                    texts.insert(index, String::new());
                    match &kind {
                        BlockKind::Text => self.emit(
                            Some(turn),
                            EventKind::ItemStarted {
                                item: Item {
                                    id,
                                    body: ItemBody::AgentMessage {
                                        text: String::new(),
                                    },
                                },
                            },
                        ),
                        BlockKind::Thinking => self.emit(
                            Some(turn),
                            EventKind::ItemStarted {
                                item: Item {
                                    id,
                                    body: ItemBody::Reasoning {
                                        text: String::new(),
                                        signature: String::new(),
                                    },
                                },
                            },
                        ),
                        BlockKind::ToolUse { .. } => {}
                    }
                    kinds.insert(index, kind);
                }
                StreamEvent::TextDelta { index, text } => {
                    texts.entry(index).or_default().push_str(&text);
                    if let Some(id) = items.get(&index) {
                        self.emit(
                            Some(turn),
                            EventKind::ItemDelta {
                                id: *id,
                                delta: Delta::Text { text },
                            },
                        );
                    }
                }
                StreamEvent::ReasoningDelta { index, text } => {
                    texts.entry(index).or_default().push_str(&text);
                    if let Some(id) = items.get(&index) {
                        self.emit(
                            Some(turn),
                            EventKind::ItemDelta {
                                id: *id,
                                delta: Delta::Reasoning { text },
                            },
                        );
                    }
                }
                StreamEvent::ReasoningSignature { index, signature } => {
                    signatures.insert(index, signature);
                }
                StreamEvent::ToolInputDelta { index, json } => {
                    acc.push(index, &json);
                    if let Some(id) = items.get(&index) {
                        self.emit(
                            Some(turn),
                            EventKind::ItemDelta {
                                id: *id,
                                delta: Delta::ToolInputJson { json },
                            },
                        );
                    }
                }
                StreamEvent::BlockEnd { index } => {
                    let Some(kind) = kinds.remove(&index) else {
                        continue;
                    };
                    let text = texts.remove(&index).unwrap_or_default();
                    let id = items.remove(&index).unwrap_or_else(ItemId::generate);
                    let body = match kind {
                        BlockKind::Text => ItemBody::AgentMessage { text },
                        BlockKind::Thinking => ItemBody::Reasoning {
                            text,
                            signature: signatures.remove(&index).unwrap_or_default(),
                        },
                        BlockKind::ToolUse { id: call_id, name } => {
                            // Parsed exactly once, here, at block end.
                            let input = acc.finish(index)?;
                            ItemBody::ToolCall {
                                call_id,
                                name,
                                input,
                                subject: String::new(),
                                preview: None,
                                status: ToolStatus::Pending,
                            }
                        }
                    };
                    let item = Item {
                        id,
                        body: body.clone(),
                    };
                    self.emit(Some(turn), EventKind::ItemCompleted { item });
                    sampled.blocks.push(body);
                }
                StreamEvent::Usage(u) => sampled.usage.add(&u),
                StreamEvent::Done { stop } => sampled.stop = Some(stop),
            }
        }

        Ok(sampled)
    }

    /// Execute tool calls. Every call yields a result, including an unknown one.
    ///
    /// The full plan/authorise/execute machinery lands with the tools; what is
    /// here already holds the invariant that matters: a `tool_use` with no
    /// matching `tool_result` makes the next request invalid, so a call that
    /// cannot run still records why.
    async fn dispatch(
        &mut self,
        turn: TurnId,
        calls: Vec<(String, String, serde_json::Value)>,
        _cancel: &CancellationToken,
    ) {
        for (call_id, name, _input) in calls {
            let status = match self.tools.get(&name) {
                Some(_) => ToolStatus::Failed {
                    message: format!("tool `{name}` is registered but execution is not wired yet"),
                },
                None => ToolStatus::Failed {
                    message: format!("unknown tool: {name}"),
                },
            };
            // Resolve the call the model already emitted rather than pushing a
            // second item, which would give it two results on the wire.
            if let Some(item) = self.session.set_tool_status(&call_id, status) {
                self.emit(Some(turn), EventKind::ItemUpdated { item });
            }
        }
    }

    fn notice(&mut self, turn: TurnId, level: NoticeLevel, message: String) {
        self.emit(Some(turn), EventKind::Notice { level, message });
    }

    fn emit(&mut self, turn: Option<TurnId>, kind: EventKind) {
        self.seq += 1;
        let event = Event {
            seq: self.seq,
            session: self.session.id(),
            turn,
            at_ms: now_ms(),
            kind,
        };
        // A closed receiver means the surface went away; the turn still has to
        // finish cleanly, so this is not an error.
        let _ = self.events.send(event);
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approver::NonInteractive;
    use crate::protocol::Usage;
    use crate::provider::StopReason;
    use crate::scripted::{Script, ScriptedProvider};
    use std::path::PathBuf;

    fn harness(scripts: Vec<Script>) -> (Agent, mpsc::UnboundedReceiver<Event>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let agent = Agent::new(
            Arc::new(ScriptedProvider::new(scripts)),
            Arc::new(NonInteractive::deny()),
            Session::new(PathBuf::from("/w"), "claude-opus-5"),
            RuntimeConfig {
                backoff: vec![Duration::from_millis(1), Duration::from_millis(1)],
                ..Default::default()
            },
            vec![],
            tx,
        );
        (agent, rx)
    }

    fn drain(rx: &mut mpsc::UnboundedReceiver<Event>) -> Vec<Event> {
        let mut out = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            out.push(ev);
        }
        out
    }

    fn text_turn(text: &str) -> Script {
        Script::Events(vec![
            StreamEvent::MessageStart { id: "m".into() },
            StreamEvent::BlockStart {
                index: 0,
                kind: BlockKind::Text,
            },
            StreamEvent::TextDelta {
                index: 0,
                text: text.into(),
            },
            StreamEvent::BlockEnd { index: 0 },
            StreamEvent::Usage(Usage {
                input_tokens: 10,
                output_tokens: 5,
                ..Default::default()
            }),
            StreamEvent::Done {
                stop: StopReason::EndTurn,
            },
        ])
    }

    #[tokio::test]
    async fn a_plain_turn_completes_and_streams_its_text() {
        let (mut agent, mut rx) = harness(vec![text_turn("hello")]);
        agent.announce(false);
        let outcome = agent.run_turn("hi".into(), CancellationToken::new()).await;
        assert!(matches!(outcome, TurnOutcome::Completed));

        let events = drain(&mut rx);
        assert!(matches!(
            events[0].kind,
            EventKind::SessionStarted { protocol: 1, .. }
        ));
        assert!(matches!(events[1].kind, EventKind::TurnStarted));
        assert!(matches!(
            events.last().unwrap().kind,
            EventKind::TurnEnded { .. }
        ));
        let deltas: Vec<String> = events
            .iter()
            .filter_map(|e| match &e.kind {
                EventKind::ItemDelta {
                    delta: Delta::Text { text },
                    ..
                } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(deltas, ["hello"]);
    }

    #[tokio::test]
    async fn seq_is_monotonic_and_gap_free() {
        let (mut agent, mut rx) = harness(vec![text_turn("a"), text_turn("b")]);
        agent.announce(false);
        agent.run_turn("one".into(), CancellationToken::new()).await;
        agent.run_turn("two".into(), CancellationToken::new()).await;

        let seqs: Vec<u64> = drain(&mut rx).iter().map(|e| e.seq).collect();
        assert_eq!(seqs[0], 1, "seq starts at 1");
        for pair in seqs.windows(2) {
            assert_eq!(pair[1], pair[0] + 1, "gap or repeat in {seqs:?}");
        }
    }

    #[tokio::test]
    async fn exactly_one_turn_ended_per_turn_on_every_path() {
        for scripts in [
            vec![text_turn("ok")],
            vec![Script::Error(ProviderError::Auth("bad key".into()))],
            vec![Script::Error(ProviderError::ContextOverflow)],
        ] {
            let (mut agent, mut rx) = harness(scripts);
            agent.run_turn("x".into(), CancellationToken::new()).await;
            let ended = drain(&mut rx)
                .iter()
                .filter(|e| matches!(e.kind, EventKind::TurnEnded { .. }))
                .count();
            assert_eq!(ended, 1);
        }
    }

    #[tokio::test]
    async fn a_retryable_failure_discards_the_partial_then_retries_cleanly() {
        let (mut agent, mut rx) = harness(vec![
            Script::PartialThenError(
                vec![
                    StreamEvent::BlockStart {
                        index: 0,
                        kind: BlockKind::Text,
                    },
                    StreamEvent::TextDelta {
                        index: 0,
                        text: "half".into(),
                    },
                ],
                ProviderError::Overloaded,
            ),
            text_turn("whole"),
        ]);
        let outcome = agent.run_turn("x".into(), CancellationToken::new()).await;
        assert!(matches!(outcome, TurnOutcome::Completed));

        let events = drain(&mut rx);
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e.kind, EventKind::ItemDiscarded { .. }))
                .count(),
            1,
            "the partial must be discarded exactly once"
        );
        // The transcript keeps only the successful attempt — no duplicated text.
        let agent_text: Vec<String> = agent
            .session()
            .transcript()
            .iter()
            .filter_map(|i| match &i.body {
                ItemBody::AgentMessage { text } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(agent_text, ["whole"]);
    }

    #[tokio::test]
    async fn a_non_retryable_failure_is_fatal_immediately() {
        let (mut agent, mut rx) = harness(vec![Script::Error(ProviderError::Auth("bad".into()))]);
        let outcome = agent.run_turn("x".into(), CancellationToken::new()).await;
        assert!(matches!(outcome, TurnOutcome::Failed { .. }));
        assert_eq!(outcome.exit_code(), 1);
        assert_eq!(
            drain(&mut rx)
                .iter()
                .filter(|e| matches!(e.kind, EventKind::ItemDiscarded { .. }))
                .count(),
            0,
            "nothing to discard, and no retry"
        );
    }

    #[tokio::test]
    async fn a_refusal_is_an_outcome_not_a_failure() {
        let (mut agent, _rx) = harness(vec![Script::Events(vec![
            StreamEvent::BlockStart {
                index: 0,
                kind: BlockKind::Text,
            },
            StreamEvent::TextDelta {
                index: 0,
                text: "I can't help with that.".into(),
            },
            StreamEvent::BlockEnd { index: 0 },
            StreamEvent::Done {
                stop: StopReason::Refusal {
                    category: Some("cyber".into()),
                },
            },
        ])]);
        let outcome = agent.run_turn("x".into(), CancellationToken::new()).await;
        match &outcome {
            TurnOutcome::Refused { category, text } => {
                assert_eq!(category.as_deref(), Some("cyber"));
                assert!(text.contains("can't help"));
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
        assert_eq!(outcome.exit_code(), 4);
    }

    #[tokio::test]
    async fn cancellation_interrupts_and_records_that_it_did() {
        let (mut agent, _rx) = harness(vec![text_turn("never seen")]);
        let cancel = CancellationToken::new();
        cancel.cancel();
        let outcome = agent.run_turn("x".into(), cancel).await;
        assert!(matches!(outcome, TurnOutcome::Interrupted));
        assert_eq!(outcome.exit_code(), 130);
        assert!(
            agent
                .session()
                .transcript()
                .iter()
                .any(|i| matches!(i.body, ItemBody::Interrupted { .. })),
            "the model must learn its work was cut short"
        );
    }

    #[tokio::test]
    async fn context_overflow_with_nothing_to_elide_fails_explicitly() {
        let (mut agent, _rx) = harness(vec![Script::Error(ProviderError::ContextOverflow)]);
        match agent.run_turn("x".into(), CancellationToken::new()).await {
            TurnOutcome::Failed { message } => assert!(message.contains("context exhausted")),
            other => panic!("expected an explicit failure, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn the_step_limit_is_enforced() {
        // A model that keeps calling a tool forever must still terminate.
        let tool_turn = || {
            Script::Events(vec![
                StreamEvent::BlockStart {
                    index: 0,
                    kind: BlockKind::ToolUse {
                        id: "toolu_1".into(),
                        name: "read".into(),
                    },
                },
                StreamEvent::ToolInputDelta {
                    index: 0,
                    json: "{}".into(),
                },
                StreamEvent::BlockEnd { index: 0 },
                StreamEvent::Done {
                    stop: StopReason::ToolUse,
                },
            ])
        };
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut agent = Agent::new(
            Arc::new(ScriptedProvider::new(vec![
                tool_turn(),
                tool_turn(),
                tool_turn(),
            ])),
            Arc::new(NonInteractive::deny()),
            Session::new(PathBuf::from("/w"), "claude-opus-5"),
            RuntimeConfig {
                max_steps: 2,
                ..Default::default()
            },
            vec![],
            tx,
        );
        match agent.run_turn("go".into(), CancellationToken::new()).await {
            TurnOutcome::StepLimit { steps } => assert_eq!(steps, 2),
            other => panic!("expected the step limit, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_unknown_tool_still_produces_a_result() {
        // A tool_use with no matching tool_result makes the next request
        // invalid, so even a call we cannot run has to answer.
        let (mut agent, _rx) = harness(vec![
            Script::Events(vec![
                StreamEvent::BlockStart {
                    index: 0,
                    kind: BlockKind::ToolUse {
                        id: "toolu_7".into(),
                        name: "nonexistent".into(),
                    },
                },
                StreamEvent::ToolInputDelta {
                    index: 0,
                    json: "{}".into(),
                },
                StreamEvent::BlockEnd { index: 0 },
                StreamEvent::Done {
                    stop: StopReason::ToolUse,
                },
            ]),
            text_turn("recovered"),
        ]);
        agent.run_turn("go".into(), CancellationToken::new()).await;

        let wire = agent.session().wire_messages("claude-opus-5");
        let results: Vec<_> = wire
            .iter()
            .flat_map(|m| &m.content)
            .filter(|c| {
                matches!(
                    c,
                    crate::provider::WireContent::ToolResult { tool_use_id, .. } if tool_use_id == "toolu_7"
                )
            })
            .collect();
        assert_eq!(results.len(), 1, "every tool_use needs exactly one result");
    }

    #[tokio::test]
    async fn a_budget_ceiling_stops_the_turn() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut agent = Agent::new(
            Arc::new(ScriptedProvider::new(vec![Script::Events(vec![
                StreamEvent::Usage(Usage {
                    input_tokens: 1_000_000,
                    output_tokens: 1_000_000,
                    ..Default::default()
                }),
                StreamEvent::Done {
                    stop: StopReason::EndTurn,
                },
            ])])),
            Arc::new(NonInteractive::deny()),
            Session::new(PathBuf::from("/w"), "claude-opus-5"),
            RuntimeConfig {
                max_usd_per_turn: Some(1.0),
                ..Default::default()
            },
            vec![],
            tx,
        );
        match agent
            .run_turn("spend".into(), CancellationToken::new())
            .await
        {
            TurnOutcome::BudgetExceeded {
                spent_usd,
                limit_usd,
            } => {
                assert!(spent_usd > limit_usd);
                assert_eq!(limit_usd, 1.0);
            }
            other => panic!("expected the budget to trip, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn reasoning_blocks_survive_into_the_next_request_verbatim() {
        let (mut agent, _rx) = harness(vec![
            Script::Events(vec![
                StreamEvent::BlockStart {
                    index: 0,
                    kind: BlockKind::Thinking,
                },
                StreamEvent::ReasoningDelta {
                    index: 0,
                    text: "considering".into(),
                },
                StreamEvent::ReasoningSignature {
                    index: 0,
                    signature: "sig-abc".into(),
                },
                StreamEvent::BlockEnd { index: 0 },
                StreamEvent::BlockStart {
                    index: 1,
                    kind: BlockKind::ToolUse {
                        id: "toolu_1".into(),
                        name: "read".into(),
                    },
                },
                StreamEvent::ToolInputDelta {
                    index: 1,
                    json: "{}".into(),
                },
                StreamEvent::BlockEnd { index: 1 },
                StreamEvent::Done {
                    stop: StopReason::ToolUse,
                },
            ]),
            text_turn("done"),
        ]);
        agent.run_turn("go".into(), CancellationToken::new()).await;

        let wire = agent.session().wire_messages("claude-opus-5");
        let thinking = wire.iter().flat_map(|m| &m.content).find_map(|c| match c {
            crate::provider::WireContent::Thinking {
                thinking,
                signature,
            } => Some((thinking.clone(), signature.clone())),
            _ => None,
        });
        assert_eq!(
            thinking,
            Some(("considering".to_owned(), "sig-abc".to_owned())),
            "the signature must survive or the block is rejected"
        );
    }
}
