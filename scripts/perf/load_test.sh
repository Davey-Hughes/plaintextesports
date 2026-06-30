#!/usr/bin/env bash
# SSR latency load test. Start the server first:
#   cargo leptos serve --release
# Usage: scripts/perf/load_test.sh [base_url]   (default http://127.0.0.1:3000)
set -euo pipefail
BASE="${1:-${BASE_URL:-http://127.0.0.1:3000}}"
routes=("/" "/day/$(date +%F)")

if ! curl -fsS -o /dev/null "$BASE/" 2>/dev/null; then
  echo "Server not reachable at $BASE — start it: cargo leptos serve --release" >&2
  exit 1
fi

run() {
  local tool="$1"; shift
  for r in "${routes[@]}"; do
    echo "== $tool $BASE$r =="
    "$tool" "$@" "$BASE$r"
  done
}

if command -v oha >/dev/null 2>&1; then
  run oha -n 500 -c 20 --no-tui
elif command -v hey >/dev/null 2>&1; then
  run hey -n 500 -c 20
else
  echo "Need a load tester. Install one:" >&2
  echo "  cargo install oha                              # preferred" >&2
  echo "  go install github.com/rakyll/hey@latest" >&2
  exit 1
fi
