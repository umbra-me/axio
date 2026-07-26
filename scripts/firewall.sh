#!/usr/bin/env bash
# Naming firewall.
#
# Design notes and provenance may name other projects; the repository itself
# must not. This greps the tracked tree for a word-boundary-anchored list of
# names, plus a few literals that must never be vendored from anywhere.
#
# This script is excluded from its own scan for the obvious reason: it has to
# contain the strings it looks for. That is the one file to read by eye.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

# Word-boundary anchored so ordinary English is not caught. Extend deliberately.
NAMES='opencode|kilocode|kilo[ -]code|earendil|oh-my-pi|terax|t3code|cmux|odysseus'

# Literals that indicate code lifted from somewhere it should not have been.
# Their presence is a licensing or impersonation problem, not a naming one.
LITERALS='claude-cli/|You are Claude Code|Stealth mode'

status=0

scan() {
  local pattern="$1" label="$2"
  # `git grep` exits 1 when there are no matches, which is the success case
  # here — so capture output and test that, never the exit status.
  local hits
  hits=$(git grep -nIE "$pattern" -- . ':!scripts/firewall.sh' || true)
  if [ -n "$hits" ]; then
    echo "FAIL: $label" >&2
    echo "$hits" >&2
    status=1
  fi
}

scan "\\b($NAMES)\\b" "reference-project name in a tracked file"
scan "($LITERALS)" "literal that must never be vendored"

if [ "$status" -eq 0 ]; then
  echo "firewall: clean"
fi
exit "$status"
