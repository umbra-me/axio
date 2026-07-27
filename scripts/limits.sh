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
MAX_FILE_LOC=300

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
#
# The struct is located rather than named by path. Hard-coding the file meant
# that moving it — `tool.rs` becoming `tool/mod.rs` — left the gate reading a
# file that no longer existed, reporting zero fields, and passing. A gate that
# cannot find what it guards must fail, not congratulate itself.
toolcx_file=$(git grep -l '^pub struct ToolCx {' -- '*.rs' | head -1 || true)
if [ -z "$toolcx_file" ]; then
  echo 'FAIL: cannot find "pub struct ToolCx" anywhere in the workspace' >&2
  exit 1
fi
toolcx=$(awk '/^pub struct ToolCx \{/{f=1;next} /^\}/{f=0} f' "$toolcx_file" \
  | grep -cE '^\s*pub [a-z_]+:' || true)
echo "ToolCx fields: $toolcx / $MAX_TOOLCX_FIELDS ($toolcx_file)"
if [ "$toolcx" -lt 1 ]; then
  echo "FAIL: ToolCx was found but has no fields — the gate is not reading it" >&2
  status=1
fi
if [ "$toolcx" -gt "$MAX_TOOLCX_FIELDS" ]; then
  echo "FAIL: ToolCx has $toolcx fields (max $MAX_TOOLCX_FIELDS)" >&2
  status=1
fi
if awk '/^pub struct ToolCx \{/{f=1;next} /^\}/{f=0} f' "$toolcx_file" \
  | grep -qE 'Option<Arc<dyn'; then
  echo "FAIL: ToolCx gained an Option<Arc<dyn ...>> host bridge" >&2
  status=1
fi

# --- workspace Rust LOC, excluding tests ------------------------------------
# Heuristic: everything from a file's first `#[cfg(test)]` to its end is a test
# module, which is where they live by convention here. Files under tests/ are
# excluded outright.
loc=0
widest=0
widest_file=""
while IFS= read -r f; do
  n=$(awk '/#\[cfg\(test\)\]/{exit} {print}' "$f" | grep -cvE '^\s*(//|$)' || true)
  loc=$((loc + n))
  if [ "$n" -gt "$widest" ]; then
    widest=$n
    widest_file=$f
  fi
  # A file past this is a module that has stopped being one thing. The answer
  # is child modules, never another crate: the dependency graph is the reason
  # the crate count is capped, and it is not what a long file is evidence of.
  if [ "$n" -gt "$MAX_FILE_LOC" ]; then
    echo "FAIL: $f is $n lines (max $MAX_FILE_LOC). Split it into modules." >&2
    status=1
  fi
done < <(git ls-files 'crates/*/src/*.rs' 'crates/*/src/**/*.rs')
echo "workspace Rust LOC (excl. tests): $loc / $MAX_LOC"
echo "widest file: $widest / $MAX_FILE_LOC ($widest_file)"
if [ "$loc" -gt "$MAX_LOC" ]; then
  echo "FAIL: $loc lines exceeds the $MAX_LOC budget. Raise it deliberately or cut scope." >&2
  status=1
fi

exit "$status"
