# Architecture

Four crates and one binary, in a tree rooted at a dependency-free core.

```
                    ┌──────────────── axio (bin) ────────────────┐
                    │ clap · surface selection · renderers        │
                    │ inline TUI · signal handling · exit codes    │
                    └──────┬──────────────────────────┬───────────┘
                           │ constructs               │ constructs
                           ▼                          ▼
        ┌──────────────────────────────┐   ┌────────────────────────┐
        │ Renderer / Tui               │   │ Arc<dyn Approver>      │
        │  PlainRenderer · JsonlRenderer│   │  NonInteractive · Tui  │
        └──────────▲───────────────────┘   └──────────▲─────────────┘
                   │ UnboundedReceiver<Event>         │ &ApprovalRequest → Decision
   ════════════════ THE BOUNDARY ══════════════════════════════════════
                   │                                  │
        ┌──────────┴──────────────────────────────────┴─────────────┐
        │ axio-core                                                  │
        │   protocol · Agent::run_turn · Session · Workspace          │
        │   Provider / Tool / Approver traits · Redacted              │
        │   links: serde, tokio, thiserror, ulid, tracing             │
        │   links NO http/tls, NO terminal, NO fs walker, NO process  │
        └──────────▲───────────────────────────▲────────────────────┘
                   │ impl Provider             │ impl Tool
        ┌──────────┴───────────┐   ┌───────────┴──────────────────┐
        │ axio-provider        │   │ axio-tools                   │
        │  Messages transport  │   │  read write edit             │
        │  SSE decoder         │   │  glob grep bash · subprocess  │
        └──────────────────────┘   └───────────────────────────────┘
```

## The three invariants

Everything else follows from these. Changing one needs a good argument.

### 1. `Tool::run` is the only execution path

`plan()` is pure and returns three things: the human-facing preview, the string
the permission engine matches on, and an opaque payload. `run()` takes the plan
back and applies exactly that payload. The loop never matches on a tool name.

Two properties fall out for free. Approval is a **pre-flight** rather than an
interception, so what was previewed is what runs — the stale-preview race cannot
happen. And adding a tool never edits a type in the core, because the payload is
`Box<dyn Any + Send>` handed from a tool to itself.

### 2. `ToolCx` is closed

Five concrete fields: workspace, cancellation token, progress sink, limits,
environment. None optional, none `dyn`. A tool that needs something not on this
struct does not ship until the struct is deliberately widened.

This is the gate against a context that accumulates optional host bridges until
tools quietly become surface-specific. `scripts/limits.sh` counts the fields and
fails the build on a sixth, or on an `Option<Arc<dyn ...>>` appearing.

### 3. Surfaces consume events and supply an `Approver`

Nothing else crosses the boundary. Every protocol type is serde-derived even
though nothing goes on a wire yet, which is what makes `--json` a second
renderer rather than a second loop, and what would make an out-of-process client
additive rather than a redesign.

The decision returns through the `Approver` trait rather than through the event
stream. That is not a style choice: a pending-approval map consulted by the same
`&mut self` that is awaiting inside the turn **deadlocks**, because the task that
would read the answer cannot run.

## The turn loop

One user turn in, exactly one `TurnEnded` out — on every exit path, including
cancellation, refusal, budget, step limit and failure. A surface that counts
`TurnStarted` and `TurnEnded` can rely on them balancing.

```
run_turn
  └── for each step, up to max_steps
        ├── build_request      transcript → wire shape, cache breakpoints placed
        ├── sample             buffer one COMPLETE assistant message
        │     ├── retryable error → ItemDiscarded, back off, retry
        │     ├── context overflow → compact and retry once, else fail
        │     └── cancelled → Interrupted
        ├── budget check
        ├── refusal → Refused (an outcome, never an error)
        ├── append the full assistant content, reasoning verbatim
        ├── no tool calls → Completed
        └── dispatch, then check cancellation
```

The whole assistant message is **buffered before any tool runs**. Executing a
tool mid-stream can idle the connection past its read timeout, and a half-drained
stream leaves content blocks unterminated. The latency cost is real and accepted;
deltas still reach the surface while the buffer fills.

## Two rules about wire shape

`Session::wire_messages` is the single place the durable transcript becomes a
request. Two things there are load-bearing and easy to get wrong:

**Reasoning blocks are echoed back verbatim, signature included.** The signature
is opaque and is part of the durable record even though no surface displays it —
echoing the text without it is rejected. Blocks are dropped entirely when the
request model differs from the one that minted them.

**A run of tool calls becomes exactly two messages**: one assistant message
holding every `tool_use`, then one user message holding every `tool_result` in
the same order. Emitting them interleaved is accepted by the API and teaches the
model to stop calling tools in parallel. Every `tool_use` gets a result — denied,
cancelled, unknown-tool alike — because one without a match makes the next
request invalid.

