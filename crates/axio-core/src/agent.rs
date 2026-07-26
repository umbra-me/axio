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
use crate::compact::{self, ContextBudget, Elisions};
use crate::policy::{Policy, Verdict};
use crate::protocol::{
    ApprovalId, ApprovalRequest, Decision, Delta, Event, EventKind, Item, ItemBody, ItemId,
    NoticeLevel, PROTOCOL_VERSION, Preview, SessionId, ToolStatus, TurnId, TurnOutcome, Usage,
};
use crate::provider::{
    BlockKind, Effort, ModelRequest, Provider, ProviderError, ReasoningDisplay, StopReason,
    StreamEvent, SystemBlock, ToolInputAccumulator, ToolSpec,
};
use crate::record::{Record, Recorder};
use crate::session::Session;
use crate::tool::{
    Plan, ProgressSink, Tool, ToolCx, ToolEnv, ToolError, ToolLimits, ToolOutput, Workspace,
};

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
    /// The cap applied to every tool result, at one choke point in the loop.
    pub max_output_bytes: usize,
    /// Where output too large to send is kept so the model can read it back.
    pub spill_dir: Option<std::path::PathBuf>,
    pub tool_limits: ToolLimits,
}

impl RuntimeConfig {
    /// The only way to build one from configuration.
    ///
    /// Everything else in the loop reads `self.cfg`, so funnelling construction
    /// through here is what makes "config is resolved once" true by
    /// construction rather than by everyone remembering.
    pub fn from_resolved(resolved: &crate::config::Resolved) -> Self {
        let c = resolved.config();
        Self {
            model: c.model.name.clone(),
            effort: c.model.effort,
            reasoning: c.reasoning(),
            max_tokens: c.model.max_tokens,
            max_steps: c.budget.max_steps,
            max_usd_per_turn: c.budget.max_usd_per_turn,
            tool_limits: c.tool_limits(),
            // The loop's own choke point reads this field, so leaving it at the
            // default made `[tools] max_output_bytes` a setting that `--explain`
            // reported and nothing obeyed.
            max_output_bytes: c.tools.max_output_bytes,
            ..Self::default()
        }
    }

    /// `--resume`. The model comes from the session header, because a
    /// transcript's reasoning blocks are only replayable under the model that
    /// minted them.
    pub fn adopt_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }
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
            max_output_bytes: 64 * 1024,
            spill_dir: None,
            tool_limits: ToolLimits::default(),
        }
    }
}

/// A call that has been planned and authorised, or already answered.
enum Planned {
    Run(String, Arc<dyn Tool>, Plan),
    Resolved(String, ToolStatus),
}

