# Architecture

Four crates and one binary, in a tree rooted at a dependency-free core.

```
                    ┌──────────────── axio (bin) ────────────────┐
                    │ clap · surface selection · renderers        │
                    │ signal handling · exit codes                │
                    └──────┬──────────────────────────┬───────────┘
                           │ constructs               │ constructs
                           ▼                          ▼
        ┌──────────────────────────────┐   ┌────────────────────────┐
        │ Renderer                     │   │ Arc<dyn Approver>      │
        │  PlainRenderer · JsonlRenderer│   │  NonInteractive        │
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
        │  Messages transport  │   │  the six tools (not yet built)│
        │  SSE decoder         │   │  subprocess · truncation      │
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

## What is not built yet

`axio-tools` is a stub and the interactive surface prints a pointer to the
one-shot form. Session persistence, layered configuration, compaction and the
permission engine have their shapes reserved in the types but no implementation.
See `docs/roadmap.md`.