## Cancellation

One token per turn. The loop takes a child token per provider request and per
tool, so cancelling the turn cancels everything under it while a single failed
request never poisons the turn's own token.

The first interrupt cancels; a second within two seconds stops waiting and exits.
An `Interrupted` item is appended to the transcript so the model learns on the
next request that its work was cut short, rather than re-planning from a state
that never happened.

Exit codes follow the shell convention: `130` for an interrupt, `143` for
termination, `129` for a hangup. A signal's code outranks the turn's outcome —
the caller asked the process to stop, and the shell reports how.

`SIGTERM` and `SIGHUP` do not get a second chance the way a first `Ctrl-C` does,
but they still cancel and hand control back to the runtime for a bounded moment
rather than calling `process::exit` outright. Exiting there runs no destructor
and polls no task, so the tree kill never happens and whatever the agent started
outlives it, reparented to init.

A turn that completes with at least one action refused exits `5`. The answer is
prose, and prose is not something a script can check — a run where every
mutation was denied otherwise looks exactly like a run that did the work.

## Surfaces

Two, and neither is privileged. Both consume the same `Event` stream and supply
an `Approver`; nothing else crosses the boundary, which is why `--json` is a
renderer rather than a second loop.

The interactive one uses an **inline** viewport rather than the alternate
screen. A full-screen application owns scrollback, selection and the scrollbar
and hands none of them back — so the finished transcript is printed into the
terminal's own history and only the live part is redrawn. A diff being approved
goes to scrollback for the same reason: it is the evidence the answer rests on,
and it should still be there afterwards.

The decision travels back through `Approver`, not through the event stream, and
the turn awaits it inline on `&mut self`. So the loop and the interface cannot
be the same task: the interactive approver hands the request across a channel
and waits on a reply. A closed channel is a denial, because "nobody is there"
must not mean "run it".

## Permission and execution

Every decision is made against a plan's **subject** — a canonical string like
`read:src/lib.rs` or `bash:git` — never against a tool's name or its raw
arguments. That indirection is what makes `deny read:*.env` expressible, and
what stops a shell command being classified by a string its own arguments can
shape.

Evaluation is ordered, and the order is the design:

1. **Built-in deny.** Credentials, keys, `.ssh`, git hooks. Not overridable by a
   user rule and not by `--yes`, because it protects things no preference should
   be able to expose by accident. A subject alone is not enough here: `bash:cat`
   names a program, and the list matches paths, so a plan also declares the
   paths its arguments name and those go through the same test. What a shell
   computes rather than states is still invisible — see SECURITY.md.
2. **User deny rules.**
3. **Read-only effects** — auto-approved. This only works *after* the denies
   have had their say; put it first and no rule can protect a secret file.
4. **User allow rules.**
5. **Session grants** from an earlier approval. Memory only: an approval is a
   decision about a moment, not a configuration change.
6. **Ask**, or the unattended answer.

The rule language is `*` and `?` and nothing else. A richer one is a rule
language nobody can predict, and the point of a deny rule is that its author is
certain what it covers.

Dispatch runs in two phases. Everything is planned and authorised first, in call
order, before anything executes — so a batch containing one refused call does
not half-run. Then read-only calls execute concurrently while everything else
runs serially in call order, because `write(a.rs)` and `edit(a.rs)` in one batch
must not race.

Output is capped and spilled at one choke point in the loop rather than in each
tool. A tool that forgot would put ten megabytes in the next request and nothing
would error; doing it once means a new tool inherits the behaviour without
knowing it exists. The spill file is named for the call that produced it, and
`read` can reach the spill directory even though it sits outside the workspace —
the truncation marker names that path and tells the model to read it, so both
have to be true or the marker is an instruction nothing can follow.

`files_changed` on `TurnEnded` is derived from the calls that succeeded and
previewed a diff, which is `write` and `edit`. A shell command that rewrites a
tree is not in it and cannot be: naming what is knowable leaves a visible gap,
and guessing at the rest would produce an invisible wrong answer.

## Sessions on disk

**The file is the record of what happened, not of what was sent.** That one rule
decides everything else about persistence.

A session is append-only JSONL. Line one is always the header, written by the
same call that creates the file, so a header can never appear after an item and
a concurrent reader sees either a complete header or nothing. `--list` reads
exactly that line and never parses a transcript, which is what keeps listing
cheap as sessions accumulate.

Compaction never writes. It is a request-shaping decision re-derived per step
from the transcript in memory, so a resumed session rebuilds the full history
and reproduces the same elisions rather than drifting further on each resume.

