# Contributing

## Getting set up

```sh
git clone https://github.com/umbra-me/axio
cd axio
git config core.hooksPath .githooks   # one-time: runs the naming firewall pre-commit
cargo test --workspace
```

## What CI checks

Everything below runs on Linux, macOS and Windows. Run the cheap gates locally
before pushing; they fail in seconds.

```sh
bash scripts/firewall.sh                            # naming firewall
bash scripts/limits.sh                              # structural budgets
bash scripts/deps.sh                                # axio-core dependency isolation
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build -p axio --no-default-features           # the headless path must not rot
```

There is deliberately **no local pre-push gate** beyond the firewall hook. Plain
CI is the gate.

## The three invariants

Changes that break one of these need a very good argument in the pull request.

1. **`Tool::run` is the only execution path.** `plan()` is pure and produces the
   preview, the policy subject and an opaque payload; `run()` applies exactly
   that payload. The turn loop never matches on a tool name. This is what makes
   "what you approved is what ran" true by construction rather than by care.
2. **`ToolCx` is closed.** Five concrete fields, none optional, none `dyn`. If a
   tool needs a capability that is not on it, the tool does not ship. CI counts
   the fields.
3. **Surfaces consume events and supply an `Approver`.** Nothing else crosses
   the boundary. `axio-core` links no HTTP stack, no terminal library, no
   filesystem walker and no subprocess machinery; CI asserts it.

## Structural budgets

`scripts/limits.sh` prints and enforces three numbers: workspace members,
`ToolCx` fields, and workspace Rust lines excluding tests. They are guesses
calibrated against a predecessor that grew to 21 crates. Raising one is allowed
— in a commit whose message says why.

A fifth workspace member additionally needs a `### Workspace member
justification` section in `AGENTS.md` giving the dependency-isolation reason.

## Testing

Three tiers, in the order you should reach for them:

1. **Scripted provider.** Replay a `Vec<StreamEvent>` and assert on loop
   behaviour. Microseconds, no I/O. This is where most tests belong.
2. **Request-body snapshots.** `insta` over the exact outgoing JSON. Cache
   breakage is invisible until the bill arrives, so this is the highest-leverage
   single test in the suite — treat a snapshot change as a real review item.
3. **Wire-layer tests.** A handful, at the transport boundary only: byte-split
   SSE, 429 with `Retry-After`, mid-stream disconnect, oversize-prompt 400.

## Commits

Conventional Commits (`type(scope): description`), 72 characters or less,
imperative, lowercase, no trailing period. No AI attribution trailers.

## Naming firewall

Design notes may cite prior art. **Files in this repository may not name other
projects.** Record the conclusion, not the provenance. `scripts/firewall.sh`
enforces this and runs both pre-commit and in CI.
