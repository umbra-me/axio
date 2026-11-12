# axio

A minimal, fast coding agent. One small Rust binary, no runtime, no daemon.

**Status: public beta.** Versions are `0.x`: expect changes between minor
versions, each listed in [CHANGELOG.md](CHANGELOG.md) with breaking ones
called out. [docs/releasing.md](docs/releasing.md) explains the numbering.

```sh
./install.sh               # from a checkout: axio, ax, axio-full, axio-min
cargo install --git https://github.com/umbra-me/axio --locked   # just axio
axio                      # interactive; asks for a key on first run
axio auth set anthropic   # or set or change it here, or export ANTHROPIC_API_KEY
axio -p "explain src/"    # one answer, then exit
git diff | axio -p "review this"
axio -c                   # continue the last session in this directory
axio -r                   # pick one of this directory's recent sessions
axio update               # install the latest release (--check only looks)
```

Release binaries for macOS, Linux and Windows are on the
[releases page](https://github.com/umbra-me/axio/releases), each listed in
`SHA256SUMS`. `axio update` downloads the one for your platform, checks its
SHA-256 against that file and replaces the running binary; it only reads
from GitHub when you run it.

Runs on macOS and Linux. Windows support is experimental: it builds and
passes the test suite, and needs [Git for Windows](https://gitforwindows.org)
for `Bash` and hooks (`AXIO_BASH` points at another bash). Windows has no
`sandbox` module; read-only sub-agents are held to read-only by permission
rules instead.

## What it does

- **Four tools**: `Read`, `Write`, `Edit`, `Bash`, with Claude Code's names and
  parameters, so hooks written for Claude Code, Codex or Grok match them.
- **Providers**: Anthropic Messages (`anthropic/claude-opus-5-5`, the default)
  and any OpenAI-compatible chat completions endpoint (`openai/<id>`,
  `openrouter/<vendor>/<id>`, or `<name>/<id>` with `<NAME>_BASE_URL`).
  Several can be signed in at once, and `/model` switches between them in the
  middle of a conversation, tool calls included.
- **Sign in with ChatGPT**: `/login chatgpt` uses a ChatGPT subscription
  instead of an API key, through OpenAI's sign-in for third-party agents (it
  lists axio by name on your account). Models are `chatgpt/<model>`, run
  through the Responses API; the token is kept in `auth.json` and refreshed
  before it expires. When the browser is on another machine, paste the address
  it ends on into axio.
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
| `standard` (default) | the core plus the `search`, `plan`, `background` and `agents` modules |
| `full` | `standard` plus `web` and `sandbox` |

| Module | Adds |
|---|---|
| `search` | `Grep` and `Glob` (ripgrep when installed, a built-in walk otherwise) |
| `plan` | `TodoWrite`, a short task list shown as it changes |
| `background` | `Bash` with `run_in_background`: output goes to a log the model reads, `TaskStop` ends it, and the model hears when it exits. Tasks end when axio exits |
| `agents` | `Agent`: sub-agents, below |
| `web` | `WebFetch`: an http(s) GET, with HTML reduced to text and links kept. With a `prompt`, the fast model answers it from the page and only the answer enters the conversation |
| `sandbox` | Bash may write only in the project and temporary folders, and has no network unless `"sandboxNetwork": true`; Write and Edit follow the same limits. Seatbelt on macOS, bubblewrap on Linux; not available on Windows. Hooks and MCP servers are not sandboxed |

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

## Permissions

axio asks before it runs a command, changes a file, fetches a page or uses an
MCP tool. Reading and searching never ask. The answer can be yes, yes and
don't ask again in this project (saved in `~/.axio/agent/permissions.json`,
never in the repository, so a cloned project cannot grant itself anything),
yes for the rest of the session, or no.

| Mode | What runs without asking |
|---|---|
| `default` | reading and searching |
| `acceptEdits` | reading, searching and file changes |
| `plan` | reading and searching; changes and commands are refused, and the model proposes a plan |
| `bypassPermissions` | everything |

Shift+Tab cycles default → accept edits → plan; the status line shows the
mode when it is not the default. Start in a mode with `--permission-mode`,
`AXIO_PERMISSION_MODE` or `"permissions": {"defaultMode": ...}` in settings
(also in `/settings`); `--dangerously-skip-permissions` is bypass.
`/permissions` shows the mode and rules. Rules use Claude Code's syntax:

