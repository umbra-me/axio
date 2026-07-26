# Roadmap

Milestones toward v0.1. Each one is done when its acceptance criteria are
checkable and checked, not when the code exists.

## Done

### M0 — hygiene gate

Four-crate workspace, three-OS CI, `cargo-deny`, and three scripts that enforce
structure rather than describing it: `firewall.sh`, `limits.sh`, `deps.sh`.

### M1 — protocol and provider

The event vocabulary, the `Provider` / `Tool` / `Approver` traits, `Workspace`
path confinement, `Redacted`, and the Messages transport: a hand-written SSE
decoder, the request builder, the block state machine and error classification.

### M2 — turn loop and one-shot CLI

`Agent::run_turn` in its final shape, the session transcript and its wire
projection, surface selection, both renderers, signal handling and exit codes,
`--doctor`.

### M3 — tools, approval, cancellation

Six tools, the plan/authorise/execute split, the ordered permission engine with
its built-in deny list, process-group containment, output truncation with spill,
and environment stripping.

### M4 — session, config, compaction

Append-only JSONL records with a versioned header, resume by replay, `--list`,
`--ephemeral`, layered configuration with per-section salvage and `--explain`,
deterministic compaction, per-turn cost records and budget enforcement.

### M5 — packaging, and the interactive surface

Tag-driven release workflow for five targets, `cargo install`, a changelog, and
`scripts/live-check.sh` — a real turn against a real model rather than a stub.
The interactive surface arrived here rather than in v0.2: an inline viewport, a
composer, and an `Approver` that shows the diff before it asks.

## Next

### v0.1 — the release itself

Everything M5 needs is built and green. What remains is the decision to tag it,
and one thing no amount of code can supply: `scripts/live-check.sh` has never
been run against the Anthropic provider, because there has never been a key.

## After v0.1

- **v0.2 — interactive ergonomics.** A coalescing frame requester, multi-line
  composer entry, and per-hunk approval if it earns its place.
- **v0.3 — durability and ergonomics.** Syntax highlighting, session search,
  better diffs.
- **v0.4 — extension surface.** Gated on demand, not on schedule.

## Deliberately not building

Each of these has a tripwire that would change the decision. Recording the
tripwire is the point: a deferral without one is just a backlog item.

| Deferred | Tripwire |
| --- | --- |
| Desktop application | Nothing in this horizon. The architecture keeps it cheap: the protocol module stays dependency-free and serde-derived, renderers keep taking a receiver and an approver, and no capability attaches to a surface |
| A server or daemon | A second process genuinely exists, or headless/remote becomes a goal |
| Typed IPC codegen | A non-Rust consumer that already exists and is load-bearing |
| Tool-protocol client | Two independent requests for a specific server |
| Language-server diagnostics | A long session where the model repeatedly ships code failing the next typecheck. The shell tool already runs the project's own checks |
| Sandboxing | The first unattended run against untrusted input. Linux lands first, and the README claim becomes "sandboxed on Linux only" |
| A database for sessions | Measured `--list` latency above 200ms on a real machine. A sidecar index comes first |
| Per-hunk edit approval | Three occasions of wanting to accept part of a diff |
| Checkpoints and undo | Working outside a git repository becomes routine, or one bad turn destroys uncommitted work |
| Subagents and multi-agent | Nothing in this horizon |
| A local pre-push gate | A regression that plain CI would have caught reaches a release |
