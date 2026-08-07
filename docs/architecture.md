# Architecture

Nine crates and three binaries, in a tree rooted at a dependency-free core.

```
                    ┌──────────────────── axio (bin) ─────────────────────┐
                    │ clap · surface selection · renderers                │
                    │ inline TUI · sandbox · signal handling · exit codes │
                    │ axio quota · axio cost                              │
                    └──────┬──────────────────────────┬───────────────────┘
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

   ─── the surfaces, and the seam that keeps them one product ───

        ┌──────────────────────┐   ┌───────────────────────────────┐
        │ axio (bin)           │   │ axio-app (bin)  [app]         │
        │  CLI · inline TUI    │   │  window · webview · terminals │
        └──────────┬───────────┘   └───────┬───────────────┬───────┘
                   │ its own wiring        │ its own wiring│ hosts
                   ▼                       ▼               ▼
        ┌──────────────────────────────────────┐  ┌────────────────────┐
        │ axio-supervisor                      │  │ axio-pty           │
        │  a worktree and branch per session   │  │  executable        │
        │  one pooled approval queue           │  │    allowlist       │
        │  sidecar index · AgentFactory seam   │  │  bounded byte ring │
        │  builds no agents, so links neither  │  │  tree-kill         │
        │  a transport nor a tool              │  │  parses nothing    │
        └──────────────────────────────────────┘  └────────────────────┘
             both surfaces drive the supervisor with their own wiring,
             so neither can hold a capability the other lacks

   ─── off the tree entirely: two leaves that depend on nothing here ───

        ┌──────────────────────┐   ┌───────────────────────────────┐
        │ axio-quota           │   │ axio-cost                     │
        │  provider probes     │   │  one parser per agent         │
        │  history · schedule  │   │  normalize · dedupe · price   │
        │  [app] tray+window   │   │  [sqlite] database stores     │
        └──────────────────────┘   └───────────────────────────────┘
             axio-quota --features app and axio-app --features app
             are the second and third binaries
```

## The two leaf crates

`axio-quota` and `axio-cost` hang off `axio` and off nothing else. Neither
depends on another crate here, so neither thickens a boundary — `axio-core` is
as isolated as it was, and the tree still has one root.

They are separate crates for the same reason, and it is isolation rather than
size. **Both read files that belong to other programs**, which is a different
trust boundary from axio's own credential store and belongs behind a different
name. `axio-quota`'s `app` feature carries a Tauri desktop surface pulling 189
packages and a webview runtime, none of which may reach a default `cargo install
axio`; `axio-cost`'s `sqlite` feature compiles SQLite's own C source, which a
clean install is promised not to need. Both are off by default and verified
absent from the default tree.

They are also different problems wearing a similar hat. Quota asks each vendor's
API what is left, and fails with an HTTP status; cost reads transcripts other
programs wrote and adds them up, and fails with a line it cannot understand. One
crate each keeps both budgets in `scripts/limits.sh` meaningful.

**A long file is still not evidence for a tenth crate.** A file past 300 lines
becomes child modules — never another crate. Crates are how the dependency graph
is kept honest, and a module that has grown says nothing about dependencies.

## Many sessions at once

`axio-supervisor` runs a lot of conversations rather than one: each on its own
task, each in its own git worktree and branch, all reporting into one event
stream and one pooled queue of approvals. It exists because `Agent::run_turn`
takes `&mut self` — the right shape for one conversation and the wrong one for
five, since a shared agent behind a lock would serialise every session behind
whichever is currently talking to a model.

It is the only crate besides `axio` that is not a leaf, and it is defined as
much by the dependency it refuses as by the ones it takes. Running many sessions
means building many agents, and building one means choosing a provider,
registering tools and resolving a policy — so the obvious shape would depend on
`axio-provider` and `axio-tools`, which is most of the workspace. Instead agents
arrive through an injected `AgentFactory`. It therefore links neither, its tests
run against `ScriptedProvider` in milliseconds, and — the part that matters for
the product rather than the build — the CLI and the desktop app drive the same
supervisor with their own wiring, so **neither can hold a capability the other
lacks**.

Nothing in it is reachable from a tool. Worktrees, branches, the session index
and the approval queue are host state, which is what keeps `ToolCx` closed at
five fields and every tool working identically in a one-shot run. It spawns
`git`, which is not a hole in "`axio-tools` is the only crate that spawns a
process": that rule is about processes run *on the model's behalf*. libgit2 is
deliberately not linked, because it compiles C and `cargo install axio` is
promised not to need a C toolchain.

Isolation is the default and never a fallback. If a worktree cannot be cut,
starting fails — falling back to the live checkout would hand an agent write
access to the files someone is using, silently, at the moment isolation was most
clearly wanted. `Isolation::Direct` exists and is chosen.

Landing work is deliberately absent. Merging, pull requests and cherry-picking
are workflows, and picking one would be wrong for the other two; the branch
name, `status()` and `diff()` are what a caller needs to land it its own way.

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
        ├── emit Usage         the turn's running total, per step
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