```json
{"permissions": {
  "allow": ["Bash(cargo test:*)", "Bash(git status)", "Edit(src/**)", "WebFetch(domain:docs.rs)", "mcp__mobbin"],
  "deny": ["Bash(rm:*)", "Edit(.env)"]
}}
```

`Bash(git log:*)` allows `git log` and anything after it. A command joined
with `&&`, `;` or `|` runs only when every part is allowed, and commands with
`$(...)`, `${...}`, backticks, `<` or `>` never match an allow rule, so they
ask. Deny rules win, even over bypass. A Bash deny rule checks every command
in the line, nested ones included, past assignments and wrappers such as `env`
or `sudo`. While any Bash deny rule is set, bypass mode runs without asking
only commands written in plain shell: words, quotes, `$NAME`, `;`, `&&`,
`||`, `|` and simple redirections. Anything else (here-documents, `$(…)`,
`((…))`, `case`, functions, backslashes, `!`) asks first, as does a command
whose program is named by an expansion, a wrapper given options, or anything
that runs other commands (`bash -c`, `eval`, `xargs`, `find -exec`, `trap`).
Deny rules cannot see what a script or another program runs, so treat them as
a guard against mistakes, and use the `sandbox` module or a container for a
hard boundary.
Sub-agents follow the same mode and rules, and their questions come to your
terminal. Without a terminal (`-p`), a call that would ask is refused with a
note on how to allow it. PreToolUse hooks can still allow, deny or ask.

## Sub-agents

The `Agent` tool hands a self-contained task to a sub-agent: another `axio`
process (about 4 MB) with only the tools, instructions, model and context its
role needs. It returns only its final answer, capped at 8,000 characters; its
full transcript is saved as its own session, under the directory's
`agents/` folder, which `-c` and `-r` do not list. Several `Agent` calls in
one reply run at once (up to four), one spinner line shows what they are
doing, and Esc stops them all.

Roles are markdown files in Claude Code's agent format, so Claude Code agents
work unchanged. `axio agents` lists them. Four are built in:

| Role | Tools | Model, effort | Sees | Sandbox |
|---|---|---|---|---|
| `explore` | Read, Grep, Glob | the main agent's | brief | read-only |
| `research` | WebFetch, Read | the main agent's | brief | read-only |
| `review` | Read, Grep, Glob, Bash | the main agent's, high | rules | read-only |
| `code` | Read, Write, Edit, Bash, Grep, Glob | the main agent's | rules | the main agent's |

A role may list MCP tools by name or as `mcp__server__*`. Sub-agents start no
MCP servers of their own: the main agent runs those calls on its servers and
passes the results back, and refuses tools the role was not given.

Add or replace roles in `~/.axio/agent/agents/<name>.md`, a plugin's
`agents/` folder or a trusted project's `.axio/agents/`; a later one replaces
a role of the same name.

```markdown
---
name: tester
description: Writes and runs tests for a change.
tools: Read, Write, Edit, Bash, Grep, Glob
model: fast          # fast | inherit | choose | provider/model (Claude Code's haiku, sonnet and opus work too)
effort: medium       # inherit | choose | a level
maxTurns: 30         # rounds of tools before it must answer (50 by default)
context: rules       # brief | rules (adds AGENTS.md) | fork (adds a transcript of the conversation)
sandbox: project     # read-only | project | inherit (default)
---
You write tests for the change described in the brief…
```

Built-in roles use the main agent's model and effort; a role runs on a
smaller model only when you set it, for example
`"subagents": {"roles": {"explore": {"model": "fast", "effort": "low"}}}`.
`fast` is `subagents.fastModel`, else Haiku 4.5 on Anthropic or
`gpt-6-luna` on a ChatGPT sign-in, else the main agent's model. A sub-agent's
sandbox is never looser than the main agent's. Settings, all optional:

```json
{"subagents": {
  "nesting": 0,
  "parallel": 4,
  "fastModel": "anthropic/claude-haiku-4-5",
  "choices": ["anthropic/claude-haiku-4-5", "chatgpt/gpt-6-luna"],
  "resultLimit": 8000,
  "fast": false,
  "roles": {"review": {"model": "inherit", "effort": "choose"}}
}}
```

`nesting` is how many levels below the main agent may start sub-agents of
their own (none by default). `choices` limits what the main agent may pick for
roles set to `choose`. `roles.<name>` overrides `model`, `effort`, `context`,
`maxTurns` or `sandbox` without editing the file.

