# Axio

Axio is a minimal coding agent harness: one Rust binary, four tools (read,
write, edit, bash), streaming providers, JSONL sessions and AGENTS.md context.
The desktop super app that used to live here is now Axio Atrium
(`umbra-me/axio-atrium`).

## Rules

- Keep it small. A feature earns its place only when it cannot live in a
  prompt, an AGENTS.md file or a shell command. Prefer deleting code to adding
  options.
- Dependencies: serde, serde_json and ureq only. Adding a crate needs the
  owner's approval and a line in README.md saying why.
- No async runtime. Blocking I/O and threads are enough for one agent.
- Every behaviour change starts with a failing test. Provider streams are
  tested from recorded SSE text, never from live calls.
- Live provider runs spend the owner's money: ask first.

## Checks

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
```

## Layout

- `src/main.rs`: arguments, terminal output, interactive loop
- `src/agent.rs`: the turn loop (model, tools, repeat)
- `src/provider/`: Anthropic Messages and OpenAI-compatible chat completions
- `src/tools.rs`: read, write, edit, bash
- `src/session.rs`: append-only JSONL transcripts
- `src/context.rs`: system prompt and AGENTS.md discovery
