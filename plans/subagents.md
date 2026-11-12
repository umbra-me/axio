# Sub-agents

Status: draft for later, not scheduled. Owner decisions so far are at the end.

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
- **Model and effort**: a fast model at low effort unless the role or brief
  says otherwise (`fastModel` in settings, for example
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
model: fast            # fast | inherit | provider/model
effort: low
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
    "resultLimit": 8000,
    "roles": {
      "explore": { "model": "chatgpt/gpt-6-luna" },
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
2. Role files and the four shipped roles; `fast` model resolution.
3. The `Agent` tool in the parent, progress lines, Esc stopping the group.
4. Settings: nesting, parallel, result limit, per-role overrides.
5. MCP calls proxied through the parent.
6. `SubagentStart` / `SubagentStop` hooks; `axio import` offers Claude Code
   agents.
7. Side jobs (compaction, long-output summaries) moved onto child mode.

Each step lands with a failing test first and is measured: child memory,
start-to-first-request time, and system-prompt tokens per role.

## Open questions

- Whether the parent may pick a role's model per call, or only settings may.
- Whether `review` should be allowed Bash at all, or only read tools.
- How progress from several parallel sub-agents is shown without crowding the
  terminal.

## Owner decisions (2026-11-12)

- Sub-agents cannot start sub-agents by default; a setting allows it.
- What a sub-agent sees is set per role (what the sub-agent does) and can be
  changed in settings.
- Built-in roles are wanted; this draft proposes four shipped as files.