Three now — one-shot, the inline interactive one, and the window — and none is
privileged. Each consumes the same `Event` stream and supplies an `Approver`;
nothing else crosses the boundary, which is why `--json` is a renderer rather
than a second loop, and why the window is a surface rather than a second
product. The rest of this section is about the interactive one; the window is in
`docs/roadmap.md` under M13 and its own hazards are in `docs/gotchas.md`.

The interactive one uses an **inline** viewport rather than the alternate
screen. A full-screen application owns scrollback, selection and the scrollbar
and hands none of them back — so the finished transcript is printed into the
terminal's own history and only the live part is redrawn. A diff being approved
goes to scrollback for the same reason: it is the evidence the answer rests on,
and it should still be there afterwards.

An answer reaches that history **a line at a time, rendered**. The model writes
markdown whether or not anything reads it, so the surface renders the subset it
actually emits — headings, lists, quotes, rules, fenced and inline code, links
and emphasis — and leaves anything else as the characters that were written. A
line is the unit because it is the unit markdown is written in: it can be
rendered as soon as its newline arrives, without seeing what follows, and the
only state that crosses a line boundary is an open code fence — or a table,
which is the one construct a line cannot settle, because its columns are as wide
as their widest cell and that cell may not have arrived. A table is therefore
held back until the row that ends it, which means a message can be entirely
consumed with nothing on screen yet: the surface tracks that separately from
whether anything was printed, or it would draw the whole message again on top of
the table it was still holding. What is left over
— the sentence still being typed — stays in the viewport, unrendered and dim,
until its own newline commits it.

The cost of committing early is that a dropped stream cannot be unprinted. The
retry re-sends what was lost, so the surface says so rather than letting the
text quietly appear twice, which is the same bargain the one-shot renderer
makes when it streams to stdout.

Drawing is **paced, not driven**. A change marks the viewport dirty and a frame
is painted at most every 16ms, because a model streaming at speed produces
changes far faster than a terminal can usefully show them and every wasted frame
is bandwidth the text itself is not getting. Committed lines do not wait for a
frame — they are written straight through, so pacing costs the answer nothing.

Colour comes from the terminal's own sixteen, never from a bundled theme. A
theme carries absolute colours chosen against a particular background, and the
background belongs to the user — there is no way to ask which one they have, and
a dark theme on a light terminal is unreadable. So the syntax highlighter takes
syntect's parser and none of its themes, mapping scopes onto the palette the
user already configured. Everything else follows the same rule, which is why the
transcript looks like the shell it was run from rather than like an application.

Everything below the run loop is generic over the backend rather than tied to
the terminal, which is what lets a test hold a fake one and assert the property
the whole design rests on: that finished lines are in the terminal's history and
only the unfinished ones are in the viewport. The line editor is further split
out with no terminal in it at all, so where a word ends and what recalls history
are ordinary functions with ordinary tests.

A line beginning `/` is the surface's own, not the model's. The commands sit in
one `const` list that the menu, `/help` and the dispatch all read, so an entry
nothing runs cannot exist — the failure that list prevents is a command offered
in a menu and unimplemented behind it, which reads as a broken feature rather
than an absent one. None of them reach the model, spend a token or append to
the transcript, and that is what makes them safe to run mid-turn: a command is
not a turn, so it needs no agent and races nothing. `/model` is the exception
that proves it, being the only one that touches the agent at all — and because
the agent is moved into a running turn and handed back at its end, `/model` is
refused while one is in flight rather than arranging to mutate it underneath.

Bare `/model` runs in two stages — provider, then model — and they are one
flow rather than two commands for a reason: a provider changed on its own
leaves the session naming a model its new endpoint has never heard of. Two
commands make that state reachable; one flow does not. `Agent::adopt` takes
both together for the same reason, and the transcript survives the move because
what was said does not depend on who is asked next.

The surface has no configuration, so it cannot build a provider. It is given a
factory instead — a closure over the resolved config — which keeps credential
lookup and transport construction where they already live.

The second stage's list is **asked of the provider** — every provider, with
no exception — rather
than compiled in — the second method on the seam exists for this. A catalogue
in the binary is wrong the first time either side ships a model, and wrong
where nobody looks: a name missing from a picker is indistinguishable from a
name the provider refuses. Both dialects publish the same `{"data":[{"id"}]}`
shape, so one parser reads either.

The fetch is a round trip, so it does not happen on the loop. The command
returns an intention, the loop spawns the request with a clone of the
provider's `Arc`, and the answer arrives on a channel the loop already selects
over — the same shape the approver uses, for the same reason. Choosing from
the picker then emits exactly the action a typed `/model NAME` emits, so one
path applies a model, warns about the same reasoning loss and is refused in the
same circumstances. A picker with its own application path is how the typed
form's warning quietly stops being given.

An unrecognised single word beginning `/` is refused rather than sent. A
mistyped command that becomes a prompt spends a turn answering a typo, and the
answer looks like the model being unhelpful. A *multi*-word line is left alone,
so prose opening with a path stays prose; the cost is that a misspelling with an
argument gets through, which is the cheaper of the two mistakes.

The menu draws in the rows the streaming tail uses. An inline viewport's height
is fixed when it is created and ratatui offers no way to change it after, so
there is nowhere to float a popup — but nothing streams while a command name is
being typed, so the space is free exactly when it is wanted.

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

