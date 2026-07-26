#!/usr/bin/env bash
# Structural limits, reported as numbers on every run.
#
# These exist because a rule written in a markdown file is not an enforcement
# mechanism. Raising any of them is allowed — in a commit whose message says
# why.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

MAX_MEMBERS=4
MAX_TOOLCX_FIELDS=5
MAX_LOC=10000

status=0

# --- workspace members -------------------------------------------------------
members=$(awk '/^members = \[/{f=1;next} /^\]/{f=0} f' Cargo.toml | grep -c '"' || true)
echo "workspace members: $members / $MAX_MEMBERS"
if [ "$members" -gt "$MAX_MEMBERS" ]; then
  # A fifth member is allowed, but only with a recorded dependency-isolation
  # reason — the split rule from the plan.
  if ! grep -q "^### Workspace member justification" AGENTS.md; then
    echo "FAIL: $members workspace members but no '### Workspace member justification' section in AGENTS.md" >&2
    status=1
  fi
fi

# --- ToolCx field count ------------------------------------------------------
# The archive accumulated twelve Option<Arc<dyn ...>> host bridges and 13 of 41
# tools became desktop-only by construction. This is the gate against that.
toolcx=$(awk '/^pub struct ToolCx \{/{f=1;next} /^\}/{f=0} f' crates/axio-core/src/tool.rs \
  | grep -cE '^\s*pub [a-z_]+:' || true)
echo "ToolCx fields: $toolcx / $MAX_TOOLCX_FIELDS"
if [ "$toolcx" -gt "$MAX_TOOLCX_FIELDS" ]; then
  echo "FAIL: ToolCx has $toolcx fields (max $MAX_TOOLCX_FIELDS)" >&2
  status=1
fi
if awk '/^pub struct ToolCx \{/{f=1;next} /^\}/{f=0} f' crates/axio-core/src/tool.rs \
  | grep -qE 'Option<Arc<dyn'; then
  echo "FAIL: ToolCx gained an Option<Arc<dyn ...>> host bridge" >&2
  status=1
fi

# --- workspace Rust LOC, excluding tests ------------------------------------
# Heuristic: everything from a file's first `#[cfg(test)]` to its end is a test
# module, which is where they live by convention here. Files under tests/ are
# excluded outright.
loc=0
while IFS= read -r f; do
  n=$(awk '/#\[cfg\(test\)\]/{exit} {print}' "$f" | grep -cvE '^\s*(//|$)' || true)
  loc=$((loc + n))
done < <(git ls-files 'crates/*/src/*.rs' 'crates/*/src/**/*.rs')
echo "workspace Rust LOC (excl. tests): $loc / $MAX_LOC"
if [ "$loc" -gt "$MAX_LOC" ]; then
  echo "FAIL: $loc lines exceeds the $MAX_LOC budget. Raise it deliberately or cut scope." >&2
  status=1
fi

exit "$status"
