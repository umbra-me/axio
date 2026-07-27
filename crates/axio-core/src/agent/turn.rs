//! One user turn: the loop, its compaction, and its budget.
//!
//! Every exit path ends in exactly one `TurnEnded` — completion, refusal,
//! cancellation, a step ceiling, a budget ceiling or a failure. That is the
//! property the whole surface contract rests on.

use super::*;

/// Why a turn stopped sampling.
pub(super) enum TurnBreak {
    Cancelled,
    Overflow,
    Fatal(String),
}

impl Agent {
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
    pub(super) fn compact(&mut self, turn: TurnId) {
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
    pub(super) fn force_compact(&mut self, turn: TurnId) -> bool {
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

    pub(super) fn build_request(&self) -> ModelRequest {
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
    pub(super) fn budget_is_inert(&self) -> bool {
        self.cfg.max_usd_per_turn.is_some() && {
            let info = self.provider.model_info(&self.cfg.model);
            info.input_price == 0.0 && info.output_price == 0.0
        }
    }

    pub(super) fn check_budget(&self, usage: &Usage) -> Option<TurnOutcome> {
        let limit = self.cfg.max_usd_per_turn?;
        let spent = self.provider.model_info(&self.cfg.model).cost_usd(usage);
        (spent > limit).then_some(TurnOutcome::BudgetExceeded {
            spent_usd: spent,
            limit_usd: limit,
        })
    }
}
