# Plugins, modules and profiles

Status: proposal for owner approval, 2026-11-12.

## Goal

Keep the axio core Pi-sized and fast, and make everything else optional:

- **Plugins** add behaviour without recompiling. They are folders of
  executables and markdown in the Claude Code plugin layout, which Codex and
  Grok Build also follow. Nothing runs inside axio except axio: no JavaScript
  runtime, no WebAssembly engine, no loaded libraries.
- **Modules** are larger built-in features compiled into axio, behind Cargo
  features when they need new dependencies.
- **Profiles** are named starting points (`minimal`, `standard`, `full`, or
  your own) that pick modules, plugins and defaults.

Non-goals for this round: a marketplace, HTTP MCP transports, sub-agents,
plugin UI widgets.

## What the references do

| | Codex (e58bf8f) | Grok Build (2bdd1d6) | Pi (5f96671) |
|---|---|---|---|
| Runtime-loaded code | none | none | TypeScript in-process |
| Plugin bundle | manifest + skills, MCP servers, hooks, apps | hooks, MCP, skills, agents; git marketplace | npm/git packages |
| Hooks | commands, JSON on stdin; Claude Code wire format | same, project hooks need trust; imports `~/.claude` hooks | event handlers in TS |
| Built-in extensions | Rust crates in `codex-rs/ext/`, compiled in | Rust crates | `builtin:` extensions |

axio follows Codex and Grok: plugins are bundles, code runs out of process.

## Core changes this needs

1. **Claude Code tool names and parameters.** Rename the built-in tools to
   `Read`, `Write`, `Edit`, `Bash` with `file_path`, `old_string`,
   `new_string`, `replace_all`, `command`, `timeout`. Hooks written for Claude
   Code, Codex and Grok then match axio's tools unchanged, and models already
   know these shapes. Sessions keep loading: names are only matched at call
   time.
2. **Settings in JSON.** `~/.axio/agent/settings.json` and `.axio/settings.json`
   (project, trusted only). JSON keeps the dependency list at serde, serde_json
   and ureq.
3. **Project trust.** `~/.axio/agent/trust.json` records trusted project roots.
   Project plugins, hooks, MCP servers and settings load only after the user
   answers a one-time prompt; `--trust` and `--no-project-config` override it.
4. **Append-only rule.** Hooks and plugins add context as new text blocks or
   messages. Nothing rewrites earlier history, because Anthropic binds thinking
   blocks to the exact prefix.

## Plugin layout

```
my-plugin/
  .claude-plugin/plugin.json   {"name", "version", "description"}
  hooks/hooks.json             hook declarations (below)
  .mcp.json                    {"mcpServers": {"name": {"command", "args", "env"}}}
  skills/<name>/SKILL.md       frontmatter: name, description
  commands/<name>.md           slash command templates
  bin/                         executables the hooks and servers run
```

Every part is optional. `${CLAUDE_PLUGIN_ROOT}` and `${AXIO_PLUGIN_ROOT}` expand
to the plugin folder in commands and arguments.

Discovery: `~/.axio/agent/plugins/*/` (global) and `.axio/plugins/*/` (project,
trusted only), plus paths listed under `"plugins"` in settings. A profile or
`"disabledPlugins"` turns individual plugins off.

Install: `axio plugin install <git-url | path>` clones or copies into
`~/.axio/agent/plugins/`; `axio plugin list | remove | update`.

## Hooks

Declared like Claude Code:

```json
{"hooks": {"PreToolUse": [{"matcher": "Bash|Edit",
  "hooks": [{"type": "command", "command": "${CLAUDE_PLUGIN_ROOT}/bin/guard", "timeout": 30}]}]}}
```

Hooks may also live in `hooks` inside settings files.

**Events in the first version**

| Event | When | Can |
|---|---|---|
| `SessionStart` | session opens or resumes | add context |
| `UserPromptSubmit` | before a prompt is sent | block it, add context |
| `PreToolUse` | before a tool runs | allow, deny, ask the user, rewrite the input |
| `PostToolUse` | after a tool returns | add context, flag the result to the model |
| `Stop` | the model is about to end its turn | block the stop with a reason, so the model keeps going (capped at 8) |
| `PreCompact` | before compaction | add instructions for the summary |
| `SessionEnd` | axio exits | observe only |

