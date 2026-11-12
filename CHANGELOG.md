# Changelog

Notable changes to axio. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); numbers are pull
requests in `umbra-me/axio`. Versions follow
[Semantic Versioning](https://semver.org); see `docs/releasing.md`.

## Unreleased

## [0.1.1] - 2026-11-13

### Fixed

- On Linux without a working bubblewrap (not installed, or blocked from
  making namespaces as on Ubuntu 24.04), the read-only sub-agent roles
  (`explore`, `review`, `research`) ran no work and ended at once. They now
  fall back to permission rules, as on Windows: file changes are refused and
  every command asks (#90).

## [0.1.0] - 2026-11-13

The first public beta. Everything below was built before the first release.

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

- Output speed (tok/s) and time to first token after each turn and in
  `/status` (#73).

- Each status line item can be switched on or off in `/settings` or
  `statusLine` (#74).

- Permission modes as in Claude Code: `default` asks before commands, file
  changes, fetches and MCP tools; `acceptEdits`, `plan` and
  `bypassPermissions`; Shift+Tab cycles them; `permissions.allow`/`deny`
  rules in Claude Code's syntax; answers saved per project in the user's own
  folder; sub-agents ask through the main terminal (#76).

**Models**
- Fast mode as an opt-in toggle on models that have it: `/fast`,
  `"fastMode"` in settings, the price shown when it turns on, standard speed
  when the provider refuses it (#66).

**Updates**
- `axio update` installs the latest GitHub release for this platform after
  checking its SHA-256 against the release's `SHA256SUMS`; `--check` only
  reports whether one is available.

**Project**
- Licensed under Apache-2.0 (#67).
- Windows (experimental): builds with the MSVC toolchain and runs in Windows
  Terminal or the console; Bash and hooks run in Git Bash, and a stopped
  command's whole process tree ends. Without a sandbox, read-only sub-agent roles are
  kept read-only by permission rules: file changes are refused and commands
  ask first.

**Testing**
- Terminal checks in `e2e/` (`sh e2e/all.sh target/release/axio`) and a Linux
  container run (`sh e2e/linux.sh`) (#59).
- GitHub Actions: fmt, clippy and tests on Linux, macOS and Windows, and the
  terminal checks on Linux and macOS, for every pull request.

**Releases**
- README status (public beta), a security and privacy section, and
  `SECURITY.md` for private vulnerability reports.
- Pushing a `vX.Y.Z` tag publishes static binaries for macOS, Linux and
  Windows with `SHA256SUMS` and the changelog section; `docs/releasing.md`
  describes versions and the steps.

### Changed
- **Breaking:** tools renamed to Claude Code's names, and configuration moved
  to `settings.json` with project trust (#4).
- **Breaking:** axio now asks before running commands and changing files;
  `--permission-mode bypassPermissions` (or `defaultMode`) restores the old
  behaviour (#76).
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
- Session transcripts, prompt history and tool output logs are private to
  the user: `~/.axio/agent` is kept at 0700, new files are 0600, and output
  logs moved from a shared `/tmp/axio` to `~/.axio/agent/logs` (#75).
- Up and Down keep walking history past `/command` entries instead of
  stopping in the command menu (#69).

### Security

Found by a pre-release review.

- A prefix allow rule such as `Bash(cat:*)` no longer approves commands with
  process substitution, `${…}`, input redirection or here-documents; they ask.
  Deny rules also see commands nested in `$(…)`, `<(…)`, backticks,
  brackets, quotes and after `NAME=value`.
- Path rules for Read, Edit and Write judge where a symlink really leads: an
  allow rule must hold for both the written and the real path, and a deny
  rule refuses either.
- The sandbox no longer lets Write or Edit escape through a missing folder
  followed by `..`.
- Read-only sub-agents without a sandbox (Windows) ask before every command,
  even ones the main agent's rules allow.
- Requests to model providers, MCP servers and sign-in endpoints no longer
  follow redirects, which carried headers such as `x-api-key` to the server a
  redirect named.
- WebFetch follows redirects only on the same site (or from http to https);
  a redirect elsewhere comes back as a new address for the model to fetch,
  which needs its own permission.
- Hooks leave `${CLAUDE_PLUGIN_ROOT}`, `${AXIO_PROJECT_DIR}` and the like to
  the shell instead of pasting the path into the command, so shell syntax in
  a folder name cannot run; installed plugin names are limited to letters,
  digits, `-`, `_` and `.`.
- Text from the model, commands, MCP servers, resumed sessions and error
  bodies is cleaned of terminal sequences before it is shown, so it cannot
  write the clipboard (OSC 52), add hidden links, clear the screen or move
  the cursor.
- Bash deny rules split a command line into words and simple commands:
  quoted words stay whole, redirections and their targets are set aside,
  `NAME=value` assignments and wrappers (`command`, `env`, `sudo`, `exec`,
  `nohup`, `time`, `nice`) are skipped, `/bin/rm` counts as `rm`, and commands
  inside `"$(…)"` are seen. While a Bash deny rule is set, bypass mode runs
  only plain shell without asking (words, quotes, `$NAME`, `;`, `&&`, `||`,
  `|`, simple redirections); anything else, a program named by an expansion,
  a wrapper with options, or a command that runs others (`bash -c`, `eval`,
  `xargs`, `find -exec`, `trap`, `declare`) asks first, as does an unquoted
  expansion that bash could split into more words or options (`env FOO=$X`,
  `printf $OP`). A randomized test runs thousands of allowed lines through
  the real bash with a stand-in `rm` and fails if one runs it. They remain a
  guard against mistakes rather than a boundary; `SECURITY.md` lists the
  limits.
- Path deny rules also refuse a symlink whose target has a denied name, even
  outside the project, a symlink to nothing yet, and any file with more than
  one hard link.
- WebFetch compares a redirect's port with the scheme's default, and resolves
  query-only and fragment-only redirects against the current page.
