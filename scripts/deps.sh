#!/usr/bin/env bash
# axio-core links no transport, no terminal, and no filesystem walker.
#
# This is not an aesthetic rule: it is what keeps `cargo test -p axio-core`
# exercising the whole loop against fakes in under two seconds, and it is the
# thing that quietly stops being true the first time someone reaches for a
# convenience crate in the wrong place.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

FORBIDDEN='reqwest|rustls|^ring|aws-lc|ratatui|crossterm|^ignore|globset|hyper'

tree=$(cargo tree -p axio-core --edges normal --prefix none --no-dedupe 2>/dev/null | awk '{print $1}' | sort -u)

# grep exits 1 with no matches, which is the passing case — capture the output
# and test that instead of letting `set -e` fire on success.
hits=$(printf '%s\n' "$tree" | grep -E "$FORBIDDEN" || true)

if [ -n "$hits" ]; then
  echo "FAIL: axio-core has grown a forbidden dependency:" >&2
  printf '%s\n' "$hits" >&2
  exit 1
fi

count=$(printf '%s\n' "$tree" | grep -c . || true)
echo "axio-core normal dependencies: $count, none forbidden"