## Using what other harnesses already have

`axio import` looks at Claude Code, Codex, Grok Build and Pi on this machine:
their hooks, installed plugins, MCP servers, skills and Claude Code's
sub-agents (`~/.claude/agents`), which become roles. It lists each item
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

`~/.axio/agent/settings.json` holds `model`, `effort` and `profile`, and
`compactModel` (`fast`, `inherit` or a model) for the model that writes
compaction summaries; it defaults to the main model.

Fast mode runs the same model up to 2.5x faster at a higher price, on the
models that have it: Claude Opus 5.5 ($8 / $40 per million tokens), Fable 5.2
and Mythos 5.2 ($16 / $80), and as a research preview Opus 5 and Opus 4.8
($10 / $50). It is off by default: `/fast` turns it on or off for the session
and says the price, `"fastMode": true` makes it the default, and the header
shows `fast` while it applies. Sub-agents and side jobs (compaction, WebFetch
prompts) never use it unless `"subagents": {"fast": true}`. If the provider
refuses fast mode with a rate limit (zero without preview access), axio
continues at standard speed for the rest of the session.
`/effort <level>` changes reasoning effort for the session. A
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
| `SubagentStart` | before a sub-agent starts (matcher: its role) | add context to its brief |
| `SubagentStop` | a sub-agent has answered (matcher: its role) | send it back to work in its own session (up to 8 times) |

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

Remote servers use streamable HTTP (`"type": "http"`, or just a `url`) or the
older HTTP+SSE transport (`"type": "sse"`), with `headers` and sign-ins
(`axio mcp login <name>`) working the same for both.

Their tools appear as `mcp__<server>__<tool>`, so hooks can match them with
`mcp__github__.*`. Local servers run over stdio for the session; a server that
exits is restarted on its next call, and Esc cancels a running call. `timeout`
(milliseconds, default ten minutes) bounds each call. Results keep their text
and text resources; images (PNG, JPEG, GIF or WebP up to 3.75 MB, including
image resources) go to the model after the result; other content such as audio,
binary resources and resource links is named so the model knows it is there.

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
the `mcp` section of `~/.axio/agent/auth.json`.

## Keys