Arguments are checked at the same choke point and for the same reason: every
schema declares what it takes and what it requires, and both halves are enforced
before a tool is asked to plan. A tool that validated its own would be a tool
that could forget to. What the model gets back names the tool and everything it
accepts, whether it invented an argument or omitted one — a rejection that does
not say what would have worked is a turn spent guessing.

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

Everything axio owns lives in `~/.axio`, the same path on every platform
rather than each one's own configuration directory. One path to document, to
sync between machines and to name in a bug report; `AXIO_HOME` moves it. The
departure from XDG is deliberate — a tool that follows someone across four
platforms is worth more consistent than conventional.

**The user's file is never also a project file.** Both are called
`.axio/config.toml`, so the upward walk finds the user's own from anywhere
beneath the home directory and would load it twice — the second time as a
project, a layer that may only restrict. Settings returning as a restricted
copy of themselves, silently, and only on machines where someone works under
`$HOME`. The walk is given the user's path and refuses to return it, matched on
the path rather than the boundary because `AXIO_HOME` can put it anywhere.

The winning layer is retained per key, which is what `axio --explain <key>`
reports. A key nobody set still explains itself as a built-in default.

**A project config may only make axio ask more.** `[permissions] allow` is
ignored from a project file with a notice: a cloned repository that can grant
itself shell access is remote code execution by `cd`, on a tool whose sandbox is
off by default and which has `--yes`.

**A default that worked is written back.** Choosing a provider and model in
the interface saves them — but only after a turn returns an outcome that could
not have happened unless the endpoint accepted the request, because a model
name is not checked until then and saving at the moment of choosing would make
a typo the default. The write is a line edit of the two keys, not a serialise
of the whole config: comments, ordering and sections axio does not use are
someone's work and survive it.

**A broken section resets that section and nothing else.** The whole file is
parsed first; each table is then validated independently, and one that fails is
dropped with a notice while the rest survive. A backup is written only when
something was actually lost — a healthy config never litters.

A section axio does not recognise is not a broken one. It belongs to another
tool, or a newer axio, or it is a typo; none of those is this file being
damaged, so it is skipped, mentioned once, and nothing is written. The
distinction is load-bearing: without it, running axio in a directory some other
program also calls `.axio` reports every section as wreckage and then copies the
file aside to prove it.

## Providers

Four names over three implementations, selected by name in configuration — the
list is `PROVIDERS` in `crates/axio-core/src/auth.rs`. Deliberately not a
registry: a second provider is a second implementation, not an extension point,
until something needs it to be. The third is where that stopped holding, and the
reason is below.

| `[model] provider` | Speaks | Credential |
| --- | --- | --- |
| `anthropic` (default) | the Messages API | `ANTHROPIC_API_KEY` |
| `ollama` | the OpenAI chat-completions dialect | `OLLAMA_API_KEY` |
| `openai-compatible` | the same dialect and the same implementation; set `base_url` to reach an endpoint of your own | `OLLAMA_API_KEY` |
| `openai-codex` | the Responses dialect, at the subscription endpoint | a browser sign-in, renewed before it expires |

The second one exists to answer a question the design could not answer by
inspection: is `Provider` actually provider-shaped, or is it one API wearing a
trait? Mostly the former. One real seam turned up, and it is instructive.

`WireMessage` is Messages-API-shaped: one user message can carry N
`tool_result`s. In the chat-completions dialect each result is its own message
with `role: "tool"`, so the projection has to be split. That conversion lives in
the provider, which is exactly where a dialect difference belongs — nothing
above the trait changed. Reasoning blocks have no equivalent at all and are
dropped rather than mistranslated. Effort was assumed to be in the same
category and was not: the dialect calls it `reasoning_effort`, and the endpoint
proved the point by rejecting an invalid value while accepting a field it had
never heard of. Five efforts map onto four accepted values, so the top two
collapse upward.

What the second provider cannot do is price itself. `ModelInfo` reports zeros,
so cost is reported as zero rather than invented — a made-up price would make
the budget check silently wrong rather than visibly absent.

A **third** provider is where "two implementations, not an extension point"
stopped holding. A subscription token is accepted by exactly one endpoint, and
that endpoint speaks the Responses dialect — so the alternative to a third arm
was not having it. The seam took it without changing: the differences are all
inside the transport. `instructions` is a field rather than a message; a tool
is flat where chat-completions nests it under `function`; a finished tool call
arrives complete in one event and is expanded here into the start, delta and
end the loop expects, rather than the loop learning a second shape.

It is also the first provider whose credential **expires**. It renews before a
request rather than failing one, re-checks under the write lock so two requests
cannot spend two refreshes — some issuers answer the second by invalidating the
first — and hands the renewed pair to a `TokenSink`. The sink is a trait
because writing credentials is not the transport's job: a refresh nobody
persists is one that every later process repeats.

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

Both surfaces are built. A binary compiled without the `tui` feature has no
interactive surface and prints a pointer to the one-shot form instead. What is
deferred, and what would change each decision, is in `docs/roadmap.md`.
