//! Asking the model, and reading what comes back.
//!
//! One request, its retries, and the block state machine that turns a stream of
//! deltas into finished items. Nothing here decides anything about tools — it
//! reports what the model asked for.

use super::*;

/// One fully-buffered assistant message.
#[derive(Debug, Default)]
pub(super) struct Sampled {
    /// Paired with the id the streaming events already used.
    ///
    /// Generating a fresh one when the block lands in the transcript made every
    /// tool call appear twice under `--json`: once reaching `item_completed`
    /// still `pending` and never resolving, and once as an `item_updated` for
    /// an item the consumer never saw start. `id` is the natural key — it is
    /// all `item_delta` carries — so a surface built on it shows a permanently
    /// spinning phantom call.
    pub(super) blocks: Vec<Item>,
    pub(super) usage: Usage,
    pub(super) stop: Option<StopReason>,
}

impl Sampled {
    pub(super) fn text(&self) -> String {
        self.blocks
            .iter()
            .filter_map(|b| match &b.body {
                ItemBody::AgentMessage { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    pub(super) fn tool_calls(&self) -> Vec<(String, String, serde_json::Value)> {
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

impl Agent {
    /// Sample one assistant message, retrying a retryable failure.
    pub(super) async fn sample(
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
}
