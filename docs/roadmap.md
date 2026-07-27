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
A second dialect followed — chat-completions, reached by the provider names
`ollama` and `openai-compatible` — which is how the trait was shown to be an
abstraction rather than one implementation with a header on it.

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

Tag-driven release workflow for five targets, `cargo install --git`, a changelog, and
`scripts/live-check.sh` — a real turn against a real model rather than a stub.
The interactive surface arrived here rather than in v0.2: an inline viewport, a
composer, and an `Approver` that shows the diff before it asks.

### M6 — the optional sandbox

Landlock on Linux, off by default: `--sandbox` or `[sandbox] enabled`, with
`read` and `write` for whatever a toolchain needs beyond the workspace. Applied
before the runtime starts, so every command axio spawns inherits it. Filesystem
only, and a second wall behind the permission engine rather than a replacement
for it.

## Next

### v0.1 — the release itself

Everything M5 needs is built and green. What remains is the decision to tag it,
and three things no amount of code can supply:

- ~~A real turn against a real model.~~ **Done.** `scripts/live-check.sh` was
  run against the chat-completions path and passed all thirteen checks, so the
  turn loop, the tools, the refusal path, the deny list and resume are no longer
  proven only against a stub somebody here wrote.

  The Messages transport is **deliberately deferred**, not pending. It has never
  been exercised against the real endpoint, no credential exists for it, and
  waiting for one would hold the release hostage to a signup. It ships labelled
  — in the README, in the changelog and by `--doctor` — and the label is the
  deal: a provider nobody has run is presented as a provider nobody has run.
  Verifying it is a half-hour job the day a key exists, and until then the
  supported path is the one that has been proven.
- **An install on a machine that has never built Rust.** Everything checkable
  has been checked: no `aws-lc-sys`, `onig_sys`, `openssl-sys`, `cmake` or
  `bindgen` in the graph for any target — now enforced by `deny.toml` rather
  than left to inspection — and `ring` hands its pre-assembled objects to the
  linker instead of invoking an assembler, so NASM is never run. CI compiles,
  links and tests the whole workspace on Windows every push.

  What none of that covers is the machine itself: a GitHub runner arrives with
  build tools a fresh laptop does not have. The test is three commands on a
  Windows box that has never built Rust — install rustup, accept the MSVC build
  tools it asks for, then `cargo install --git https://github.com/umbra-me/axio
  --locked axio` and run one turn. If it needs anything else, the dependency
  graph gained something it should not have.
- ~~A dogfooding session, written up here.~~ **Done, with one half open.** Two
  sessions: axio adding a feature to its own repository through its interactive
  surface, against a real model, with every approval displayed before it was
  answered. Both are written up in the changelog, including the code it got
  wrong and the two bugs of its own they exposed. A human still has not watched
  the approvals — both runs were driven programmatically — and that is the half
  that stays open.

### Not tagged yet, deliberately

Everything above is either done or bounded, and the tag is still being held.
Two sessions of real use produced four fixes on the day they ran, three of them
in code written the day before. That is not a reason to panic and it is not a
reason to hurry: a release is a promise that what is in it has settled, and the
honest thing to say about code this recently changed is that it has not.

What would move it: the Windows install above, a human watching a session, and
a few days in which nothing new turns up. The release workflow has itself never
run, so the first tag tests the tagging as much as the code — better done
deliberately than as an afterthought.

## After v0.1

The interactive ergonomics that were v0.2 arrived early — frame coalescing,
multi-line entry with bracketed paste, word-wise editing — as did v0.3's syntax
highlighting. What is left of each:

- **v0.2 — per-hunk approval**, if three occasions of wanting to accept part of
  a diff ever turn up.
- **v0.3 — durability.** Session search, and diffs worth looking at.
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
| A database for sessions | Measured `--list` latency above 200ms on a real machine. A sidecar index comes first |
| Per-hunk edit approval | Three occasions of wanting to accept part of a diff |
| Checkpoints and undo | Working outside a git repository becomes routine, or one bad turn destroys uncommitted work |
| Subagents and multi-agent | Nothing in this horizon |
| A local pre-push gate | A regression that plain CI would have caught reaches a release |
