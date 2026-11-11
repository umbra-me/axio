# axio

A minimal, fast coding agent. One small Rust binary, no runtime, no daemon.

```sh
./install.sh               # from a checkout: axio, ax, axio-full, axio-min
cargo install --git https://github.com/umbra-me/axio --locked   # just axio
axio                      # interactive; asks for a key on first run
axio auth set anthropic   # or set or change it here, or export ANTHROPIC_API_KEY
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
| `full` | `standard` plus `web` and `sandbox` |

| Module | Adds |
|---|---|
| `search` | `Grep` and `Glob` (ripgrep when installed, a built-in walk otherwise) |
| `plan` | `TodoWrite`, a short task list shown as it changes |
| `background` | `Bash` with `run_in_background`: output goes to a log the model reads, `TaskStop` ends it, and the model hears when it exits. Tasks end when axio exits |
| `web` | `WebFetch`: an http(s) GET, with HTML reduced to text and links kept |
| `sandbox` | Bash may write only in the project and temporary folders, and has no network unless `"sandboxNetwork": true`; Write and Edit follow the same limits. Seatbelt on macOS, bubblewrap on Linux. Hooks and MCP servers are not sandboxed |

Choose one with `--profile`, `AXIO_PROFILE` or `"profile"` in settings;
`/profile` shows the current one. `axio profile full` (or `min`, `standard`,
or your own) makes it the default. The command name works too: `ax` is the
same as `axio`, and `axio-full`, `axio-min` or any `axio-<profile>` /
`ax-<profile>` start that profile, because `install.sh` links those names to
the one binary. Your own go in
`~/.axio/agent/profiles/<name>.json`:

```json
{"extends": "minimal", "modules": ["plan"], "plugins": ["guard"], "model": "anthropic/claude-sonnet-5-5", "compactAt": 0.7}
```

`plugins` lists the plugin names to load (`"*"` for all, the default).

## Using what other harnesses already have

`axio import` looks at Claude Code, Codex, Grok Build and Pi on this machine:
their hooks, installed plugins, MCP servers and skills. It lists each item
with its version, the harness and its version, and whether it works as is,
partly (and what is missing) or not at all, then asks: import everything that
works, choose, later, or never ask. Accepted items are used where they are, so
updates in the other harness carry over; answers are kept in
`~/.axio/agent/imports.json`.

This is off by default. Accepting something sets
`"importFromOtherHarnesses": "ask"`, so accepted items load and new ones are
offered at start; set it to `"off"` (or pass `--no-import`) to stop. Codex and
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
`mcp__github__.*`. Local servers run over stdio for the session; a server that
exits is restarted on its next call, and Esc cancels a running call. `timeout`
(milliseconds, default ten minutes) bounds each call.

Remote servers use streamable HTTP:

```json
{"mcpServers": {"tracker": {"type": "http", "url": "https://mcp.example.com/mcp",
                            "headers": {"Authorization": "Bearer ${TRACKER_TOKEN}"}}}}
```

`${VAR}` in `url` and `headers` comes from the environment. A server that
needs an OAuth sign-in says so at start; sign in once with

```sh
axio mcp login tracker     # opens the browser; tokens refresh on their own
axio mcp list              # servers, and which are signed in
axio mcp logout tracker
```

Sign-in follows the MCP authorization spec (protected-resource discovery,
dynamic client registration, PKCE, resource indicators). Tokens are kept in
the `mcp` section of `~/.axio/agent/auth.json`. The older SSE transport
is skipped.

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
