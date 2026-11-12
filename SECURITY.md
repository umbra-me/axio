# Security

## Reporting a vulnerability

Please report security problems privately through GitHub: on
[umbra-me/axio](https://github.com/umbra-me/axio), open **Security →
Report a vulnerability**. Do not open a public issue for them.

Include what you ran, what happened and what you expected, and the axio
version (`axio --version`) and platform.

## Supported versions

axio is in public beta (`0.x`). Fixes go into the latest release only;
`axio update` installs it.

## Scope

axio runs the commands and edits that a language model asks for, as your
user, within the permission mode you choose. The model doing what you
allowed it to do is not a vulnerability. These are:

- a command or edit that runs although the permission mode or a deny rule
  should have stopped it;
- a project's configuration taking effect before you trusted that project;
- escaping the `sandbox` module, or a read-only sub-agent changing files;
- keys, sessions or logs readable by other users of the machine;
- `axio update` installing a binary whose SHA-256 does not match the release.

## Known limits

These are not treated as vulnerabilities:

- Bash deny rules see the command line, not what a program does. A script, an
  interpreter (`python -c`), an alias or a function defined elsewhere can run
  a denied command. Lines axio cannot read with certainty ask first; see
  Permissions in the README.
- Path rules are checked before a file is written, so another process that
  swaps a symlink between the check and the write can redirect it.
- On Windows there is no sandbox module, and hard links are not detected.
