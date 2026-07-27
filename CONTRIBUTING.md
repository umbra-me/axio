# Contributing

## Getting set up

```sh
git clone https://github.com/umbra-me/axio
cd axio
git config core.hooksPath .githooks   # one-time: runs the naming firewall pre-commit
cargo test --workspace
```

## What CI checks

The three-OS matrix runs clippy, the tests and the feature builds. The cheap
gates and the two Linux-only checks run once, on Linux. Run the cheap gates
locally before pushing; they fail in seconds.

```sh
bash scripts/firewall.sh                            # naming firewall
bash scripts/limits.sh                              # structural budgets
bash scripts/deps.sh                                # axio-core dependency isolation
bash scripts/check-windows.sh                       # core and tools still compile for Windows
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
bash scripts/features.sh                            # both feature sets, with CI's -D warnings
```

A third job runs `cargo-deny` over `deny.toml`, on Linux only: permissive
licences, no yanked or advisory-flagged crates, no wildcard versions, one TLS
stack, and no `aws-lc-*` — the default crypto provider, which wants a C
toolchain a clean Windows install is promised not to need. It is not part of a stock toolchain — `cargo install cargo-deny`, then
`cargo deny check`. It runs on every pull request, so a new upstream advisory
can turn it red when nothing about the dependencies changed.

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

`scripts/limits.sh` prints four numbers and enforces three: workspace members,
`ToolCx` fields and the widest single file fail the build; workspace Rust lines
excluding tests is reported and never does. They are guesses calibrated against
a predecessor that grew to 21 crates. Raising one is allowed — in a commit whose
message says why.

The total is the odd one out on purpose. The other three can always be satisfied
without giving anything up — by not adding a dependency edge, by not coupling a
tool to a host, by splitting a module. A total-lines ceiling can only be
satisfied by building less, and as it tightens it rewards density: comments are
free here and code is not, so it would push toward clever code with long
explanations. Seeing the number is what catches drift; failing on it only
interrupts someone mid-change.

A file past the per-file ceiling becomes child modules, never a new crate: the
member cap exists to keep the dependency graph honest, and a long file says
nothing about dependencies.

A fifth workspace member additionally needs a `### Workspace member
justification` section in `AGENTS.md` giving the dependency-isolation reason.

## Testing

Four tiers, in the order you should reach for them:

1. **Scripted provider.** Replay a `Vec<StreamEvent>` and assert on loop
   behaviour. Microseconds, no I/O. This is where most tests belong.
2. **Request-body snapshots.** `insta` over the exact outgoing JSON. Cache
   breakage is invisible until the bill arrives, so this is the highest-leverage
   single test in the suite — treat a snapshot change as a real review item.
3. **Wire-layer tests.** A handful, at the transport boundary only: byte-split
   SSE, 429 with `Retry-After`, mid-stream disconnect, oversize-prompt 400.
4. **End to end through the binary.** A stub HTTP server speaking a provider
   dialect, driven by the real `axio` process — for anything a library test
   structurally cannot see, like what a signal does to a running command.

None of those four touches a real provider. `bash scripts/live-check.sh` runs a
real turn against the configured model in a throwaway directory, and is the only
check that proves the wire format. It needs a credential and it spends money, so
it never runs in CI; run it by hand after a transport or provider change. It
has been run against the chat-completions transport and passed; the Messages
transport has no credential and has never been through it.

## Commits

Conventional Commits (`type(scope): description`), 72 characters or less,
imperative, lowercase, no trailing period. No AI attribution trailers.

## Naming firewall

Design notes may cite prior art. **Files in this repository may not name other
projects.** Record the conclusion, not the provenance. `scripts/firewall.sh`
enforces this and runs both pre-commit and in CI.
