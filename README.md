# axio

A minimal, fast coding agent. One small Rust binary, no runtime, no daemon.

```sh
cargo install --git https://github.com/umbra-me/axio --locked
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

## What it leaves out

No MCP, no sub-agents, no plan mode, no permission prompts, no built-in to-do
list. The model runs commands with your user's rights in the working
directory, so run it where that is acceptable.

## Dependencies

- `serde`, `serde_json`: messages and provider JSON
- `ureq`: blocking HTTPS with rustls, so there is no async runtime

Axio started as a Rust CLI agent in 2026. This repository is a fresh start on
that idea; the desktop app that grew out of it is Axio Atrium.
