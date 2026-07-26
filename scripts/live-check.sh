#!/usr/bin/env bash
# A real turn against a real model, in a throwaway directory.
#
# Everything else in the suite runs against a stub. This is the only check that
# proves the wire format is right, and it is the reason the Anthropic path is
# labelled unverified in the README: nobody has run this against it.
#
#   scripts/live-check.sh                  # whichever provider is configured
#   AXIO_PROVIDER=anthropic scripts/live-check.sh
#
# Costs a handful of cheap turns. Never runs in CI: it needs a credential and
# it spends money.
set -uo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
bin="$root/target/debug/axio"
[ -x "$bin" ] || { echo "build first: cargo build -p axio"; exit 2; }

sandbox=$(mktemp -d)
trap 'rm -rf "$sandbox"' EXIT
export AXIO_STATE="$sandbox/state"
cd "$sandbox"

provider=$("$bin" --explain model.provider >/dev/null 2>&1 && "$bin" --doctor 2>/dev/null | awk '/^  provider/ {print $2}')
echo "provider: ${provider:-unknown}"
echo "sandbox:  $sandbox"
echo

pass=0
fail=0
check() {
  local name=$1 condition=$2
  if [ "$condition" = "1" ]; then
    echo "  ok    $name"
    pass=$((pass + 1))
  else
    echo "  FAIL  $name"
    [ -n "${detail:-}" ] && echo "        $detail"
    fail=$((fail + 1))
  fi
  detail=""
}

echo "1. a plain turn"
out=$(timeout 180 "$bin" -p "Reply with exactly the word: pineapple" 2>/dev/null)
check "the model answered" "$(echo "$out" | grep -qi pineapple && echo 1 || echo 0)"
# Captured now: later steps each start their own session, so the newest is not
# the one whose history this checks.
first_session=$("$bin" --list 2>/dev/null | head -1 | awk '{print $1}')

echo "2. a tool-using turn"
printf 'alpha\nbeta\ngamma\n' > words.txt
out=$(timeout 180 "$bin" -p "Read words.txt and reply with only its second line." 2>/dev/null)
check "the file was read" "$(echo "$out" | grep -qi beta && echo 1 || echo 0)"

echo "3. a write, approved"
out=$(timeout 180 "$bin" --yes -p "Create a file named made.txt containing exactly: done" 2>&1)
check "the file was written" "$([ -f made.txt ] && echo 1 || echo 0)"
check "the change was reported" "$(echo "$out" | grep -q 'changed:' && echo 1 || echo 0)"

echo "4. a write, refused"
out=$(timeout 180 "$bin" -p "Create a file named refused.txt containing: nope" 2>&1)
code=$?
check "nothing was written" "$([ ! -f refused.txt ] && echo 1 || echo 0)"
check "the refusal was visible" "$(echo "$out" | grep -q '\[denied\]' && echo 1 || echo 0)"
check "the exit code says so" "$([ "$code" = "5" ] && echo 1 || echo 0)"

echo "5. the built-in deny list"
printf 'API_KEY=live-check-canary\n' > .env
out=$(timeout 180 "$bin" --yes -p "Use bash to cat .env and report API_KEY." 2>&1)
check "the secret stayed out of the output" \
  "$(echo "$out" | grep -q 'live-check-canary' && echo 0 || echo 1)"
check "the secret stayed out of the transcript" \
  "$(grep -rq 'live-check-canary' "$AXIO_STATE" 2>/dev/null && echo 0 || echo 1)"

echo "6. sessions"
check "a session was recorded" "$([ -n "$first_session" ] && echo 1 || echo 0)"
listing=$("$bin" --list 2>/dev/null | head -1)
detail="listing was: $listing"
check "the listing is readable" \
  "$(echo "$listing" | grep -qE '(just now|[0-9]+[mhd] ago)' && echo 1 || echo 0)"
if [ -n "$first_session" ]; then
  timeout 180 "$bin" --resume "$first_session" -p "Reply with the word: banana" \
    >/dev/null 2>&1
  # Asserted on the transcript, not on the answer: whether the model recalls
  # something is a fact about the model, and this is a check on axio. The
  # resumed file holding both turns is what "carried the history" means.
  file=$(find "$AXIO_STATE/sessions" -name "$first_session*.jsonl" | head -1)
  check "resume appended to the same session" \
    "$([ -n "$file" ] && grep -q pineapple "$file" && grep -q banana "$file" && echo 1 || echo 0)"
  check "resume recorded a second turn" \
    "$([ -n "$file" ] && [ "$(grep -c '"rec":"turn_ended"' "$file")" -ge 2 ] && echo 1 || echo 0)"
fi

echo
echo "$pass passed, $fail failed"
[ "$fail" = "0" ]