| Key | Does |
|---|---|
| Enter | send |
| Alt-Enter, or `\` then Enter | new line |
| Up / Down | move between lines, then through history |
| Esc or Ctrl-C | stop the running turn (a running command is killed) |
| Enter while a turn runs | send what you typed to the model at its next step, or as the next prompt if the turn ends first |
| Ctrl-C twice, or Ctrl-D, on an empty line | exit |
| Ctrl-A / Ctrl-E, Alt-B / Alt-F, Ctrl-W, Ctrl-U, Ctrl-K | usual line editing |
| `/` | open the command menu: type to filter, Up/Down to move, Enter runs, Tab completes, Esc closes |
| Tab | complete a `/command`, or a path after `@` |
| Ctrl-R | search prompt history (Enter takes the match, Esc cancels) |

`@path` in a prompt attaches that file (up to 256 KB), an image (PNG, JPEG,
GIF or WebP, up to 3.7 MB) or a directory listing, so the model does not have
to read it first. Dropping an image file on the terminal pastes its path as an
`@` mention.

Commands:

- `/model` opens a model browser: providers on the left (`all` searches every
  provider; ones not set up offer sign-in), the highlighted provider's models
  on the right with their context size. It opens on the model in use. Tab
  moves between the two sides, Up/Down move, Enter or Right goes into a
  provider's models, typing filters them, and a name no list holds can be
  typed and used. `/model provider/model` switches directly.
- `/login [provider]` saves an API key, or a name, base URL
  and optional key for any other OpenAI-compatible endpoint; `/logout
  [provider]` removes one. Keys go to `~/.axio/agent/auth.json`.
- `/effort` opens a slider of the levels the current model takes (with each
  level's description when the provider gives one); `/effort <level>` sets it.
  The model browser also shows the effort under the list: Left and Right
  change it for the highlighted model.
- `/resume` picks a saved session from this directory to continue.
- Esc twice on an empty prompt (or `/rewind`) lists your messages in this
  session; choosing one cuts the conversation back to just before it and
  puts it in the prompt to edit and send again. When files were written or
  edited since, it offers to put them back too (commands are not undone).
  Resuming the session keeps the rewind.
- `/history` picks any prompt you typed before, in any session, into the
  prompt to edit.
- A dim status line under the prompt shows the model, how full the context
  is and what the session has cost so far: an estimate from each reply's
  token counts and the published prices of Claude models (fast mode, cache
  reads and writes, side jobs and sub-agents included). A ChatGPT sign-in
  shows "ChatGPT plan"; models without a known price show their tokens.
  `/status` repeats it.
- The line after each turn shows the output speed (output tokens a second,
  timed from each reply's first token to its end) and the wait for the first
  token; `/status` gives the session average.
- Each item on both lines can be switched off: `/settings` → "Under the
  prompt" or "After each turn" (Enter switches an item, Esc saves; it
  applies at once), or in settings.json, where an empty list hides the line:
  `"statusLine": {"underPrompt": ["model", "context", "cost"], "afterTurn":
  ["context", "output", "cache", "time", "speed", "first"]}`.
- `/context` shows what fills the context window: a bar, then the system
  prompt, AGENTS.md, skills list, MCP guidance, built-in tools, the Agent
  tool, each MCP server's tools and the messages, with the free space and
  where compaction starts.
- `/settings` lists every setting with its value; Enter toggles, picks or
  asks for a new one and saves it in `~/.axio/agent/settings.json` for new
  sessions.
- `/init [notes]` asks the model to write or improve the project's
  AGENTS.md.
- Long tool output is collapsed to its end; Ctrl-O on an empty prompt (or
  `/output`) shows the last result in full.
- `/mcp` lists MCP servers with their state (tools and transport, signed in,
  needs sign-in, or why one is not connected); choosing one shows its tools,
  and HTTP servers can be signed in to or out of from there.
- `/skills` lists skills and commands, the ones you can run first; Enter runs
  one, or shows where a model-only skill lives. `/agents` lists sub-agent
  roles and shows one in full. `/hooks` lists hooks by event with their
  source, `/plugins` what each plugin provides, and `/status` the model,
  profile, project, session, context and servers at a glance.
- `/new`, `/undo` (put back the files the last turn wrote or edited),
  `/output` (the last tool call's full result), `/compact`, `/profile`,
  `/session`, `/help`, `/quit`, plus plugin commands and skills.

In both, Enter makes the choice the default for new sessions (saved as
`"model"` and `"effort"` in `~/.axio/agent/settings.json`) and Ctrl-S uses it
for this session only (`s` in the effort slider; in the browser Tab switches
sides and typing filters).
Keys typed while a turn runs are kept for the next prompt.

## Security and privacy

- **What leaves your machine**: your conversation, the files and command
  output tools read, and AGENTS.md go to the model provider you chose. MCP
  servers get the calls the model makes to them, and WebFetch fetches the
  addresses the model asks for. axio sends no telemetry. `axio update` is the
  only time it contacts GitHub.
- **What stays on it**: keys in `~/.axio/agent/auth.json`, sessions, prompt
  history and command logs under `~/.axio/agent/`. On macOS and Linux these
  are readable only by you (folders 0700, files 0600). On Windows they take
  your profile folder's permissions. Sessions and logs hold whatever tools
  saw, secrets included, so treat them like the files themselves.
- **What can run**: commands the model asks for, in your user's name, under
  the permission mode you pick (see [Permissions](#permissions)). A project's
  own `.axio/` folder and `.mcp.json` can start programs, so they load only
  after you trust that project. The `sandbox` module (macOS and Linux)
  confines commands to the project and temporary folders with no network.
- **Updates**: `axio update` installs a release binary only when its SHA-256
  matches the release's `SHA256SUMS`. That guards against a damaged or
  swapped download, not against a compromised release.

Report a vulnerability privately as described in [SECURITY.md](SECURITY.md).

## What it leaves out

`/undo` and `/rewind` put back files written by Write and Edit, not changes
made by commands. Commands run with your user's rights in the working
directory: keep the default permission mode, or use the sandbox, where that
matters.

## Dependencies

- `serde`, `serde_json`: messages and provider JSON
- `ureq`: blocking HTTPS with rustls, so there is no async runtime

Axio started as a Rust CLI agent in 2026. This repository is a fresh start on
that idea; the desktop app that grew out of it is Axio Atrium.

## Licence

axio is licensed under the [Apache License, Version 2.0](LICENSE).
