# axio

A minimal, fast coding agent. One small Rust binary, no runtime, no daemon.

```sh
cargo install --git https://github.com/umbra-me/axio --locked   # or: cargo install --path . --locked
export ANTHROPIC_API_KEY=...
axio                      # interactive
axio -p "explain src/"    # one answer, then exit
git diff | axio -p "review this"
axio -c                   # continue the last session in this directory
```

## What it does

- **Four tools**: `read`, `write`, `edit`, `bash`. Anything else is a command
  the model can run.
- **Providers**: Anthropic Messages (`anthropic/claude-opus-5-5`, the default)
  and any OpenAI-compatible chat completions endpoint (`openai/<id>`,
  `openrouter/<vendor>/<id>`, or `<name>/<id>` with `<NAME>_BASE_URL`).
- **Context**: `~/.axio/agent/AGENTS.md`, then every `AGENTS.md` (or
  `CLAUDE.md`) from the filesystem root down to the working directory.
- **Sessions**: append-only JSONL under `~/.axio/agent/sessions/`, grouped by
  directory. `-c` continues the latest; `-s <file>` opens one.
- **Compaction**: at 85% of the model's context window, older turns are
  summarized and the last ~20k tokens stay verbatim. `/compact` does it on
  demand; `AXIO_CONTEXT_WINDOW` and `AXIO_COMPACT_AT` override the numbers.

## Keys

| Key | Does |
|---|---|
| Enter | send |
| Alt-Enter, or `\` then Enter | new line |
| Up / Down | move between lines, then through history |
| Esc or Ctrl-C | stop the running turn (a running command is killed) |
| Ctrl-C twice, or Ctrl-D, on an empty line | exit |
| Ctrl-A / Ctrl-E, Alt-B / Alt-F, Ctrl-W, Ctrl-U, Ctrl-K | usual line editing |

Commands: `/new`, `/compact`, `/model <provider/model>`, `/session`, `/quit`.
Keys typed while a turn runs are kept for the next prompt.

## What it leaves out

No MCP, no sub-agents, no plan mode, no permission prompts, no built-in to-do
list. The model runs commands with your user's rights in the working
directory, so run it where that is acceptable.

## Dependencies

- `serde`, `serde_json`: messages and provider JSON
- `ureq`: blocking HTTPS with rustls, so there is no async runtime

Axio started as a Rust CLI agent in 2026. This repository is a fresh start on
that idea; the desktop app that grew out of it is Axio Atrium.
