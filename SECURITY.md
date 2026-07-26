# Security

## What axio does on your machine

axio executes code written by a language model against your working directory.
Read that sentence again before running it anywhere that matters.

Concretely, in the shipped design:

- File reads and writes are confined to the workspace root. Every path goes
  through one constructor, which rejects `..`, absolute paths, UNC and
  drive-relative spellings lexically, then canonicalises to close the symlink
  escape. Reads may additionally reach the spill directory, where axio parks
  tool output too large to send; nothing else outside the root is reachable, and
  writes have no such exception.
- Writes, edits and shell commands require approval. Reads, globs and greps do
  not.
- A built-in deny list is evaluated **before** read-only auto-approval, so a
  rule cannot accidentally expose `.env`, private keys, `~/.ssh`, `~/.aws`, or
  axio's own credential store. It is not overridable by an allow rule or by
  `--yes`.
- The deny list is tested against a shell command's arguments as well as against
  the file tools' paths, so `bash cat .env` is refused the same way `read .env`
  is. **This holds only for a path the command states plainly.** A shell can
  compute one — `cat $(echo .env)`, `cat .e''nv`, a script that reads it — and
  nothing short of intercepting the syscalls would see that. Treat the deny list
  as a guard against accident, not against a determined agent with a shell.
- The credential is stripped from the environment of any child process axio
  spawns.

## What axio does not do

**There is no sandbox.** Confinement is the workspace root, the approval prompt
and process-group containment — not an OS-enforced boundary. A shell command you
approve can do anything your user account can do.

`--yes` disables the approval prompt. Combined with the shell tool it gives a
model unattended access to your account. It exists because automation needs it;
it is not a mode to leave on.

Environment stripping is a speed bump, not a boundary: under `--yes` the agent
has your shell, and anything your shell can read it can read.

The first unattended run against untrusted input is the tripwire for adding
Linux Landlock support, at which point this document will say "sandboxed on
Linux only" and nothing stronger.

## Reporting a vulnerability

Open a GitHub security advisory on this repository rather than a public issue.
Include the version (`axio --version`), your platform, and a reproduction if you
have one. Expect an acknowledgement within a few days.

Please do report: a path that escapes the workspace root, an approval that can
be bypassed, a credential reaching a log, a child process, or the model's
context, or a deny-list rule that can be overridden.
