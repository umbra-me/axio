# Changelog

Notable changes to axio. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); numbers are pull
requests in `umbra-me/axio`. axio has not been released yet, so everything is
under Unreleased.

## Unreleased

### Added

**Core** (started 2026-11-11)
- One Rust binary with Read, Write, Edit and Bash; Anthropic and
  OpenAI-compatible providers; JSONL sessions in `~/.axio/agent/sessions`;
  AGENTS.md (or CLAUDE.md) discovery.
- Prompt editor, Esc or Ctrl-C to stop a turn, and compaction near the end of
  the context window (#1). Codex-style compaction (recent requests plus a
  summary), head-and-tail Bash output with a log file, SIGTERM then SIGKILL,
  `replace_all` for Edit (#2).
- Retries on overload, rate limits and dropped connections (#23).
- `/undo` puts back the files the last turn wrote or edited (#26).
- `@path` attaches files and folders, with Tab completion (#31); images by
  `@path` or by dropping them on the terminal (#33); Read shows images to the
  model and refuses binary files (#34).
- `axio -r` picks a recent session (#35); `/output` shows the last tool call's
  full result (#36); typing during a turn steers it at the next step (#37).

**Configuration and extensions**
- Claude Code tool names, `settings.json` and project trust (#4).
- Hooks in the Claude Code format: SessionStart, UserPromptSubmit,
  PreToolUse, PostToolUse, Stop, PreCompact and SessionEnd (#5).
- MCP: stdio servers (#6), streamable HTTP with headers and `${VAR}` (#13),
  OAuth sign-in with `axio mcp login | list | logout` (#14), the older
  HTTP+SSE transport (#61), images and resources passed to the model (#60), and
  each server's own instructions in the prompt (#63).
- Skills and slash commands in the Claude Code layout (#7).
- Profiles (`minimal`, `standard`, `full`) and the `search`, `plan` and
  `background` modules (#8); `web` and `sandbox` modules (#12); `axio-<profile>`
  and `ax` command names and `axio profile <name>` (#16).
- `axio plugin install | list | update | remove` (#9).
- `axio import` offers hooks, plugins, MCP servers and skills from Claude Code,
  Codex, Grok Build and Pi (#10), opt-in only (#17).
- `~/.axio/agent/auth.json` for provider keys and MCP tokens, with
  `axio auth` (#15).

**Terminal**
- Streamed markdown, a spinner with elapsed time, compact tool display and a
  first-run key prompt (#18); tool-call progress (#19); thinking summaries in
  the spinner and Tab completion of commands (#21); numbered edit diffs, Ctrl-R
  history search and an exit summary (#22); a running command's newest output
  line (#24); line diffs and calmer failed commands (#25); a recap of where a
  resumed session left off (#27); markdown tables (#30); a line saying what
  compaction did (#32).
- A command menu while typing `/`, and pickers for model, provider and login
  (#44); Sign in with ChatGPT (#45, #46); effort in the model picker and an
  effort slider (#49); browsing models by provider in `/model` (#50). Enter in
  a picker sets the default; Ctrl-S applies to this session only.

**Sub-agents** (`plans/subagents.md`)
- `axio agent`: a sub-agent's own process, with a JSON task spec in and JSON
  lines out (#53).
- The `Agent` tool and role files in Claude Code's agent format, with four
  built-in roles (`explore`, `research`, `review`, `code`); several sub-agents
  run at once and Esc stops them all; `subagents` settings; `axio agents`
  lists the roles (#55).
- Sub-agents use the main agent's MCP servers (#56).
- `SubagentStart` and `SubagentStop` hooks; `axio import` offers Claude Code's
  `~/.claude/agents` (#57).
- `compactModel`, and an optional `prompt` for WebFetch answered by the fast
  model (#58).

**Terminal panels**
- `/mcp`, `/skills`, `/agents`, `/hooks`, `/plugins` and `/status`, and
  `/help` grouped into sections (#68).

- A status line under the prompt with the model, context use and an
  estimated session cost; Ctrl-O shows the last tool result in full; long
  edit diffs are cut to their start (#70).

- `/context`, `/settings` and `/init` (#71).

- Rewind with Esc twice or `/rewind`, optionally putting back files changed
  since, and `/history` to pick a past prompt (#72).

**Models**
- Fast mode as an opt-in toggle on models that have it: `/fast`,
  `"fastMode"` in settings, the price shown when it turns on, standard speed
  when the provider refuses it (#66).

**Project**
- Licensed under Apache-2.0 (#67).

**Testing**
- Terminal checks in `e2e/` (`sh e2e/all.sh target/release/axio`) and a Linux
  container run (`sh e2e/linux.sh`) (#59).

### Changed
- **Breaking:** tools renamed to Claude Code's names, and configuration moved
  to `settings.json` with project trust (#4).
- `/provider` removed now that `/model` browses providers (#51).
- Built-in sub-agent roles run on the main agent's model and effort by
  default; a smaller model is opt-in per role (#64).

### Removed
- Fast mode, added in #19, was removed in #20 (back as an opt-in toggle in
  #66).

### Fixed
- Resumed sessions compact correctly, and refused tool calls are shown (#11).
- The spinner follows terminal resizes (#28).
- Crashes, wrong diff anchors and stalls found in review (#29); stray keys no
  longer stop turns, and other review fixes (#39).
- Queued lines after a command's output (#38).
- The newest prompt is shown after compaction, and the system prompt counts in
  the estimate (#40); "1 message", not "1 messages" (#41).
- Problems found testing every feature live (#42); quieter imported items and
  a clear message when a server refuses sign-in (#43).
- The ChatGPT sign-in refreshes once across sessions (#47).
- Sub-agents read keys saved in `auth.json` (#54).
- A read-only sub-agent cannot change a project that lives in a temporary
  folder (#62).
- Up and Down keep walking history past `/command` entries instead of
  stopping in the command menu (#69).
