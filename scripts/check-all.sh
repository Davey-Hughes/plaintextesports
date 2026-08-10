#!/usr/bin/env bash
# The local mirror of CI (.forgejo/workflows/ci.yml). Run it before opening a pull request —
# nothing runs it for you now that scripts/land.sh is retired, and CI runs these same checks
# against the PR head rather than against the merged tree.
#
#   scripts/check-all.sh               # everything CI gates on
#   scripts/check-all.sh --no-release  # skip the slow legs: release wasm build + bench smoke
#
# The point of this file is that the local gate and the CI gate cannot drift: CI's gating jobs are
# reproduced here in the same order with the same flags. When a job changes there, change it here.
#
# WHY BOTH FEATURE SETS. This is a Leptos app: `ssr` and `hydrate` are mutually exclusive and compile
# DIFFERENT code. A change can be clean under ssr and fail to compile under hydrate — that is what
# the second block exists to catch, and running only `cargo test` would miss it entirely.
#
# The release leg is ON by default rather than opt-in, because the rule it protects is that every
# commit on main builds and passes CI by itself; a gate that skips a CI job by default cannot make
# that claim. --no-release is there for iterating, not for opening a PR. The bench smoke run is
# grouped with it for the same reason it is slow: both pay for a fresh non-debug compile.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

# Parse up front so a typo (`--no-releas`) fails immediately rather than silently falling through to
# a full run — a flag that quietly does the opposite of what was asked is the same class of bug as a
# gate that quietly covers less than it claims.
no_release=0
case "${1:-}" in
  "")           ;;
  --no-release) no_release=1 ;;
  *)            echo "error: unknown argument: $1" >&2; exit 2 ;;
esac
[ "$#" -le 1 ] || { echo "error: too many arguments" >&2; exit 2; }

run() { echo; echo "==> $*"; "$@"; }

# --- the `rust` job (ssr) ---
run cargo fmt --all --check
run cargo clippy --features ssr --all-targets -- -D warnings
run cargo test --features ssr
# Doc links break silently: nothing else in this file compiles a doc comment, so a
# `[`renamed_fn`]` left behind by a refactor survives every other gate here.
# `--document-private-items` is the view that matters — this crate is `pub` only so
# the binary and the wasm build can consume it, so the interesting items are
# `pub(crate)` and absent from the default build.
run env RUSTDOCFLAGS=-Dwarnings cargo doc --no-deps --document-private-items --bins --features ssr

# --- the `hydrate` job ---
run cargo clippy --lib --no-default-features --features hydrate -- -D warnings
run cargo test --lib --no-default-features --features hydrate
# Docs need both feature sets for the same reason clippy does: they compile
# different code, and a link is resolved against whichever half is present. A
# path into `cache` (ssr-only) resolves under ssr and dangles under hydrate.
run env RUSTDOCFLAGS=-Dwarnings cargo doc --no-deps --document-private-items --lib --no-default-features --features hydrate

# --- the `rust` job's bench smoke (slow: benches compile under their own profile) ---
if [ "$no_release" -eq 1 ]; then
  echo; echo "==> SKIPPED: cargo bench -- --test (--no-release)"
else
  # `--all-targets` above type-checks the benches but never runs them, so a bench
  # that panics on its own fixtures is invisible. Criterion's `--test` runs each
  # once with no measurement.
  run cargo bench --features ssr -- --test
fi

# --- the `wasm` job ---
if [ "$no_release" -eq 1 ]; then
  echo; echo "==> SKIPPED: cargo leptos build --release (--no-release)"
  echo "    CI still runs it. Do not land on the strength of this run alone."
else
  if ! command -v cargo-leptos >/dev/null 2>&1; then
    echo "error: cargo-leptos not found (CI's wasm job uses it)." >&2
    echo "  install: cargo install cargo-leptos --locked" >&2
    echo "  or re-run with --no-release, knowing CI will still gate on it." >&2
    exit 1
  fi
  run cargo leptos build --release
fi

echo
echo "all green"