**Wire format** (same as Codex and Claude Code). axio writes one JSON object to
the hook's stdin: `session_id`, `transcript_path`, `cwd`, `hook_event_name`,
`model`, and the event's fields (`tool_name`, `tool_input`, `tool_use_id`,
`tool_response`, `prompt`, `source`). The hook answers by exit code and
optional JSON on stdout:

- exit 0, no output: carry on.
- exit 2: block; stderr goes to the model as the reason.
- JSON: `decision: "block"` with `reason`; `continue: false` with
  `stopReason`; `systemMessage` shown to the user;
  `hookSpecificOutput.additionalContext` appended for the model; for
  `PreToolUse`, `hookSpecificOutput.permissionDecision` (`allow`, `deny`,
  `ask`), `permissionDecisionReason` and `updatedInput`.

Hooks for one event run in parallel; any block wins. Timeout default 60
seconds, after which the hook is killed and ignored. `ask` prompts in the
terminal; in `-p` mode it denies. Environment: `AXIO_PROJECT_DIR`,
`CLAUDE_PROJECT_DIR`, plugin root variables.

## MCP servers (tools from other programs)

- stdio transport only: axio starts the server, speaks JSON-RPC 2.0 over
  stdin/stdout (`initialize`, `tools/list`, `tools/call`, `notifications/*`).
- Tools appear to the model as `mcp__<server>__<tool>` with the server's JSON
  schema; `PreToolUse` hooks see the same names.
- Text and error results in the first version; images and resources later.
- Servers start with the session and stop on exit; a crashed server reports
  errors for its tools and restarts on the next call.
- Config from plugins' `.mcp.json`, `~/.axio/agent/mcp.json` and the trusted
  project's `.mcp.json`.

## Skills and commands

- **Skills:** each `SKILL.md`'s name and description are listed in the system
  prompt with its path. The model reads the file with `Read` when a task
  matches, so unused skills cost one line each.
- **Commands:** `/name args` loads `commands/name.md`, replaces `$ARGUMENTS`,
  and sends the result as the prompt. Plugin commands are also reachable as
  `/plugin:name`.

## Modules

Built into axio, switched on by profiles:

| Module | Adds | Cargo feature |
|---|---|---|
| `search` | `Grep` and `Glob` tools (ignore-aware walk, regex) | none, unless the walk needs a crate |
| `background` | `Bash` with `run_in_background`, `TaskOutput`, `KillTask` | none |
| `plan` | `TodoWrite` plan tool shown in the terminal | none |
| `web` | `WebFetch` (HTTP GET, HTML to text) | none (ureq) |
| `sandbox` | macOS Seatbelt and Linux bwrap wrappers for `Bash` | none (external binaries) |

MCP, hooks, skills and commands are core, not modules: they are small and
every profile benefits.

## Profiles

`~/.axio/agent/profiles/<name>.json`, shipped defaults built in:

```json
{"model": "anthropic/claude-opus-5-5", "effort": null,
 "modules": ["search", "plan"], "plugins": ["*"],
 "compactAt": 0.85, "prompt": ["skills"], "permissions": "allow"}
```

| Profile | Modules | Notes |
|---|---|---|
| `minimal` | none | Pi-sized: four tools, plugins on, no permission prompts |
| `standard` | search, plan, background | the default |
| `full` | all, sandbox on, `ask` for destructive Bash | for unattended or unfamiliar repos |

Choose with `axio --profile <name>`, `"profile"` in settings, or `/profile`.
`axio init --profile <name>` writes settings for a new install.

## Order of work

1. Claude Code tool names, JSON settings, project trust.
2. Hooks (all seven events) with tests driving small shell hooks, plus a
   permission-gate example plugin.
3. MCP stdio client, tested against a stub server and one real server.
4. Skills and commands.
5. Profiles and the `search`, `plan` and `background` modules.
6. `axio plugin install | list | remove | update`.

Each step is its own PR with tests; the core stays dependency-free.

## Questions for the owner

1. Rename the built-in tools to Claude Code's names (recommended), or keep the
   lowercase ones and map names only for hooks?
2. Import hooks, plugins and MCP servers from `~/.claude` automatically (Grok
   does), behind a setting, or not at all?
3. Default profile: `standard` (recommended) or `minimal`?
