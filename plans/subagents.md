# Sub-agents

Status: approved 2026-11-12; being built in the order below. Owner decisions
so far are at the end.

## Goal

Let axio hand self-contained pieces of work to sub-agents, each one the
smallest axio needed for its task: only the tools, instructions, model and
context the task needs. Less in each request means faster and cheaper answers,
and less running means a smaller footprint.

## Why axio can do this cheaply

A sub-agent is another `axio` process. Measured on 2026-11-12 (Apple
silicon, same ChatGPT model and account):

| | axio | Pi (Node.js), add-ons off |
|---|---|---|
| Start | 1.8 ms, 1.9 MB | 117 ms, 114 MB |
| One prompt, peak memory | 12 MB | 157 MB |

Node-based harnesses keep sub-agents in one process because each extra process
costs about 115 MB. axio can give each sub-agent its own process: the
operating system shares the binary's code between copies, so a sub-agent costs
its own heap (about 10 MB during a request). A separate process also means a
crash stays in that sub-agent, Esc can stop a whole group at once, and each
one can have its own sandbox.

The same measurements show the model's reply time dominates a prompt. The
speed gain comes from smaller requests (fewer tool definitions, a shorter
system prompt, less history, a faster model), not from process start-up.

## How it works

- The main agent gets an `Agent` tool: a role, a task brief, and optionally a
  model or effort. A brief must stand on its own.
- axio starts `axio agent` (a hidden mode of the same binary) with a JSON task
  spec on stdin. The child streams JSON lines on stdout: progress (tool calls)
  and a final result. The parent shows progress as one line per sub-agent.
- The child returns a summary only, capped in size (a setting, 8k characters
  by default). Its full transcript is saved as its own session, linked from
  the parent's, so it can be opened but never enters the parent's context.
- Several sub-agents can run at once, up to a setting (4 by default).
- Esc stops the turn and the whole process group of sub-agents.
- Sub-agents emit `SubagentStart` and `SubagentStop` hook events. axio skips
  these events in imported Codex hooks today; they would start to work.

## The smallest axio for the task

The parent decides, from the role and the brief:

- **Tools**: only the role's list. An explorer gets Read, Grep and Glob; no
  Write, Bash or web. Profiles and modules already select tools, so this is a
  list passed to the child.
- **Instructions**: only the role's prompt and the tool instructions it uses;
  no skills list, plugin text or command list unless the role asks for them.
- **Model and effort**: chosen as below; a fast model at low effort when
  nothing says otherwise (`fastModel` in settings, for example
  `anthropic/claude-haiku-4-5` or `chatgpt/gpt-6-luna`).
- **MCP servers**: none started in the child. A role that names an MCP tool
  gets it through the parent, which already has the server running and runs
  the call for the child over the pipe. Servers then start once, not once per
  sub-agent.
- **Hooks, plugins, imports, session picker**: off in the child.
- **Sandbox**: never looser than the parent's; a read-only role gets a
  read-only sandbox.
