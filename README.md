# axio

A cross-platform AI coding agent — Rust, with a CLI and a TUI.

axio runs its own agent loop: it talks to LLM providers directly, executes tool
calls against your workspace, and streams the result back to you.

This repo is a from-scratch rewrite; the previous implementation is archived and
read-only.

## Status

Early. The turn loop, the provider transport and the one-shot CLI work; the
tools do not exist yet, so it can talk but not act.

```sh
export ANTHROPIC_API_KEY=sk-ant-...
cargo run -p axio -- -p "explain this repo"
cat src/lib.rs | cargo run -p axio -- -p "review this"
cargo run -p axio -- --doctor       # what axio can currently see
```

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
