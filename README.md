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

- **Four tools**: `Read`, `Write`, `Edit`, `Bash`, with Claude Code's names and
  parameters, so hooks written for Claude Code, Codex or Grok match them.
- **Providers**: Anthropic Messages (`anthropic/claude-opus-5-5`, the default)
  and any OpenAI-compatible chat completions endpoint (`openai/<id>`,
  `openrouter/<vendor>/<id>`, or `<name>/<id>` with `<NAME>_BASE_URL`).
- **Context**: `~/.axio/agent/AGENTS.md`, then every `AGENTS.md` (or
  `CLAUDE.md`) from the filesystem root down to the working directory.
- **Sessions**: append-only JSONL under `~/.axio/agent/sessions/`, grouped by
  directory. `-c` continues the latest; `-s <file>` opens one.
- **Compaction**: at 85% of the model's context window, the conversation is
  replaced by your recent requests (about 20k tokens, verbatim) and a summary
  of everything else. `/compact` does it on
  demand; `AXIO_CONTEXT_WINDOW` and `AXIO_COMPACT_AT` override the numbers.

## Profiles and modules

A profile picks built-in modules, which plugins load, and defaults.

| Profile | Tools |
|---|---|
| `minimal` | Read, Write, Edit, Bash: the Pi-sized core |
| `standard` (default) | the core plus the `search`, `plan` and `background` modules |

| Module | Adds |
|---|---|
| `search` | `Grep` and `Glob` (ripgrep when installed, a built-in walk otherwise) |
| `plan` | `TodoWrite`, a short task list shown as it changes |
| `background` | `Bash` with `run_in_background`: output goes to a log the model reads, `TaskStop` ends it, and the model hears when it exits. Tasks end when axio exits |

Choose one with `--profile`, `AXIO_PROFILE` or `"profile"` in settings;
`/profile` shows the current one. Your own go in
`~/.axio/agent/profiles/<name>.json`:

```json
{"extends": "minimal", "modules": ["plan"], "plugins": ["guard"], "model": "anthropic/claude-sonnet-5-5", "compactAt": 0.7}
```

`plugins` lists the plugin names to load (`"*"` for all, the default).

## Using what other harnesses already have

On an interactive start, axio looks at Claude Code, Codex, Grok Build and Pi
on this machine: their hooks, installed plugins, MCP servers and skills. When
it finds something new or changed it lists each item with its version, the
harness and its version, and whether it works as is, partly (and what is
missing) or not at all, then asks: import everything that works, choose,
later, or never ask. Accepted items are used where they are, so updates in the
other harness carry over; answers are kept in `~/.axio/agent/imports.json`.
`axio import` reviews everything again, `--no-import` skips it for one run,
and `"importFromOtherHarnesses": "off"` in settings turns it off. Codex and
Grok keep MCP servers in TOML, which axio reads with a small built-in reader.

## Settings and trust

`~/.axio/agent/settings.json` holds `model`, `effort` and `profile`. A
project's `.axio/settings.json` overrides them, but project configuration
(`.axio/`, `.mcp.json`) can make axio run programs, so it loads only after you
trust the project: axio asks once, remembers the answer in
`~/.axio/agent/trust.json`, and `--trust` or `--no-project-config` decide it for
one run. Flags beat environment variables, which beat settings.

## Hooks and plugins

Hooks are commands axio runs at fixed points, in the format Claude Code, Codex
and Grok Build use: the event arrives as JSON on stdin, and the hook answers
with its exit code (2 blocks, with stderr as the reason) or JSON on stdout
(`decision`, `continue`, `systemMessage`, and `hookSpecificOutput` with
`permissionDecision`, `updatedInput` or `additionalContext`).

| Event | When | A hook can |
|---|---|---|
| `SessionStart` | session opens or resumes | add context |
| `UserPromptSubmit` | before a prompt is sent | block it, add context |
| `PreToolUse` | before a tool runs | allow, deny, ask you, rewrite the input |
| `PostToolUse` | after a tool returns | add context, flag the result |
| `Stop` | the model is about to finish | send it back to work (up to 8 times) |
| `PreCompact` | before compaction | add summary instructions, skip it |
| `SessionEnd` | axio exits | observe |

Declare hooks under `"hooks"` in a settings file, or ship them in a plugin:
a folder in `~/.axio/agent/plugins/` (or a trusted project's
`.axio/plugins/`) with `hooks/hooks.json`, in the Claude Code plugin layout.
[`examples/plugins/guard`](examples/plugins/guard) blocks destructive commands
and asks before `git push`.

```sh
axio plugin install ./examples/plugins/guard     # a folder
axio plugin install owner/repo                   # GitHub
axio plugin install git:host/owner/repo@v1.2     # any git host, pinned
axio plugin list | update [name] | remove <name> # --project for .axio/plugins
```

Installing runs nothing: it copies or clones the plugin, records where it
came from, and lists the hooks and MCP servers it will run from the next
session. `update` refuses a git plugin with local edits.
Matchers support `*`, exact names, `A|B` and prefixes like `mcp__github__.*`.

## Skills and commands

- **Skills** are `skills/<name>/SKILL.md` files with `name` and `description`
  frontmatter. The system prompt lists each one in a line, and the model reads
  the file when a task matches. A skill with `user-invocable: true` can also
  be run as `/name`.
- **Commands** are `commands/<name>.md` files. `/name args` sends the body as
  your prompt, with `$ARGUMENTS` (or `$1` to `$9`) filled in; plugin commands
  also answer to `/plugin:name`. `/help` lists them.

Both are read from a trusted project's `.axio/`, then `~/.axio/agent/`, then
plugins; the first definition of a name wins.

## MCP servers

axio starts MCP servers listed in `~/.axio/agent/mcp.json`, in plugins'
`.mcp.json`, and in a trusted project's `.mcp.json`, in the usual format:

```json
{"mcpServers": {"github": {"command": "github-mcp", "args": ["stdio"], "env": {"TOKEN": "…"}}}}
```

Their tools appear as `mcp__<server>__<tool>`, so hooks can match them with
`mcp__github__.*`. Servers run over stdio for the session; a server that
exits is restarted on its next call, and Esc cancels a running call. `timeout`
(milliseconds, default ten minutes) bounds each call. HTTP and SSE servers are
listed as skipped for now.

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
