# AGENTS.md

## Commands

```sh
cargo test --workspace                              # everything
cargo test -p axio-core                             # the loop, against fakes; must stay under 2s
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
bash scripts/features.sh                            # both feature sets, with CI's -D warnings

bash scripts/firewall.sh                            # naming firewall (also a pre-commit hook)
bash scripts/limits.sh                              # members, ToolCx fields, file size; LOC per crate
bash scripts/deps.sh                                # axio-core links no transport/terminal/walker
bash scripts/check-windows.sh                       # cfg-gated code still compiles for windows

axio quota                                          # provider limits; --json, --diagnose
axio cost --by model|client|day|workspace|session|week|month|hour
axio cost --calendar | --wide | --cached            # shape; derived columns; skip the rescan
curl -fsSL https://models.dev/api.json -o p.json && axio cost --import-prices p.json
cargo build --release -p axio-app --features app                # the desktop surface
npm --prefix crates/axio-app/ui run build                       # frontend, before that
npm --prefix crates/axio-app/ui run typecheck                   # the only step that checks TS types
cargo test -p axio-app && git diff --exit-code crates/axio-app/ui/src/generated  # boundary drift
axio session start|list|diff|close                              # supervised worktree sessions
cargo build --release -p axio-quota --features app              # tray + flyout + window
npm --prefix crates/axio-quota/ui run build                     # frontend, before that
node crates/axio-quota/icons/make-icon.mjs                      # regenerate icon.ico
cargo test -p axio-cost --features sqlite           # the database-backed agents
AXIO_COST_THREADS=1 axio cost                       # pin the scan to one thread
AXIO_CONNECT_PROBE=1                                # open the sign-in window, print cookie names

INSTA_UPDATE=always cargo test -p axio-provider     # re-accept request-body snapshots
bash scripts/live-check.sh                          # a real turn against a real model
git config core.hooksPath .githooks                 # one-time, per clone
```

## Architecture

Nine crates and three binaries. The dependency graph is a tree rooted at
`axio-core`, so there are no cycles and no cycle-breaker crates.

| Crate | Owns |
| --- | --- |
| `axio-core` | `protocol`, the `Provider` / `Tool` / `Approver` traits, the turn loop, `Session` and its JSONL records, layered `config`, `compact`, `policy`, output `truncate`, credential storage, `Workspace`, `Redacted` |
| `axio-provider` | All three transports — the Messages dialect, chat-completions, and Responses — plus the OAuth flows: SSE decoder, request builders, block state machines, error classification. The only crate linking HTTP or TLS |
| `axio-tools` | The six tools, subprocess helpers, diff previews, byte-stable schemas. The only crate that walks a filesystem or spawns a process |
| `axio-cost` | What the coding agents on this machine have spent, read from the session transcripts they already write. Token normalization, deduplication, a bundled price table, and one parser per agent — three hand-written, the rest table-driven. Behind the `sqlite` feature it also reads the agents whose store is a database. Depends on nothing else in the workspace |
| `axio-quota` | Provider quota probes — the credential files `codex` and `claude` wrote, and the usage endpoints behind them — plus local history, and behind the `app` feature the Tauri desktop surface: tray icon, HTML flyout, window. Depends on nothing else in the workspace |
| `axio-pty` | Running another agent's command-line tool - Claude Code, Codex, Pi - in a pseudo-terminal axio owns. An executable allowlist, a bounded byte ring read by cursor, and tree-killing. Depends on nothing else in the workspace |
| `axio-app` | The desktop surface. Rust owns all state and TypeScript owns pixels; every command is `async` so none of them runs on the thread that paints. Behind an `app` feature, so a default build links no webview |
| `axio-supervisor` | Many sessions at once, across many repositories: one task per session, a git worktree and branch each, one pooled approval queue, and a sidecar index of what exists. Builds no agents — they arrive through `AgentFactory` — so it links no transport and no tools |
| `axio` | clap, surface selection, renderers, the inline TUI and its slash commands, the optional Landlock sandbox, `Approver` implementations, `axio quota`, `axio cost` |

### Workspace member justification

`axio-pty` is the ninth member, and it is a leaf: it depends on no other crate
here and only `axio-app` depends on it. Isolation is the reason, as it was for
the fifth and sixth. It links `portable-pty`, which a one-shot CLI run has no
use for, and it is the one crate whose security property is a *list* - the
executable allowlist - which is much easier to audit when it is the only thing
in the crate.

It is also a different problem from `axio-tools` despite both spawning
processes. Tools run a command on the model's behalf, capture its output and
feed it back; this hands a program a real terminal and a person, and never reads
what it says.

