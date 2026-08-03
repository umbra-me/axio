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

### M7 — asking the endpoint, and commands in the surface

`--probe`: two requests, one carrying a tool, reporting whether the provider
accepts it. `--doctor` stays offline and answers from configuration alone, so
the one question it cannot answer — a model that serves chat perfectly and
rejects every request with a tool in it — got its own flag rather than a
socket in the local modes.

Effort reaches the chat-completions dialect after all. It was dropped there on
the belief that the dialect had no equivalent; asking the endpoint settled it,
and the same asymmetry that settled it is now written down in the gotchas.

In the surface: a slash command menu over one `const` list, `/status`,
`/model` for changing model mid-session — bare, it offers what the provider
says it serves, which is why the seam grew a second method — and `/login`, a
credential typed into the viewport, never echoed and never in scrollback. Plus one line at startup
when the launch directory holds several repositories, because the workspace
being a whole shelf of projects is invisible until a search is slow and its
results are almost right.

### M8 — signing in, and a third dialect

Browser OAuth with PKCE for `openai-codex`, a credential store that holds a
token pair beside an API key, renewal before expiry with the renewed pair
persisted through a sink, and the Responses transport the subscription endpoint
requires. The client id is Codex's, vendored deliberately and said so in the
module that holds it. Verified against the live endpoint: a turn answered and
`--probe` reported a tool called.

`/model` became two stages — provider, then model — because changing one
without the other leaves a session naming a model its new endpoint has never
heard of. Every provider's list is fetched, including the subscription
endpoint's, whose three compiled-in names turned out to be wrong on the day
they were written. A switch that works is saved as the default, after the turn
rather than at the moment of choosing.

The home moved to `~/.axio` on every platform, which needed the project-config
walk taught that the user's own file is not a project's.

### M9 — what the limits are, and a desktop surface

`axio quota` and the fifth crate behind it: how much of each provider's limit is
left and when it resets, across ten providers reached by three different routes —
a credential another tool already wrote, an API key, or a browser session the
user grants. None of it touches axio's own credential store, which is the point
of it being a separate crate rather than a subcommand's module.

Behind an `app` feature, a Tauri desktop surface — tray icon, flyout and window,
with a React frontend built by Vite — carrying 189 packages that must never
reach a default `cargo install axio`, and verified absent from the default tree.

Signing in happens in a window against the vendor's own page rather than by
pasting a cookie header, because both obvious ways to copy a cookie produce
something every server refuses. Nothing on the machine is decrypted to do it.
Refresh intervals are computed from what the last probe found rather than fixed,
since the cost of being wrong is asymmetric in both directions.

### M10 — what it all cost

`axio cost` and the sixth crate: what the coding agents on this machine have
spent, read from the transcripts they already write. No network, no credentials.
Twenty-three agents — four hand-written parsers, twelve catalog rows, and eight
whose store is a database, behind a `sqlite` feature for the same
no-C-toolchain reason the `app` feature exists for.

Every token rule verified against real transcripts, three of them contradicting
what the formats look like at a glance. A model with no known rate is reported
unpriced, never as zero, and no total prints without the share of tokens it
accounts for.

The scan runs across one worker pool and is saved to disk, so the CLI and the
window share it and neither pays half a minute to rediscover what has not
changed. `--calendar` and the app's Stats tab draw the year as a shape rather
than a number.

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

  `axio.exe` has since been built and run on Windows, and started correctly —
  which leaves only whether that machine already had a toolchain. That run also
  found a bug nothing here could: against a `.axio/config.toml` belonging to
  another tool, axio reported every section as damage and wrote a backup of a
  healthy file. Two seconds of use, on the first platform nobody had used it on.
- ~~A dogfooding session, written up here.~~ **Done, with one half open.** Four
  sessions across two models: axio adding a feature to its own repository
  through its interactive surface, against a real model, with every approval
  displayed before it was answered. All four are written up in the changelog,
  including the code it got wrong and the bugs of its own they exposed — among
  them the discovery that it read no project instructions at all, which was the
  ceiling the whole time and not the model. A human still has not watched the
  approvals; every run was driven programmatically, and that half stays open.

### Not tagged yet, deliberately

Everything above is either done or bounded, and the tag is still being held.
Real use keeps producing fixes on the day it happens: four from the dogfooding
sessions, several in code written hours earlier, and a fifth from someone
starting the binary on Windows once. Nothing in a suite of 381 tests reached any
of them. That is not a reason to panic and it is not a reason to hurry — a
release is a promise that what is in it has settled, and the honest thing to say
about code this recent is that it has not.

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