Two things are excluded from the record even though the wire would replay them:
an empty assistant message, which the wire rejects anyway, and a context-elision
marker. The second matters more than it looks — the file still contains every
item such a marker claims was removed, so persisting one writes a lie, and
because the file is append-only every resume would append another.

Loading never fails on a damaged line. A torn final line is a crash, not
corruption, and erroring there would lose an entire session over its last few
bytes; it is skipped with a notice. A bad line in the *middle* is different: the
transcript has a hole the model would misread as history, so the session is
marked degraded. Any tool call without a result is repaired to cancelled,
because a `tool_use` with no matching `tool_result` is rejected outright and
would make the session unresumable.

## Compaction

Two staged elisions plus a force-only third, all pure functions of the
transcript:

| Stage | Fires at | Does |
| --- | --- | --- |
| 1 | 55% of the window | Drops `read` results whose path was later re-read or edited — the content is stale, a newer copy is already in context |
| 2 | 70% | Replaces tool outputs over 200 bytes with a marker |
| 3 | overflow only | Drops a prefix, leaving a marker so the hole is visible |

Purity is the point: the same items always produce the same plan, which is what
makes a resumed request identical to the original rather than approximately so.

Two clamps protect what cannot be detected as missing from the request itself.
Index 0 always survives, because the opening prompt is the task. And a prefix
drop never advances past the most recent user message — losing it leaves a
perfectly valid request about the wrong question, at full price.

There is deliberately no clamp for tool pairings. One `ToolCall` item emits both
the `tool_use` and its `tool_result`, so dropping whole items can never orphan
half a pair. That is why compaction removes items rather than editing the wire
projection.

## Configuration

Five layers, weakest first: built-in defaults, the user file, the nearest
project file walking up from the working directory, `AXIO_*` environment
variables, then command-line flags. The merge is leaf-wise, so a layer that
mentions one key does not clobber the rest of its section.

The winning layer is retained per key, which is what `axio --explain <key>`
reports. A key nobody set still explains itself as a built-in default.

**A project config may only make axio ask more.** `[permissions] allow` is
ignored from a project file with a notice: a cloned repository that can grant
itself shell access is remote code execution by `cd`, on a tool that ships no
sandbox and has `--yes`.

**A broken section resets that section and nothing else.** The whole file is
parsed first; each table is then validated independently, and one that fails is
dropped with a notice while the rest survive. A backup is written only when
something was actually lost — a healthy config never litters.

## Providers

Two implementations, selected by name in configuration. Deliberately not a
registry: a second provider is a second implementation, not an extension point,
until something needs it to be.

| `[model] provider` | Speaks | Credential |
| --- | --- | --- |
| `anthropic` (default) | the Messages API | `ANTHROPIC_API_KEY` |
| `ollama` | the OpenAI chat-completions dialect | `OLLAMA_API_KEY` |

The second one exists to answer a question the design could not answer by
inspection: is `Provider` actually provider-shaped, or is it one API wearing a
trait? Mostly the former. One real seam turned up, and it is instructive.

`WireMessage` is Messages-API-shaped: one user message can carry N
`tool_result`s. In the chat-completions dialect each result is its own message
with `role: "tool"`, so the projection has to be split. That conversion lives in
the provider, which is exactly where a dialect difference belongs — nothing
above the trait changed. Two features have no equivalent at all and are dropped
rather than mistranslated: effort, and reasoning blocks.

What the second provider cannot do is price itself. `ModelInfo` reports zeros,
so cost is reported as zero rather than invented — a made-up price would make
the budget check silently wrong rather than visibly absent.

## Credentials

`axio auth login` reads a credential from stdin — never from an argument, which
would put it in shell history and in `ps` output for every user on the machine —
and stores it at `$AXIO_HOME/auth.json`.

An environment variable always outranks the stored file. That makes a stored
credential easy to override for one command, and it means CI never silently
picks up a developer's saved key. `axio auth status` and `--doctor` both report
which source won, and neither ever prints the credential itself.

On unix the file is created `0600` — at creation, not chmodded afterwards,
which would leave a window where it is world-readable. **On Windows no
protection is claimed at all**, and `login` says so. The predecessor project
shipped a `restrict_to_owner` that was a documented no-op there; a false claim
about a credential is worse than an honest absence of one.

The directory holding it is added to the permission engine's built-in deny
list, for read *and* write, non-overridably. Without that, running axio from a
parent of its own home puts `auth.json` inside the workspace, where the `read`
tool would hand the key to the model — and no allow rule or `--yes` should be
able to reach it.

## What is not built yet

The interactive surface prints a pointer to the one-shot form. See
`docs/roadmap.md`.