`axio-app` is the eighth member, and the reason is the same dependency argument
the fifth and sixth made: Tauri carries a webview runtime and a build script,
and none of it may reach `cargo install axio`. It is off by default and
verified absent — `cargo tree -p axio` names neither `axio-app` nor `tauri`.

Its state and boundary types are *outside* the feature, deliberately. That is
what lets the whole surface behind the glass be exercised by ordinary unit
tests with no webview, and it is the enforcement mechanism for "Rust owns all
state": logic that has to compile without Tauri cannot quietly become a
TypeScript store.

`axio-supervisor` is the seventh member, and the reason is the dependency it
*refuses* rather than the ones it takes. Running many sessions at once means
building many agents, and building one means choosing a provider, registering
tools and resolving a policy — so the obvious shape would have it depend on
`axio-provider` and `axio-tools`, which is most of the workspace. Instead
agents arrive through an injected `AgentFactory`, so it links neither, and its
tests run against `ScriptedProvider` in milliseconds like `axio-core`'s.

The same seam is what keeps the CLI and the desktop app one product: both drive
this crate with their own wiring, so neither can hold a capability the other
lacks. It is also the only crate besides `axio` that is not a leaf — the graph
is still a tree rooted at `axio-core`, just wider at the top.

It spawns `git`, which is not a hole in "`axio-tools` is the only crate that
spawns a process": that rule is about processes run **on the model's behalf**,
and nothing here is reachable from a tool. libgit2 is deliberately not used —
it compiles C, and `cargo install axio` is promised not to need a C toolchain.

`axio-cost` is the sixth member, and like the fifth the reason is isolation
rather than size. It is one parser per agent over a shared normalization layer,
and the agent list only grows — a shape that would swamp `axio-quota`'s budget
in `scripts/limits.sh` and bury the probe layer it shares a hat with.

The two are different problems wearing that similar hat. Quota asks each
vendor's API what is left; cost reads transcripts other programs wrote and adds
them up. Different inputs, different failure modes — a quota probe fails with an
HTTP status, a cost parser fails with a line it cannot understand — and one
crate each keeps both budgets in `scripts/limits.sh` meaningful.

It is a leaf, like `axio-quota`: it depends on no other crate here, and only
`axio` depends on it.

`axio-quota` is the fifth member, and the reason is dependency isolation rather
than size. Its `app` feature carries the desktop surface — tray icon, HTML
flyout and window in one Tauri process — which pulls **189 packages**, a webview
runtime and a build script, by far the largest dependency event in this repo.
None of it may reach a default `cargo install axio`. Feature-gating it inside
the `axio` crate would have put platform-specific code and a `tauri.conf.json`
in the one crate `scripts/check-windows.sh` already documents that it cannot
check.

It is off by default and verified absent from the default tree: `cargo tree -p axio
-e normal` names no tauri, wry or webview2.

The frontend is React and TypeScript under `crates/axio-quota/ui`, matching
`apps/site`, built by Vite rather than Next — a desktop app has no server to
render on. `build.rs` calls `tauri_build::build()` behind `#[cfg(feature =
"app")]`, so a headless `cargo build -p axio-quota` neither reads
`tauri.conf.json` nor links `tauri-build`.

It is a leaf: it depends on no other workspace crate, and nothing but `axio`
depends on it. So it thickens no boundary — `axio-core` is as isolated as it was,
and the tree still has one root. It also reads *other tools'* credentials rather
than axio's, which is a different trust boundary and belongs behind a different
crate name.

**A long file is still not evidence for a sixth crate.** A file past 300
lines becomes child modules — `tui/`, `agent/`, `config/`, `markdown/` — never
another crate. Crates are how the dependency graph is kept honest (`axio-core`
links no transport, no terminal, no filesystem walker), and a module that has
grown says nothing about dependencies. `scripts/limits.sh` enforces both counts
and reports the widest file on every run. Lines are reported per crate and never
enforced: the only way to satisfy such a ceiling is to build less, and one
workspace figure cannot tell a growing surface apart from a thickening core. A
crate with no budget entry does fail — pricing one gives up nothing.

Splitting means the boundaries get named: a method reached from a sibling module
is `pub(super)` and everything else stays private, so each file states what it
offers rather than leaving every item equally reachable.

Three invariants everything else follows from:

1. **`Tool::run` is the only execution path.** `plan()` is pure and returns the
   preview, the policy subject and an opaque payload; `run()` applies exactly
   that payload. The loop never matches on a tool name, so approval is a
   pre-flight rather than an interception, and what was previewed is what runs.
