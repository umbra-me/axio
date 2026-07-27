//! The turn loop.
//!
//! One user turn in, one `TurnEnded` out — on every exit path, including
//! cancellation and failure. A surface that counts `TurnStarted` and
//! `TurnEnded` can rely on them balancing.

mod dispatch;
mod runtime;
mod sample;
mod turn;

pub use runtime::RuntimeConfig;
use turn::TurnBreak;

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

    /// The model the next request will name.
    pub fn model(&self) -> &str {
        &self.cfg.model
    }

    /// The model that minted this transcript.
    ///
    /// Exposed beside [`Agent::model`] so a caller can tell whether the two
    /// have parted, which is the condition the warning in `announce` is about.
    pub fn session_model(&self) -> &str {
        self.session.model()
    }

    /// Point the next request at a different model.
    ///
    /// The provider is untouched. This is the name in the request body, so it
    /// reaches only models the configured provider already serves — a name
    /// from somewhere else is a request that fails, not a provider that
    /// changes.
    ///
    /// Leaving the transcript's own model behind costs its reasoning: the
    /// projection drops blocks minted by a different one, which is the silent
    /// loss `announce` warns about on a resume. Nothing here warns, because
    /// nothing here knows whether a person is watching — the caller compares
    /// [`Agent::model`] with [`Agent::session_model`] and says so in whatever
    /// way its surface says things.
    pub fn set_model(&mut self, model: impl Into<String>) {
        self.cfg.model = model.into();
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

    /// A cap the provider cannot measure is not a cap. Silently inert is the
    /// worst outcome for a guardrail: the user believes they set one.
    #[tokio::test]
    async fn a_budget_that_cannot_be_measured_says_so() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut agent = Agent::new(
            Arc::new(ScriptedProvider::say("hi").unpriced()),
            Arc::new(NonInteractive::deny()),
            Session::new(PathBuf::from("/w"), "gpt-oss:120b"),
            RuntimeConfig {
                max_usd_per_turn: Some(2.0),
                ..Default::default()
            },
            vec![],
            tx,
        );
        agent.announce(false, vec![]);

        let mut warned = false;
        while let Ok(event) = rx.try_recv() {
            if let EventKind::Notice { level, message } = &event.kind
                && *level == NoticeLevel::Warn
                && message.contains("cannot trip")
            {
                warned = true;
            }
        }
        assert!(warned, "an inert spend cap must announce itself");
    }

    /// And a provider that does report prices must not be warned about.
    #[tokio::test]
    async fn a_budget_that_can_be_measured_is_not_warned_about() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut agent = Agent::new(
            Arc::new(ScriptedProvider::say("hi")),
            Arc::new(NonInteractive::deny()),
            Session::new(PathBuf::from("/w"), "claude-opus-5"),
            RuntimeConfig {
                max_usd_per_turn: Some(2.0),
                ..Default::default()
            },
            vec![],
            tx,
        );
        agent.announce(false, vec![]);
        while let Ok(event) = rx.try_recv() {
            if let EventKind::Notice { message, .. } = &event.kind {
                assert!(!message.contains("cannot trip"), "{message}");
            }
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
