# axio

A cross-platform AI coding agent — Rust, with a CLI and a TUI.

axio runs its own agent loop: it talks to LLM providers directly, executes tool
calls against your workspace, and streams the result back to you.

This repo is a from-scratch rewrite; the previous implementation is archived and
read-only.

## Status

Empty. Nothing is built yet.

## Planned shape

Four crates, and one binary serving both surfaces:

| Crate           | Contains                                                        |
| --------------- | --------------------------------------------------------------- |
| `axio-core`     | Conversation state, turn loop, `Tool` and `Provider` traits      |
| `axio-provider` | Provider HTTP client, streaming, request/response types          |
| `axio-tools`    | `read`, `write`, `edit`, `bash`, `glob`, `grep`                  |
| `axio`          | The binary — one-shot CLI when piped or given `-p`, TUI on a TTY |

The core emits a stream of agent events and knows nothing about rendering, so
each surface is a consumer of the same channel. A desktop app is deferred until
the core is real.

## License

Apache-2.0. See [`LICENSE`](LICENSE).
