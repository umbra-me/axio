# AGENTS.md

## Commands

```sh
cargo test --workspace                              # everything
cargo test -p axio-core                             # the loop, against fakes; must stay under 2s
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
bash scripts/features.sh                            # both feature sets, with CI's -D warnings

bash scripts/firewall.sh                            # naming firewall (also a pre-commit hook)
bash scripts/limits.sh                              # workspace members, ToolCx fields, LOC
bash scripts/deps.sh                                # axio-core links no transport/terminal/walker
bash scripts/check-windows.sh                       # cfg-gated code still compiles for windows

INSTA_UPDATE=always cargo test -p axio-provider     # re-accept request-body snapshots
bash scripts/live-check.sh                          # a real turn against a real model
git config core.hooksPath .githooks                 # one-time, per clone
```

## Architecture

Four crates and one binary. The dependency graph is a tree rooted at
`axio-core`, so there are no cycles and no cycle-breaker crates.

| Crate | Owns |
| --- | --- |
| `axio-core` | `protocol`, the `Provider` / `Tool` / `Approver` traits, the turn loop, `Session` and its JSONL records, layered `config`, `compact`, `policy`, `Workspace`, `Redacted` |
| `axio-provider` | The Messages transport: SSE decoder, request builder, block state machine, error classification. The only crate linking HTTP or TLS |
| `axio-tools` | The six tools, subprocess helpers, output truncation. The only crate that walks a filesystem or spawns a process |
| `axio` | clap, surface selection, renderers, the TUI, `Approver` implementations |

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
- **`cargo-deny` bans a second TLS stack.** If `openssl-sys` or `native-tls`
  appears, something pulled in a default feature set we turned off.

## Naming firewall

Design notes may cite prior art. **No file in this repository may name another
project.** Record the conclusion, not the provenance. Enforced by
`scripts/firewall.sh`, pre-commit and in CI.

## Definition of done

- The relevant build and tests run clean, and their output was read.
- Anything unverified is labelled as such.
- Docs invalidated by the change are updated in the same change.