/// Turn a tool's return into a terminal status, applying the loop's output
/// policy on the way.
fn finish_status(
    outcome: Result<ToolOutput, ToolError>,
    started: std::time::Instant,
    limit: usize,
    spill_dir: Option<&std::path::Path>,
    call_id: &str,
) -> ToolStatus {
    let ms = started.elapsed().as_millis() as u64;
    match outcome {
        Ok(out) => {
            // The call id, not a constant: two large outputs in one session
            // otherwise land on the same file, and the first one's marker keeps
            // promising content the second one overwrote.
            let capped = crate::truncate::finish(&out.content, limit, spill_dir, call_id);
            match capped {
                Ok(capped) => ToolStatus::Ok {
                    output: capped.content,
                    truncated: capped.truncated,
                    spill: capped.spill,
                    ms,
                },
                // Failing to write a spill file must not lose the output; send
                // what fits and say so.
                Err(_) => {
                    let fallback = crate::truncate::cap(&out.content, limit);
                    ToolStatus::Ok {
                        output: fallback.text,
                        truncated: fallback.truncated,
                        spill: None,
                        ms,
                    }
                }
            }
        }
        Err(ToolError::Cancelled) => ToolStatus::Cancelled,
        Err(e) => ToolStatus::Failed {
            message: e.to_string(),
        },
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
    /// Paired with the id the streaming events already used.
    ///
    /// Generating a fresh one when the block lands in the transcript made every
    /// tool call appear twice under `--json`: once reaching `item_completed`
    /// still `pending` and never resolving, and once as an `item_updated` for
    /// an item the consumer never saw start. `id` is the natural key — it is
    /// all `item_delta` carries — so a surface built on it shows a permanently
    /// spinning phantom call.
    blocks: Vec<Item>,
    usage: Usage,
    stop: Option<StopReason>,
}

impl Sampled {
    fn text(&self) -> String {
        self.blocks
            .iter()
            .filter_map(|b| match &b.body {
                ItemBody::AgentMessage { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    fn tool_calls(&self) -> Vec<(String, String, serde_json::Value)> {
        self.blocks
            .iter()
            .filter_map(|b| match &b.body {
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
    approver: Arc<dyn Approver>,
    session: Session,
    cfg: RuntimeConfig,
    policy: Policy,
    workspace: Arc<Workspace>,
    env: Arc<ToolEnv>,
    system: Arc<[SystemBlock]>,
    /// Where the durable history goes. The file records what happened;
    /// compaction never writes to it.
    recorder: Recorder,
    /// What the next request leaves out. Re-derived per step, and per resume,
    /// because it is a pure function of the transcript.
    elisions: Elisions,
    context: ContextBudget,
    /// Accumulated across the session, so `--json` can show what a resumed
    /// session has cost in total rather than only this turn.
    session_usage: Usage,
    /// Unbounded deliberately: a bounded send from inside the loop can deadlock
    /// against a surface that is slow to drain.
    events: mpsc::UnboundedSender<Event>,
    seq: u64,
    /// Subjects already refused this turn.
    ///
    /// A model told only "this needs approval" reads the refusal as transient
    /// and re-sends the identical call — observed eight times for one write,
    /// and six for one `ls`. The second answer says so plainly instead of
    /// repeating the first, and an interactive approver is not asked the same
    /// question twice in one turn. Cleared when a session grant arrives, since
    /// that is the one thing that can change the answer.
    denied_this_turn: std::collections::HashSet<String>,
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
        let mut workspace = Workspace::new(session.cwd())
            .unwrap_or_else(|_| Workspace::unchecked(session.cwd().clone()));
        if let Some(spill) = &cfg.spill_dir {
            // Otherwise the truncation marker names a path that `read` refuses,
            // and recovering a large output costs a shell command or a lot of
            // wasted steps.
            workspace = workspace.with_readable(spill);
        }
        let workspace = Arc::new(workspace);
        Self {
            provider,
            tools: BTreeMap::new(),
            approver,
            session,
            cfg,
            policy: Policy::default(),
            workspace,
            env: Arc::new(ToolEnv::default()),
            system: Arc::from(system),
            recorder: Recorder::Ephemeral,
            elisions: Elisions::default(),
            context: ContextBudget::default(),
            session_usage: Usage::default(),
            events,
            seq: 0,
            denied_this_turn: std::collections::HashSet::new(),
        }
    }

    /// Where to record what happens. Defaults to recording nothing.
    pub fn with_recorder(mut self, recorder: Recorder) -> Self {
        self.recorder = recorder;
        self
    }

    pub fn session_path(&self) -> Option<&std::path::Path> {
        self.recorder.path()
    }

    /// The permission engine. Supplied by the surface, which is the only place
    /// that knows whether a human is available to ask.
    pub fn with_policy(mut self, policy: Policy) -> Self {
        self.policy = policy;
        self
    }

    /// The sanitised environment children inherit.
    pub fn with_env(mut self, env: ToolEnv) -> Self {
        self.env = Arc::new(env);
        self
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
    pub fn announce(&mut self, resumed: bool, notices: Vec<crate::protocol::Notice>) {
        let kind = EventKind::SessionStarted {
            protocol: PROTOCOL_VERSION,
            session: self.session.id(),
            model: self.cfg.model.clone(),
            cwd: self.session.cwd().clone(),
            effort: self.cfg.effort,
            resumed,
        };
        self.emit(None, kind);

        // Everything resolved before the agent existed — config salvage,
        // session load, budget validation — replayed through the same counter,
        // because `seq` being gap-free is a promise `--json` makes and a
        // surface has no way to compute one.
        for notice in notices {
            self.emit(
                None,
                EventKind::Notice {
                    level: notice.level,
                    message: notice.message,
                },
            );
        }

        if self.budget_is_inert() {
            let limit = self.cfg.max_usd_per_turn.unwrap_or_default();
            self.emit(
                None,
                EventKind::Notice {
                    level: NoticeLevel::Warn,
                    message: format!(
                        "budget.max_usd_per_turn is set to {limit:.2}, but this provider \
                         reports no prices for `{}` — nothing measures spend here, so the \
                         cap cannot trip",
                        self.cfg.model
                    ),
                },
            );
        }

        // The one silent data-loss path in the projection: a model that differs
        // from the one that minted the transcript drops every reasoning block.
        if self.cfg.model != self.session.model() {
            self.emit(
                None,
                EventKind::Notice {
                    level: NoticeLevel::Warn,
                    message: format!(
                        "this session was recorded with {} but is running under {};                          earlier reasoning will not be replayed",
                        self.session.model(),
                        self.cfg.model
                    ),
                },
            );
        }
    }

    /// One user turn. Every exit path emits exactly one `TurnEnded`.
    pub async fn run_turn(&mut self, input: String, cancel: CancellationToken) -> TurnOutcome {
        let turn = TurnId::generate();
        // A refusal is a fact about this turn, not about the session: a new
        // prompt may legitimately be about the file the last one was refused.
        self.denied_this_turn.clear();
        let user_item = self.session.push_user(&input);
        self.recorder.append_item(&user_item);
        self.emit(Some(turn), EventKind::TurnStarted);

        let mut usage = Usage::default();
        let mut steps = 0u32;

        let outcome = 'turn: {
            while steps < self.cfg.max_steps {
                steps += 1;

                // One choke point, once per step, before the wire shape is
                // built. Neither surface has, or needs, its own pass.
                self.compact(turn);

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
                // Per step, cumulative. A caller could otherwise not notice a
                // turn running away until the one event that reports it — the
                // last one — and by then it has been paid for.
                self.emit(Some(turn), EventKind::Usage(usage));

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
                for item in &sampled.blocks {
                    // The same id the stream used, so one call has one identity
                    // from `item_started` to its terminal status.
                    self.session.push_item(item.clone());
                    self.recorder.append_item(item);
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

        self.session_usage.add(&usage);
        let cost_usd = self.provider.model_info(&self.cfg.model).cost_usd(&usage);
        self.recorder.append(&Record::TurnEnded {
            outcome: outcome.clone(),
            usage,
            cost_usd,
        });

        let files_changed = self.session.take_files_changed();
        self.emit(
            Some(turn),
            EventKind::TurnEnded {
                outcome: outcome.clone(),
                usage,
                cost_usd,
                files_changed,
            },
        );
        outcome
    }

    /// Re-derive what the next request leaves out.
    ///
    /// Pure in the transcript, so a resumed session reproduces the same plan
    /// rather than drifting a little further each time.
    fn compact(&mut self, turn: TurnId) {
        let before = compact::estimate_tokens(self.session.transcript());
        let planned = compact::plan(self.session.transcript(), self.context);
        if planned == self.elisions {
            return;
        }
        self.elisions = planned;
        if self.elisions.is_empty() {
            return;
        }
        let projected = compact::apply(self.session.transcript(), &self.elisions);
        let after = compact::estimate_tokens(&projected);
        let stage = self.elisions.stage;
        let dropped = self.elisions.dropped_prefix as u32;
        self.recorder.append(&Record::Compacted {
            stage,
            dropped,
            tokens_before: before,
            tokens_after: after,
        });
        self.emit(
            Some(turn),
            EventKind::Compacted {
                stage,
                tokens_before: before,
                tokens_after: after,
            },
        );
    }

    /// Elide more, because the provider refused the request as too large.
    ///
    /// Returns false when there is nothing left to give up, which is the
    /// caller's cue to fail with an explicit message rather than retry forever.
    fn force_compact(&mut self, turn: TurnId) -> bool {
        let Some(next) = compact::force(self.session.transcript(), &self.elisions) else {
            return false;
        };
        self.elisions = next;
        self.notice(
            turn,
            NoticeLevel::Warn,
            "the context window overflowed; compacting further and retrying".to_owned(),
        );
        true
    }

    fn build_request(&self) -> ModelRequest {
        let mut req = ModelRequest::new(&self.cfg.model);
        req.system = self.system.clone();
        let projected = compact::apply(self.session.transcript(), &self.elisions);
        req.messages = self.session.wire_messages_from(&projected, &self.cfg.model);
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

    /// A spend cap that nothing can measure is not a cap.
    ///
    /// The unpriced providers report zero for every token, so `cost_usd` is
    /// always `0.0` and the comparison can never be true. Silently inert is the
    /// worst outcome for a guardrail: the user believes they set one.
    fn budget_is_inert(&self) -> bool {
        self.cfg.max_usd_per_turn.is_some() && {
            let info = self.provider.model_info(&self.cfg.model);
            info.input_price == 0.0 && info.output_price == 0.0
        }
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
        let mut forced = false;
        let mut req = req;
        loop {
            // A child token so cancelling the turn cancels the in-flight
            // request, while a per-request failure never poisons the turn's own
            // token.
            let child = cancel.child_token();
            let item = ItemId::generate();
            match self.stream_once(turn, item, req.clone(), child).await {
                Ok(sampled) => return Ok(sampled),

                Err(ProviderError::Cancelled) => return Err(TurnBreak::Cancelled),

                // Forced compaction plus exactly one retry. Without it the
                // first long session dies on a hard 400 with the answer one
                // elision away.
                Err(ProviderError::ContextOverflow) if !forced => {
                    forced = true;
                    if !self.force_compact(turn) {
                        return Err(TurnBreak::Overflow);
                    }
                    req = self.build_request();
                }
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
                    let item = Item { id, body };
                    self.emit(Some(turn), EventKind::ItemCompleted { item: item.clone() });
                    sampled.blocks.push(item);
                }
                // Cumulative per message, so take the maximum rather than
                // summing: message_start and message_delta both report the
                // running total for the same message.
                StreamEvent::Usage(u) => sampled.usage.merge_cumulative(&u),
                StreamEvent::Done { stop } => sampled.stop = Some(stop),
            }
        }

        Ok(sampled)
    }

    /// Execute tool calls.
    ///
    /// Two phases, and the split is deliberate. Everything is planned and
    /// authorised **first**, in call order, before anything executes — so a
    /// batch containing one refused call does not half-run. Then read-only
    /// calls execute concurrently while everything else runs serially in call
    /// order, because `write(a.rs)` and `edit(a.rs)` in one batch must not race.
    ///
    /// Every call yields a result, whatever happens to it. A `tool_use` with no
    /// matching `tool_result` makes the next request invalid, so an unknown
    /// tool, a refused call, a panicking tool and a cancelled one all record
    /// why against their real call id.
    async fn dispatch(
        &mut self,
        turn: TurnId,
        calls: Vec<(String, String, serde_json::Value)>,
        cancel: &CancellationToken,
    ) {
        // ---- phase one: plan and authorise, in call order.
        let mut planned: Vec<Planned> = Vec::with_capacity(calls.len());

        for (call_id, name, input) in calls {
            let Some(tool) = self.tools.get(&name).cloned() else {
                planned.push(Planned::Resolved(
                    call_id,
                    ToolStatus::Failed {
                        message: format!("unknown tool: {name}"),
                    },
                ));
                continue;
            };

            if let Err(e) =
                crate::tool::reject_unknown_arguments(&input, tool.schema(), tool.name())
            {
                planned.push(Planned::Resolved(
                    call_id,
                    ToolStatus::Failed {
                        message: e.to_string(),
                    },
                ));
                continue;
            }

            let cx = self.tool_cx(cancel.child_token());
            let plan = match tool.plan(&input, &cx).await {
                Ok(plan) => plan,
                Err(e) => {
                    // A bad argument is a tool_result, not a turn failure: the
                    // model can read it and try again.
                    planned.push(Planned::Resolved(
                        call_id,
                        ToolStatus::Failed {
                            message: e.to_string(),
                        },
                    ));
                    continue;
                }
            };

            self.record_plan(turn, &call_id, &plan);

            // Already refused this turn: answer from the record rather than
            // asking again. Repeating the first message invites the model to
            // read the refusal as transient and try once more, and an
            // interactive approver should not be asked the same question twice.
            if self.denied_this_turn.contains(&plan.subject) {
                let subject = plan.subject.clone();
                planned.push(Planned::Resolved(
                    call_id,
                    ToolStatus::Denied {
                        message: format!(
                            "`{subject}` was already refused earlier in this turn and the \
                             answer has not changed. Stop retrying it and report what you \
                             could not do."
                        ),
                    },
                ));
                continue;
            }

            match self.policy.evaluate(&plan) {
                Verdict::Allow => planned.push(Planned::Run(call_id, tool, plan)),
                Verdict::Deny(reason) => {
                    self.denied_this_turn.insert(plan.subject.clone());
                    planned.push(Planned::Resolved(
                        call_id,
                        ToolStatus::Denied { message: reason },
                    ));
                }
                Verdict::Ask(reason) => {
                    let request = ApprovalRequest {
                        id: ApprovalId::generate(),
                        call_id: call_id.clone(),
                        tool: tool.name().to_owned(),
                        subject: plan.subject.clone(),
                        effects: plan.effects,
                        preview: plan.preview.clone(),
                        reason,
                    };
                    self.set_status(turn, &call_id, ToolStatus::AwaitingApproval);
                    self.emit(
                        Some(turn),
                        EventKind::ApprovalRequested {
                            id: request.id,
                            request: request.clone(),
                        },
                    );

                    // Cancelling while waiting is a refusal, not a hang.
                    let decision = tokio::select! {
                        biased;
                        _ = cancel.cancelled() => Decision::Deny {
                            feedback: Some("cancelled before approval".into()),
                        },
                        d = self.approver.decide(&request) => d,
                    };
                    self.emit(
                        Some(turn),
                        EventKind::ApprovalResolved {
                            id: request.id,
                            decision: decision.clone(),
                        },
                    );

                    match decision {
                        Decision::Allow => planned.push(Planned::Run(call_id, tool, plan)),
                        Decision::AllowSession => {
                            self.policy.grant(&plan.subject);
                            // A grant is the one thing that changes a previous
                            // no, so the memo of refusals stops being valid.
                            self.denied_this_turn.clear();
                            planned.push(Planned::Run(call_id, tool, plan));
                        }
                        Decision::Deny { feedback } => {
                            self.denied_this_turn.insert(plan.subject.clone());
                            planned.push(Planned::Resolved(
                                call_id,
                                ToolStatus::Denied {
                                    // The feedback becomes what the model reads,
                                    // so "no, use the existing helper" steers
                                    // rather than dead-ends.
                                    message: feedback
                                        .unwrap_or_else(|| "denied by the user".into()),
                                },
                            ));
                        }
                    }
                }
            }
        }

        // ---- phase two: execute.
        let mut results: Vec<(usize, String, ToolStatus)> = Vec::new();
        let mut concurrent = tokio::task::JoinSet::new();

        for (index, item) in planned.into_iter().enumerate() {
            match item {
                Planned::Resolved(call_id, status) => results.push((index, call_id, status)),
                Planned::Run(call_id, tool, plan) if plan.effects.parallel_safe() => {
                    let cx = self.tool_cx(cancel.child_token());
                    let limit = self.cfg.max_output_bytes;
                    let spill = self.spill_dir();
                    concurrent.spawn(async move {
                        let started = std::time::Instant::now();
                        let outcome = tool.run(plan, &cx).await;
                        let status =
                            finish_status(outcome, started, limit, spill.as_deref(), &call_id);
                        (index, call_id, status)
                    });
                }
                Planned::Run(call_id, tool, plan) => {
                    self.set_status(turn, &call_id, ToolStatus::Running);
                    let cx = self.tool_cx(cancel.child_token());
                    let started = std::time::Instant::now();
                    let outcome = tool.run(plan, &cx).await;
                    let spill = self.spill_dir();
                    let status = finish_status(
                        outcome,
                        started,
                        self.cfg.max_output_bytes,
                        spill.as_deref(),
                        &call_id,
                    );
                    results.push((index, call_id, status));
                }
            }
        }

        while let Some(joined) = concurrent.join_next().await {
            match joined {
                Ok(result) => results.push(result),
                // A panicking tool must still answer, and with its real call
                // id — an empty or invented one makes the next request invalid.
                Err(e) => {
                    tracing::error!("a tool panicked: {e}");
                }
            }
        }

        // Completion order is not call order, and the wire needs call order.
        results.sort_by_key(|(index, _, _)| *index);
        for (_, call_id, status) in results {
            self.set_status(turn, &call_id, status);
        }

        // Anything that never produced a result — a panicked task, an aborted
        // one — is still pending in the transcript. Give it an answer.
        self.resolve_pending(turn);
    }

    /// Record the subject and preview a plan produced, so a surface can show
    /// what is about to happen and the transcript records what was decided.
    fn record_plan(&mut self, turn: TurnId, call_id: &str, plan: &Plan) {
        if let Some(item) = self
            .session
            .set_tool_plan(call_id, &plan.subject, plan.preview.clone())
        {
            self.emit(Some(turn), EventKind::ItemUpdated { item });
        }
    }

    fn set_status(&mut self, turn: TurnId, call_id: &str, status: ToolStatus) {
        if let Some(item) = self.session.set_tool_status(call_id, status) {
            // A call that succeeded and previewed a diff changed that file.
            // Without this, `files_changed` on `TurnEnded` is always empty —
            // present in the protocol, emitted every turn, and never true, which
            // is worse for a `--json` consumer than not being there at all.
            //
            // Only what is knowable: `write` and `edit` name their file, and a
            // shell command that rewrites a tree does not. Guessing at the rest
            // would trade a visible gap for an invisible wrong answer.
            if let ItemBody::ToolCall {
                status: ToolStatus::Ok { .. },
                preview: Some(Preview::Diff { path, .. }),
                ..
            } = &item.body
            {
                self.session.record_file_changed(path.clone());
            }
            self.recorder.append_item(&item);
            self.emit(Some(turn), EventKind::ItemUpdated { item });
        }
    }

    /// Give every unfinished call a terminal status.
    fn resolve_pending(&mut self, turn: TurnId) {
        for call_id in self.session.unfinished_calls() {
            self.set_status(turn, &call_id, ToolStatus::Cancelled);
        }
    }

    fn spill_dir(&self) -> Option<std::path::PathBuf> {
        self.cfg
            .spill_dir
            .as_ref()
            .map(|root| root.join(self.session.id().to_string()))
    }

    fn tool_cx(&self, cancel: CancellationToken) -> ToolCx {
        ToolCx {
            workspace: self.workspace.clone(),
            cancel,
            progress: ProgressSink::null(),
            limits: self.cfg.tool_limits,
            env: self.env.clone(),
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
        agent.announce(false, Vec::new());
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
        agent.announce(false, Vec::new());
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
