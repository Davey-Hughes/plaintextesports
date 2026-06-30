#!/usr/bin/env bash
# CPU profile the request-pipeline bench. Prefers samply (no root needed).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

if command -v samply >/dev/null 2>&1; then
  echo "Building bench binary..."
  cargo bench --features ssr --bench request_pipeline --no-run
  bin="$(ls -t "$ROOT"/target/release/deps/request_pipeline-* 2>/dev/null | grep -v '\.d$' | head -1 || true)"
  if [ -z "${bin:-}" ]; then
    echo "Could not locate the compiled bench binary under target/release/deps." >&2
    exit 1
  fi
  echo "Profiling $bin with samply (opens the profile in your browser)..."
  samply record -- "$bin" --bench --profile-time 10
elif command -v cargo-flamegraph >/dev/null 2>&1; then
  cargo flamegraph --features ssr --bench request_pipeline -- --bench
else
  echo "Need a profiler. Install one:" >&2
  echo "  cargo install samply       # preferred, no root" >&2
  echo "  cargo install flamegraph   # needs perf + permissions" >&2
  exit 1
fi
