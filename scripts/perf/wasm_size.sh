#!/usr/bin/env bash
# Measure the hydration WASM bundle (raw + gzip + brotli) against a budget.
# Build first: `cargo leptos build --release` (only that emits valid hashed assets).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
WASM="$(ls -1 "$ROOT"/target/site/pkg/*.wasm 2>/dev/null | head -1 || true)"
if [ -z "${WASM:-}" ]; then
  echo "No wasm at target/site/pkg/*.wasm — run: cargo leptos build --release" >&2
  exit 1
fi

raw=$(wc -c < "$WASM")
gz=$(gzip -9 -c "$WASM" | wc -c)
br="(install brotli)"
if command -v brotli >/dev/null 2>&1; then
  br_bytes=$(brotli -q 11 -c "$WASM" | wc -c || true)
  br="${br_bytes} bytes ($(( (br_bytes + 1023) / 1024 )) KiB)"
fi
gz_kb=$(( (gz + 1023) / 1024 ))

budget_file="$ROOT/scripts/perf/wasm-budget.txt"
budget=$(grep -E '^gzip_kb=' "$budget_file" 2>/dev/null | cut -d= -f2 || true)
if [ -z "${budget:-}" ]; then
  echo "Budget not found in $budget_file — set gzip_kb=<N> there (see the budget-init step)." >&2
  exit 1
fi

printf 'wasm:        %s\n' "$WASM"
printf 'raw:         %d bytes (%d KiB)\n' "$raw" "$((raw / 1024))"
printf 'gzip:        %d bytes (%d KiB)\n' "$gz" "$gz_kb"
printf 'brotli:      %s\n' "$br"
printf 'budget gzip: %d KiB\n' "$budget"

if command -v twiggy >/dev/null 2>&1; then
  echo "--- twiggy top (code size by item) ---"
  twiggy top -n 20 "$WASM" || true
else
  echo "(install twiggy for a code-size breakdown: cargo install twiggy)"
fi

if [ "$gz_kb" -gt "$budget" ]; then
  echo "OVER BUDGET: gzip ${gz_kb} KiB > ${budget} KiB" >&2
  exit 2
fi
echo "within budget."