2. **`ToolCx` is closed** — five concrete fields, none optional, none `dyn`. A
   tool needing more does not ship. `scripts/limits.sh` counts them.
3. **Surfaces consume a stream of `Event` and supply an `Approver`.** Nothing
   else crosses the boundary. Every protocol type is serde-derived from commit
   one, so `--json`, and later a second process, are additive.

## Gotchas

### axio-pty

- **Hosting is a *windowing* capability, not an agent capability, and that is
  why the CLI does not have it.** The invariant everywhere else is that no
  surface holds something the command line lacks — but the command line is
  already inside a terminal, so "run Claude Code in a pty" there is just running
  Claude Code. The application provides a terminal because it does not have one.
  Nothing about what an *agent* may do differs between the two.
- **A hosted agent must not inherit `CLAUDE_CODE_*` or `CLAUDE_PID`.** A tool
  that finds them concludes it was started by a copy of itself and behaves as a
  child session. axio may itself have been launched by one, so they are stripped.
- **`NO_COLOR` is stripped and `TERM`/`COLORTERM` are set.** axio strips colour
  from the tools *it* runs because a model reads them; a hosted agent is read by
  a person through a real terminal, and `TERM=dumb` makes its interface unusable.
- **On Windows the launch goes through `cmd.exe /d /s /c`.** Agent CLIs installed
  by npm are `.cmd` shims and `CreateProcess` will not execute one. `/d` skips
  AutoRun so a registry key set years ago does not run first.
- **Drop the slave end of the pair immediately after spawning.** While it is open
  the reader never sees EOF, so an agent that exits leaves a thread waiting on a
  terminal nobody is attached to.
- **Output is bytes, and decoding happens where the whole stream is.** A `read`
  lands wherever the kernel decided; decoding each chunk turns any multi-byte
  character straddling that boundary into a replacement character, permanently.
- **Reads are pulled by cursor, not pushed.** Something is always not listening -
  every webview reload - and a push-only stream loses whatever arrived then.
- **A submitted line is two writes: the text, then the carriage return.** A
  provider that treats one combined chunk as a paste leaves the text on its
  prompt unsent, which reads as the agent ignoring you.
- **Killing the direct child is not enough.** It is `cmd.exe` on Windows and may
  have started a build on unix. The tree goes, via the same `taskkill /T` and
  process-group signal `axio-tools` already uses - no Job Object, so no unsafe.
- **`MasterPty` is `Send` but not `Sync`.** It sits behind a mutex, or the whole
  application state stops being shareable for the sake of one resize call.
- **Closing a ConPTY blocks until its output pipe drains, and the child exiting
  does not break that.** The pump thread is blocked reading that pipe and the
  terminal outlives its process, so each waits for the other. The master is
  therefore taken out in `Drop` and released on a detached thread — otherwise
  closing a terminal hangs whatever thread the click arrived on.
- **ConPTY asks where the cursor is before letting the child's output through.**
  It writes `ESC [ 6 n` and stalls until something answers. A real terminal and
  xterm.js both answer; a test reading bytes into a buffer does not, and the
  whole stream sits behind the question. The tests reply `ESC [ 1 ; 1 R`.
- **ConPTY's opening handshake is about forty bytes.** A test that waits for
  "some output" is satisfied by terminal setup before the child has written
  anything. Wait for the text actually expected.

### axio-app

- **The TypeScript boundary is generated, and `cargo test -p axio-app` is what
  generates it.** ts-rs writes `ui/src/generated/` during the test run, so a
  Rust change with no regeneration shows up as a dirty tree — `git diff
  --exit-code` on that directory is the drift check. Never hand-edit those files.
- **`u64` becomes `bigint` unless told otherwise, and that would be wrong here.**
  Tauri's IPC is JSON, so what actually arrives is a JS number; `#[ts(type =
  "number")]` on those fields makes the declared type the one that shows up.
  Generation caught this on its first run, on a field the hand-written mirror
  had as `number` and the derive read as `bigint`.
- **A Tauri command without `async` runs on the thread that paints.** Not a
  performance note — a synchronous command doing a process probe, a teardown
  spin or a per-keystroke write freezes the window for its duration, and nothing
  in the code says so. Every command here is `async`; `State<'_, T>` works fine
  in one as long as `T: Send + Sync + 'static`.
- **Window controls are a command, not a capability.** Granting
  `core:window:allow-close` lets any script in the webview close the window.
  Routing it through `window_control` keeps a place that can refuse, which is
  what the close guard needs in order to exist.
