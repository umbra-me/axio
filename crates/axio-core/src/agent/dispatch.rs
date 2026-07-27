//! Planning, authorising and running the tools one step asked for.
//!
//! The order is the design: everything is planned first, the approver sees each
//! plan before anything runs, reads go concurrently and writes go in call order,
//! and every call answers in the same message whatever happened to it.

use super::*;

/// A call that has been planned and authorised, or already answered.
enum Planned {
    Run(String, Arc<dyn Tool>, Plan),
    Resolved(String, ToolStatus),
}

impl Agent {
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
    pub(super) async fn dispatch(
        &mut self,
        turn: TurnId,
        calls: Vec<(String, String, serde_json::Value)>,
        cancel: &CancellationToken,
    ) {
        // ---- phase one: plan and authorise, in call order.
        let mut planned: Vec<Planned> = Vec::with_capacity(calls.len());

        for (call_id, name, input) in calls {
            let Some(tool) = self.tools.get(&name).cloned() else {
                self.label(turn, &call_id, &name);
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
                self.label(turn, &call_id, tool.name());
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
                    //
                    self.label(turn, &call_id, tool.name());
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
    /// Give a call that never produced a plan something to be known by.
    ///
    /// Three things can end a call before it has a subject: no such tool,
    /// arguments the schema rejects, and a `plan` that fails. All three used to
    /// leave the item's subject empty, so the surface rendered `invalid
    /// arguments: …` against a blank name and the reader was left to guess
    /// which of six tools had been called. Found by watching a real session:
    /// four of them in a row, none of them identifiable.
    ///
    /// Nothing is authorised by this. The subject of a call that never ran is a
    /// label, and policy is not consulted again after it.
    fn label(&mut self, turn: TurnId, call_id: &str, name: &str) {
        if let Some(item) = self.session.set_tool_plan(call_id, name, None) {
            self.emit(Some(turn), EventKind::ItemUpdated { item });
        }
    }

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
}
