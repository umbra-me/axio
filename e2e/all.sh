#!/bin/sh
# Terminal checks: drives one axio binary in pseudo-terminals against local
# stub servers (no model is called) and prints failures and a count.
#   sh e2e/all.sh target/release/axio
# The binary must be named axio: axio-<name> would select a profile.
set -u
case $1 in /*) B=$1 ;; *) B=$(pwd)/$1 ;; esac
D=$(cd "$(dirname "$0")" && pwd)
P=$(cd "$D/.." && pwd)/examples/plugins
# Resolved, because axio's working directory is (macOS: /var is /private/var).
T=$(cd "$(mktemp -d "${TMPDIR:-/tmp}/axio-e2e.XXXXXX")" && pwd -P)
OUT=$T/results.txt
h() { mktemp -d "$T/h.XXXXXX"; }
{
  timeout 90 python3 "$D/pty_check.py" "$B" "$(h)"
  timeout 120 python3 "$D/ask_check.py" "$B" "$(h)" "$P"
  timeout 60 python3 "$D/progress.py" "$B" "$(h)" | grep -v 'fast toggled\|/model shows both'
  timeout 60 python3 "$D/firstrun.py" "$B" "$(h)"
  timeout 30 python3 "$D/tab.py" "$B" "$(h)"
  timeout 40 python3 "$D/pickers.py" "$B" "$(h)"
  timeout 60 python3 "$D/r4.py" "$B" "$(h)"
  timeout 120 python3 "$D/modules_check.py" "$B" "$(h)" | grep -q "minimal tools: \['Read', 'Write', 'Edit', 'Bash'\]" && echo "PASS modules" || echo "FAIL modules"
  timeout 60 python3 "$D/retry.py" "$B" "$(h)"
  timeout 60 python3 "$D/liveout.py" "$B" "$(h)"
  timeout 60 python3 "$D/undo.py" "$B" "$(h)"
  timeout 60 python3 "$D/resume.py" "$B" "$(h)"
  timeout 60 python3 "$D/resize.py" "$B" "$(h)"
  timeout 60 python3 "$D/mention.py" "$B" "$(h)"
  timeout 60 python3 "$D/compact.py" "$B" "$(h)"
  timeout 60 python3 "$D/pick.py" "$B" "$(h)"
  timeout 60 python3 "$D/output.py" "$B" "$(h)"
  timeout 60 python3 "$D/steer.py" "$B" "$(h)"
  timeout 60 python3 "$D/steer_color.py" "$B" "$(h)"
  timeout 60 python3 "$D/fkey.py" "$B" "$(h)" | grep F5
  timeout 60 python3 "$D/stdin_wait.py" "$B" "$(h)"
  timeout 60 python3 "$D/hook_banner.py" "$B" "$(h)"
  timeout 90 python3 "$D/agents_tui.py" "$B" "$(h)"
  timeout 60 python3 "$D/fast_mode.py" "$B" "$(h)"
  timeout 90 python3 "$D/panels.py" "$B" "$(h)"
  timeout 60 python3 "$D/status_line.py" "$B" "$(h)"
  timeout 60 python3 "$D/settings_context.py" "$B" "$(h)"
  timeout 60 python3 "$D/rewind_history.py" "$B" "$(h)"
} 2>&1 | grep -E '^(PASS|FAIL)' > "$OUT"
grep FAIL "$OUT"; echo "$(grep -c PASS "$OUT") passed, $(grep -c FAIL "$OUT") failed"