- **`close` and `destroy` are different on purpose.** Closing runs the guard;
  destroying skips it. Only something that has already dealt with the running
  work may reach for the second.
- **A `beforeunload` listener does not fire for a taskbar close or Alt+F4.**
  Both guards are native: `WindowEvent::CloseRequested` for the title bar and
  `RunEvent::ExitRequested` for the rest. The second needs `Builder::build` then
  `app.run(closure)`; `.run(context)` cannot intercept it.
- **The lib target is named apart from the binary.** Both otherwise emit
  `axio_app.pdb` and cargo warns about the collision on every Windows build.
- **Nothing in `state` or `model` may link Tauri.** That is the rule that keeps
  the state layer testable and stops it drifting into the webview.

### axio-supervisor

- **A ULID orders by time only across milliseconds.** Its first ten characters
  are the timestamp and the rest is random, so two sessions started in the same
  millisecond sort arbitrarily — which a queue of agents does routinely. The
  session index therefore keeps **file order**, not id order, and a worktree
  branch is named with the *whole* ULID rather than a readable prefix.
- **A worktree path is recorded exactly as git was given it, never
  canonicalised.** git registers a worktree under the string it was handed, and
  on Windows `canonicalize` returns the `\\?\` extended-length form — a
  different string, so `worktree remove` fails to match its own registration and
  every closed session leaks a directory. `Workspace` canonicalises for
  confinement on its own, so nothing downstream needs it.
- **The approval queue cannot be dropped to shut it down.** Every session's
  approver holds an `Arc` to it, so it outlives the supervisor by construction,
  and session tasks are detached. `ApprovalQueue::shutdown` exists because
  without it a turn that asks a question after the last surface closed waits
  forever for an answer nobody can give.
- **Cancellation does not travel the session's command channel.** The task is
  inside `run_turn` and would not read the message until the turn it is meant to
  interrupt had already finished. It goes through the token the handle holds.
- **Isolation failing is an error, never a downgrade.** Falling back to the live
  checkout would hand an agent write access to the files you are using, silently,
  at the moment isolation was most clearly wanted. `Isolation::Direct` is chosen.
- **`[worktree] enabled = false` is refused from a project config**, like
  `[permissions] allow`. It does not look like a permission, which is exactly
  why it is worth stating: a repository that could set it would be deciding that
  a clone of it may edit your working tree.
- Landing work — merge, pull request, cherry-pick — is deliberately **not** here.
  It is a workflow, and picking one would be wrong for the other two. The branch
  name, `status()` and `diff()` are what a caller needs to land it its own way.

### axio-quota

- **A Tauri command declared `fn` rather than `async fn` runs on the main
  thread**, which is also the thread that paints. Anything that can be slow —
  a scan, a cookie read, opening a webview — hangs the window from there. And
  `Webview::cookies` posts to the event loop and blocks on the reply, so a
  synchronous command calling it deadlocks outright.
- **Tauri v2 denies any permission no `capabilities/` file grants, silently.**
  No error, no warning. Rust-side calls bypass the permission layer, so a
  denied frontend permission and a working button look like unrelated bugs.
- **A window showing a vendor's sign-in form stays decorated and stays out of
  `capabilities/default.json`.** Undecorated it is shaped like a phishing
  screen; listed, a remote page reaches axio's command surface.
- **A cookie's name is not proof anyone signed in.** One provider sets `auth`
  mid-handshake. Use the candidate against the provider's own endpoint and
  accept only what authenticates; only Unauthorized means keep waiting.
- **Refreshes pile up from four places** — settings, capture, schedule, button
  — and the resulting 429 gets reported as the vendor's fault. Throttle, and
  let an explicit Refresh bypass it.
- **Tauri picks dev-versus-production from a feature, not the cargo profile.**
  `tauri-build` emits `cfg(dev)` unless `tauri/custom-protocol` is on, so a
  `--release` build without it still loads `devUrl` and shows
  ERR_CONNECTION_REFUSED. The Tauri CLI adds the feature during `tauri build`;
  we build with plain cargo, so `app` names it. Do not remove it.
- **The frontend must be built before the Rust binary.** `frontendDist` points
  at `ui/dist`, and a stale or missing `dist` is embedded silently.
- **Label rate windows by duration, not by field position.** Codex's
  `primary_window` is the *weekly* window on some plans, with `secondary_window`
  null. See `codex::duration_label`.
- **Claude's `is_active` is not a filter.** It marks which limit is currently
  governing, not which exist: a live account returns `weekly_all` with
  `is_active: false` while the flat `seven_day` key reports real usage for that
  same window. Filtering on it hides every model-scoped window.
- Claude's `expiresAt` is **milliseconds**; Codex's `reset_at` is **seconds**.
- `CLAUDE_SECURESTORAGE_CONFIG_DIR` overrides only the credential store;
  `CLAUDE_CONFIG_DIR` overrides the whole profile. Check them in that order.
- **Send our own User-Agent.** Upstream CodexBar sends a Claude Code-shaped one
  to Anthropic's usage endpoint; tested 2026-08-02, the endpoint answers
  identically to `axio-quota/<version>`. `scripts/firewall.sh` fails the build if
  another client's identifier returns to `crates/`.
- Probes parse into `serde_json::Value` and pick fields with `json::pick`, never
  `#[derive(Deserialize)]` over a whole payload. Provider APIs send numbers as
  strings and add fields without notice; a strict decode turns that into an
  outage. A live payload proved it within an hour: `"balance": "0"`, a string.
