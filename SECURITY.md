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
- Every provider's credential variable — and everything named `AXIO_*` — is
  stripped from the environment of any child process axio spawns. The list is
  derived from the providers themselves, so adding one cannot leave its key
  behind.

## What axio does not do

**There is no sandbox by default.** Confinement is the workspace root, the
approval prompt and process-group containment — not an OS-enforced boundary.
`--sandbox` adds one on Linux; see below. Without it, a shell command you
approve can do anything your user account can do.

`--yes` disables the approval prompt. Combined with the shell tool it gives a
model unattended access to your account. It exists because automation needs it;
it is not a mode to leave on.

Environment stripping is a speed bump, not a boundary: without the sandbox,
under `--yes` the agent has your shell, and anything your shell can read it can
read.

## The sandbox

`--sandbox`, or `[sandbox] enabled = true`. **Linux only**, using Landlock —
the kernel's own unprivileged sandbox. Nothing to install, no container, no
setuid helper.

It is an allow-list over paths, applied to axio before its runtime starts and
inherited by every command it spawns:

| | |
| --- | --- |
| read, write, execute | the workspace, axio's state directory, `/dev` |
| read, execute | `/usr` `/bin` `/sbin` `/lib` `/lib64` `/opt` `/etc` `/proc` `/sys` `/nix` `/snap`, axio's own config directory, and `~/.gitconfig` `~/.config/git` `~/.cargo` `~/.rustup` |
| nothing | everything else, including `$HOME` |

A command that goes looking for `~/.ssh/id_rsa` gets `EACCES` from the kernel
rather than a refusal from a policy engine, and `TMPDIR` points inside the state
directory so the shared `/tmp` need not be opened up.

A granted path that is a regular file rather than a directory gets the
file-applicable subset of the same rights — so `~/.gitconfig`, and anything you
list under `read`, is readable and not writable. That distinction is load-
bearing: a writable git config is `core.hooksPath` pointing wherever it likes.

Add what your toolchain needs:

```toml
[sandbox]
enabled = true
read = ["/home/me/.pyenv"]
write = ["/home/me/.cache/go-build"]
```

**What it does not do.**

- **Nothing about the network.** A sandboxed command can still open a socket and
  send the workspace anywhere. This stops reading and writing, not exfiltration.
- **Nothing on macOS or Windows.** Asking for it there is a warning, never a
  silent no-op — but it is a warning, so an unattended run on the wrong platform
  is unconfined unless you check.
- **It does not protect `auth.json`.** axio's config directory is readable
  because the credential is read after the sandbox is applied. The built-in deny
  list is what refuses it to the tools, including to a shell command's
  arguments.
- **It cannot tell axio's reads from a command's**, because it restricts the one
  process. It is a second wall behind the permission engine, not a replacement.

If the kernel enforces only part of the ruleset, axio says so and tells you to
treat it as no confinement. It never reports a sandbox it does not have.


## Reporting a vulnerability

Open a GitHub security advisory on this repository rather than a public issue.
Include the version (`axio --version`), your platform, and a reproduction if you
have one. Expect an acknowledgement within a few days.

Please do report: a path that escapes the workspace root, an approval that can
be bypassed, a credential reaching a log, a child process, or the model's
context, or a deny-list rule that can be overridden.
