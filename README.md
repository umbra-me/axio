# axio

A cross-platform AI coding agent — Rust, with a CLI and a TUI.

axio runs its own agent loop: it talks to LLM providers directly, executes tool
calls against your workspace, and streams the result back to you.

This repo is a from-scratch rewrite; the previous implementation is archived and
read-only.

## Status

Early, but it works: the turn loop, the provider transport, the one-shot CLI,
all six tools, sessions on disk, and layered configuration. It can read, search,
edit and run commands in a workspace, and pick up where it left off.

Writes and shell commands ask before they happen; reads do not.

```sh
export ANTHROPIC_API_KEY=sk-ant-...
cargo run -p axio -- -p "explain this repo"
cat src/lib.rs | cargo run -p axio -- -p "review this"
cargo run -p axio -- --doctor       # what axio can currently see

axio --list                         # recent sessions
axio --resume 01K3F               # continue one; a unique prefix is enough
axio --explain model.effort         # where a setting came from
axio --ephemeral -p "..."           # record nothing
```

Configuration is layered — defaults, then `~/.config/axio/config.toml`, then the
nearest `.axio/config.toml`, then `AXIO_*` variables, then flags:

```toml
[model]
effort = "xhigh"

[budget]
max_usd_per_turn = 2.0
max_steps = 50

[permissions]
deny = ["bash:curl"]
```

A project's own `.axio/config.toml` may only add restrictions, never remove
them.

## Providers

Two, selected by name — not a plugin system:

```sh
export ANTHROPIC_API_KEY=sk-ant-...        # the default
axio -p "..."

export OLLAMA_API_KEY=...                   # anything speaking the
AXIO_PROVIDER=ollama AXIO_MODEL=gpt-oss:120b axio -p "..."   # chat-completions dialect
```

The second exists mostly to keep the first honest: implementing a differently
shaped API is how you find out whether an abstraction is one.

Actions that change something ask first. `--yes` approves everything without
asking — unattended and unsandboxed, so read [`SECURITY.md`](SECURITY.md) before
reaching for it.

`--json` emits the event stream as one object per line. It is unstable and
carries a `protocol` version so a consumer can refuse a stream it does not
understand.

## Shape

Four crates, and one binary serving both surfaces:

| Crate           | Contains                                                        |
| --------------- | --------------------------------------------------------------- |
| `axio-core`     | Conversation state, turn loop, `Tool` and `Provider` traits      |
| `axio-provider` | Provider HTTP client, streaming, request/response types          |
| `axio-tools`    | `read`, `write`, `edit`, `bash`, `glob`, `grep`                  |
| `axio`          | The binary — one-shot CLI when piped or given `-p`, interactive on a TTY |

The core emits a stream of events and knows nothing about rendering, so each
surface is a consumer of the same channel. `--json` is a second renderer, never
a second loop.

See [`docs/architecture.md`](docs/architecture.md) for the invariants,
[`docs/gotchas.md`](docs/gotchas.md) for the traps, and
[`docs/roadmap.md`](docs/roadmap.md) for what is coming and what is deliberately
not.

## Safety

axio executes code written by a language model against your working directory.
There is no sandbox: confinement is the workspace root, the approval prompt and
process-group containment. Read [`SECURITY.md`](SECURITY.md) before running it
anywhere that matters.

## License

Apache-2.0. See [`LICENSE`](LICENSE).
