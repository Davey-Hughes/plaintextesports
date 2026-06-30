#!/usr/bin/env bash
# Run the criterion benchmarks and point at the HTML report.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"
cargo bench --features ssr "$@"
echo
echo "Criterion HTML report: $ROOT/target/criterion/report/index.html"
