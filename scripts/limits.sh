#!/usr/bin/env bash
# Structural limits, reported as numbers on every run.
#
# These exist because a rule written in a markdown file is not an enforcement
# mechanism. Raising any of them is allowed — in a commit whose message says
# why.
#
# Some of them fail the build; the line counts do not, and the difference is
# deliberate. A crate count, a `ToolCx` field count and a file length can
# always be satisfied without giving anything up — by not adding a dependency
# edge, by not coupling a tool to a host, by splitting a module. The only way
# to satisfy a line-count ceiling is to write less code, which means building
# less. Worse, as it tightens it rewards density: comments are free here and
# code is not, so the pressure would be toward clever code with long
# explanations, which is backwards for this codebase.
#
# So the counts are reported and never enforced. Being able to see them is what
# catches drift; failing on them only interrupts someone mid-change to edit a
# constant they are permitted to edit anyway.
#
# They are per crate because one workspace number cannot distinguish the two
# things it is summing. `axio` is where surfaces land and it is meant to grow —
# a TUI, then whatever follows it. `axio-core` is the root every other crate
# depends on, so lines there are where coupling accumulates, and the same
# thousand lines mean something entirely different. A single total reports both
# as the same event. The workspace figure is the sum of the parts rather than a
# separate knob, so it cannot disagree with them.
#
# A crate with no budget entry does fail, and that is not an exception to the
# above: adding a line to `crate_budget` gives up nothing, and a crate the
# script cannot price is one it has quietly stopped tracking — the same failure
# the `ToolCx` lookup below is written to avoid.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

MAX_MEMBERS=4
MAX_TOOLCX_FIELDS=5
MAX_FILE_LOC=300

# Reported, not enforced — see the header. A crate absent from this list fails,
# so a rename cannot drop one out of the report unnoticed.
crate_budget() {
  case "$1" in
    axio-core) echo 6000 ;;      # the loop, protocol, session, config, policy
    axio-cost) echo 6000 ;;      # one parser per agent, and there are dozens of agents
    axio-provider) echo 5000 ;;  # one block per transport
    axio-quota) echo 3000 ;;     # probes, plus the Windows tray surface
    axio-supervisor) echo 3000 ;; # session lifecycle, worktrees, the approval queue, the index
    axio-tools) echo 4000 ;;     # one module per tool
    axio) echo 10000 ;;          # every surface
    *) echo '' ;;
  esac
}

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

# --- Rust LOC per crate, excluding tests -------------------------------------
# The countable body of a file is everything outside a `#[cfg(test)]` item.
#
# This used to truncate at the first `#[cfg(test)]`, on the assumption that
# tests are one module at the bottom. `redact.rs` has a `#[cfg(test)] fn`
# helper at line 29 and its test module at 136, so a 185-line file was scored
# as 14 — and no file with an early test helper could ever have failed the
# width check, however wide it grew. A gate that can only err toward passing is
# the one that never gets caught by use.
#
# So the test module still ends the count — everything below it is tests by
# convention — but a `#[cfg(test)]` item that is not a module is skipped by
# brace balance and counting resumes after it.
#
# The module is not balanced through, only truncated at, and that is not
# laziness: test code contains deliberately malformed fixtures. `auth.rs` has
# the literal `"{ not json"` and `sse.rs` a truncated JSON frame, so brace
# counting through a test module is unreliable by construction.
non_test_body() {
  awk '
    { sub(/\r$/, "") }
    /^[[:space:]]*#\[cfg\(test\)\]/ { pending = 1; next }
    pending && /^[[:space:]]*$/ { next }
    pending && /^[[:space:]]*(pub[[:space:]]+)?mod[[:space:]]/ { exit 0 }
    pending { pending = 0; skip = 1; depth = 0; opened = 0 }
    skip {
      o = gsub(/\{/, "{"); c = gsub(/\}/, "}")
      if (o > 0) opened = 1
      depth += o - c
      if (opened && depth <= 0) skip = 0
      else if (!opened && /;[[:space:]]*$/) skip = 0
      next
    }
    { print }
    END { if (skip) exit 3 }
  ' "$1"
}

# The crate list is derived from the tracked paths rather than from Cargo.toml,
# so it names what is actually there.
loc=0
budgeted=0
widest=0
widest_file=""
over=""

echo "Rust LOC (excl. tests):"
for crate in $(git ls-files 'crates/*/src/*.rs' | cut -d/ -f2 | sort -u); do
  crate_loc=0
  while IFS= read -r f; do
    if ! body=$(non_test_body "$f"); then
      echo "FAIL: $f has an unterminated #[cfg(test)] item and cannot be counted" >&2
      status=1
      continue
    fi
    n=$(printf '%s\n' "$body" | grep -cvE '^\s*(//|$)' || true)
    crate_loc=$((crate_loc + n))
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
  done < <(git ls-files "crates/$crate/src/*.rs")

  loc=$((loc + crate_loc))
  budget=$(crate_budget "$crate")
  if [ -z "$budget" ]; then
    printf '  %-14s %6d / (none)\n' "$crate" "$crate_loc"
    echo "FAIL: crate '$crate' has no entry in crate_budget()" >&2
    status=1
    continue
  fi
  budgeted=$((budgeted + budget))
  printf '  %-14s %6d / %6d\n' "$crate" "$crate_loc" "$budget"
  if [ "$crate_loc" -gt "$budget" ]; then
    over="$over $crate"
  fi
done

printf '  %-14s %6d / %6d\n' "workspace" "$loc" "$budgeted"
echo "widest file: $widest / $MAX_FILE_LOC ($widest_file)"
if [ -n "$over" ]; then
  echo "note: over budget:$over. Not a failure — a number worth knowing, and" >&2
  echo "      worth asking whether the scope grew on purpose." >&2
fi

exit "$status"
