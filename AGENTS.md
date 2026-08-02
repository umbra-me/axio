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
cargo build --release -p axio-quota --features app              # tray + flyout + window
npm --prefix crates/axio-quota/ui run build                     # frontend, before that
node crates/axio-quota/icons/make-icon.mjs                      # regenerate icon.ico

INSTA_UPDATE=always cargo test -p axio-provider     # re-accept request-body snapshots
bash scripts/live-check.sh                          # a real turn against a real model
git config core.hooksPath .githooks                 # one-time, per clone
```

## Architecture

Five crates and two binaries. The dependency graph is a tree rooted at
`axio-core`, so there are no cycles and no cycle-breaker crates.

| Crate | Owns |
| --- | --- |
| `axio-core` | `protocol`, the `Provider` / `Tool` / `Approver` traits, the turn loop, `Session` and its JSONL records, layered `config`, `compact`, `policy`, output `truncate`, credential storage, `Workspace`, `Redacted` |
| `axio-provider` | All three transports — the Messages dialect, chat-completions, and Responses — plus the OAuth flows: SSE decoder, request builders, block state machines, error classification. The only crate linking HTTP or TLS |
| `axio-tools` | The six tools, subprocess helpers, diff previews, byte-stable schemas. The only crate that walks a filesystem or spawns a process |
| `axio-quota` | Provider quota probes — the credential files `codex` and `claude` wrote, and the usage endpoints behind them — plus local history, and behind the `app` feature the Tauri desktop surface: tray icon, HTML flyout, window. Depends on nothing else in the workspace |
| `axio` | clap, surface selection, renderers, the inline TUI and its slash commands, the optional Landlock sandbox, `Approver` implementations, `axio quota` |

### Workspace member justification

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

### axio-quota

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