- Quota's config is a **different file** from axio's own credential store, and
  both are called `auth.json`. `axio_core::auth` writes axio's; `axio-quota`
  reads the ones `codex` and `claude` wrote.
- Debug-level logs contain the raw provider response, which includes account id
  and email. Do not paste them into issues unredacted.

### axio

- **`temperature`, `top_p`, `top_k` and `budget_tokens` are 400s on this model.**
  There is no field for any of them in `ModelRequest`, deliberately — a knob that
  cannot be expressed cannot be reintroduced by a config layer. Depth is
  `output_config.effort` and nothing else.
- **Thinking is never disabled.** It is on by default on this model, and
  disabling it is a 400 above `high` effort. Even where legal it makes the model
  occasionally write a tool call into visible text: the turn succeeds, the call
  silently never runs, and nothing errors.
- **Request-body bytes are load-bearing.** Prompt caching is a prefix match, so
  non-deterministic JSON silently costs the whole cached prefix with no error.
  `serde_json` is pinned with `preserve_order`, and a snapshot test guards it.
- **The cache lookback window is 20 content blocks.** A tool-heavy turn that
  adds more than that between breakpoints stops finding the previous entry —
  again silently. `rolling_cache_plan` re-places at 15 to keep a margin.
- **A truncated stream must surface as `Truncated`, not a decode error.** An
  unterminated trailing SSE line is discarded at EOF along with its pending
  frame; emitting half a JSON document turns a retryable truncation into a
  fatal-looking bug.
- **A trailing `\r` is undecidable mid-stream** — the next chunk may open with
  `\n` — so the decoder holds it. At EOF it resolves as a bare-CR terminator.
  This is the case the byte-split property test exists for.
- **The session file records what happened, not what was sent.** Compaction
  never writes to it; it is re-derived per step from the transcript, which is
  what makes a resume reproducible instead of drifting.
- **A context-elision marker is never persisted.** The file still contains every
  item it names, so writing one is a lie — and append-only means every resume
  would add another.
- **`#[serde(default = "…")]` does not fire for an absent table.** Any struct
  with a non-false bool default needs a hand-written `impl Default`.
- **Usage reports are cumulative.** Summing `message_start` and `message_delta`
  double-counts the input tokens.
- **`cargo-deny` bans a second TLS stack and the default crypto provider.** If
  `openssl-sys`, `native-tls` or `aws-lc-*` appears, something pulled in a
  default feature set we turned off — and `aws-lc-sys` wants CMake everywhere
  and NASM on Windows, which a clean install is promised not to need.
- **A session file lives in a directory named for its day**, not directly under
  `sessions/` — `sessions/<yyyy-mm-dd>/<id>.jsonl`, derived from the id's own
  timestamp so a resume never scans. Anything counting or finding sessions goes
  through `SessionStore`; a `read_dir` of `sessions/` sees only day directories
  and reports nothing, convincingly.
- **`scripts/firewall.sh` and `scripts/limits.sh` cannot see an untracked
  file.** Both work through `git grep` and `git ls-files`, so a new module is
  invisible to them until it is at least `git add -N`'d — and they pass by not
  looking, which reads exactly like passing. Stage new files before believing
  either one.
- **A call can end before it has a subject.** No such tool, arguments the schema
  rejects, or a `plan` that fails — all three label the call with the tool's
  name first, or the surface shows a failure with no way to tell what failed.

## Naming firewall

Design notes may cite prior art. **No file in this repository may name another
project.** Record the conclusion, not the provenance. Enforced by
`scripts/firewall.sh`, pre-commit and in CI.

## Definition of done

- The relevant build and tests run clean, and their output was read.
- Anything unverified is labelled as such.
- Docs invalidated by the change are updated in the same change.