- **Sign-in**: children read the same `auth.json`. The ChatGPT refresh lock
  (#47) already lets many processes refresh one sign-in safely.

## Model and effort

Model and effort are set separately, and each can come from one of four
places, all of which the user controls:

| Value | Meaning |
|---|---|
| `inherit` | the main agent's current model (or effort) |
| `choose` | the main agent picks per call, from `subagents.choices` when set |
| a model or level | fixed by the role file, for example `model: fast` or `effort: high` |
| settings | `subagents.roles.<role>.model` / `.effort` override the role file |

Settings win over the role file; a role's `choose` lets the main agent pick,
otherwise the main agent cannot override. `fast` resolves to `fastModel`.

The same pickers as the main session (model list with an effort row, the
effort slider) set a role's model and effort, with `inherit` and `choose`
listed first, so no one has to edit JSON.

## What a sub-agent sees

Set per role, overridable in settings and per call:

- `brief` (default): the task brief, the working directory and the role's
  instructions. Cheapest, and it makes the parent write a clear task.
- `rules`: the brief plus AGENTS.md / CLAUDE.md. For roles that change files
  and must follow project conventions.
- `fork`: a copy of the parent's conversation. Costly in general, though on
  Anthropic the shared prefix is already cached, so a fork mostly pays for
  the new turns. Opt-in only.

## Roles

Roles are markdown files, not code, in Claude Code's agent format, so agents
written for Claude Code work unchanged and `axio import` can offer them:

```markdown
---
name: explore
description: Finds and explains code; read-only.
tools: Read, Grep, Glob
model: fast            # fast | inherit | choose | provider/model
effort: low            # inherit | choose | a level
maxTurns: 20
context: brief         # axio: brief | rules | fork
sandbox: read-only     # axio: read-only | project
---
You search the code base and answer the question in the brief…
```

Claude Code fields axio does not use are ignored. Files live in
`~/.axio/agent/agents/`, `.axio/agents/` (trusted projects) and plugins'
`agents/` folders; a project's role overrides a global one of the same name.

axio ships four, which users can edit or replace:

| Role | Tools | Model | Sees |
|---|---|---|---|
| `explore` | Read, Grep, Glob | fast, low | brief |
| `research` | WebFetch, Read | fast, low | brief |
| `review` | Read, Grep, Glob, Bash (read-only sandbox) | inherit, high | rules |
| `code` | Read, Write, Edit, Bash, Grep, Glob | inherit | rules |

Anything else (documenter, auditor for a specific standard, test writer) is a
user file; a documenter is `code` with different instructions and does not
need to ship.

## Settings

```json
{
  "subagents": {
    "nesting": 0,
    "parallel": 4,
    "fastModel": "anthropic/claude-haiku-4-5",
    "choices": ["anthropic/claude-haiku-4-5", "chatgpt/gpt-6-luna"],
    "resultLimit": 8000,
    "roles": {
      "explore": { "model": "chatgpt/gpt-6-luna", "effort": "low" },
      "review": { "model": "inherit", "effort": "choose" },
      "code": { "context": "brief" }
    }
  }
}
```

- `nesting`: how many levels below the main agent may start sub-agents. 0 by
  default: sub-agents cannot start their own.
- `roles.<name>`: overrides any field of a role file without editing it.

## Beyond sub-agents

The same child mode can run axio's own side jobs on a small model with a
clean context: compaction summaries, summarising a long web page or tool
output before it enters the conversation, and background tasks.

## Order of work

1. `axio agent` child mode: JSON spec in, JSON lines out, summary capped.
   Built: the spec (`task`, `tools`, `instructions`, `cwd`, `model`,
   `effort`, `context`, `maxTurns`, `resultLimit`, `sessionDir`; unknown
   fields are refused) and the lines `start`, `tool`, `tool_end`, `notice`,
   then `result` or `error`. Tools not in the list are refused even when
   called. After `maxTurns` rounds of tools (50 by default) the model gets
   one reply to answer without them. Sessions go to an `agents` folder that
   `-c` and `-r` skip. Measured against a local stub server: 3.8 MB peak,
   about 4 ms to the first request, 644 system-prompt characters for a
   brief role.
2. Role files and the four shipped roles; `fast` model resolution.
3. The `Agent` tool in the parent, progress lines, Esc stopping the group.
4. Settings: nesting, parallel, result limit, per-role overrides.
   Steps 2-4 built together. Decisions taken while building: `review` keeps
   Bash in a read-only sandbox; progress from several sub-agents shares the
   one spinner line, with a line per sub-agent as each finishes. The tool
   takes Claude Code's input (`description`, `prompt`, `subagent_type`) plus
   `model`, `effort` and `context`. `fork` sends a text transcript of the
   conversation rather than the raw history, because thinking blocks are bound
   to the model that wrote them. An attached child stops its turn when its
   input closes, which is how Esc reaches its own commands. The `agents`
   module carries the tool; `minimal` leaves it out.
5. MCP calls proxied through the parent.
   Built: the spec carries the granted MCP tools' definitions; the child asks
   with an `mcp_call` line and reads an `mcp_result` line on stdin; the
   parent runs each call on its own thread against its running server and
   refuses tools the child was not granted.
6. `SubagentStart` / `SubagentStop` hooks; `axio import` offers Claude Code
   agents.
7. Side jobs (compaction, long-output summaries) moved onto child mode.

Each step lands with a failing test first and is measured: child memory,
start-to-first-request time, and system-prompt tokens per role.

## Open questions

Both settled while building steps 2-4 (see the order of work above).

## Owner decisions (2026-11-12)

- Sub-agents cannot start sub-agents by default; a setting allows it.
- What a sub-agent sees is set per role (what the sub-agent does) and can be
  changed in settings.
- Built-in roles are wanted; this draft proposes four shipped as files.
- A sub-agent's model, and separately its effort, can be the main agent's
  (`inherit`), picked by the main agent (`choose`), fixed by the role, or set
  in settings; the user can change each.
