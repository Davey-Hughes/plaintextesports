# Performance tooling

Scripts and benchmarks for measuring this server's performance. The architecture
fact that shapes all of this: **every page request reads the in-memory snapshot
and builds a `ScheduleView` — requests never touch SQLite.** So per-request cost
is snapshot processing + clones + JSON serialization, and that is what these
tools measure.

**Note on reproducibility.** Benchmark and allocation-profile numbers assume the
default configuration (empty `.env` / no overriding `config.toml`). The
`config()` function's `upcoming_days` and `tz` values influence the esports
window size and therefore row counts, so cross-machine comparisons should use
the same config to get comparable results.

## Benchmarks (criterion)

```sh
scripts/perf/bench.sh                 # all benches, then prints the HTML report path
cargo bench --features ssr            # equivalent
cargo bench --features ssr --bench request_pipeline   # just the request pipeline
cargo bench --features ssr --bench tiering            # just the poller tier filter
```

- `request_pipeline` — the per-request hot path: `homepage_render` scaled over
  100/500/1500/3000 matches (esports + traditional modes), the individual
  building blocks (`to_view`, `matches_in_window`, `collapse_wrc_days`,
  `group_days`, `next_motorsport_events`), and `ScheduleView` JSON serialization.
- `tiering` — `is_tier_one`, the poller-side per-match filter.

Criterion compares against the previous run automatically and reports regressions.

## Allocation profile (dhat)

```sh
cargo run --release --example alloc_profile --features ssr        # n=1500
cargo run --release --example alloc_profile --features ssr 3000   # custom size
```

Prints total allocations / bytes / peak bytes for one homepage render and writes
`dhat-heap.json` (load it at https://nnethercote.github.io/dh_view/dh_view.html).
Use this to verify clone-reduction work — the number should drop.

## WASM bundle size

```sh
cargo leptos build --release      # required first — only this emits valid assets
scripts/perf/wasm_size.sh         # raw + gzip + brotli vs budget; exits 2 if over
```

The budget lives in `wasm-budget.txt` (`gzip_kb=<N>`). Raise it deliberately when
a feature genuinely grows the bundle. Install `twiggy` (`cargo install twiggy`)
for a per-item code-size breakdown.

## SSR latency under load

```sh
cargo leptos serve --release                 # in one terminal
scripts/perf/load_test.sh                     # in another (default :3000)
scripts/perf/load_test.sh http://host:port    # custom base URL
```

Reports p50/p95/p99 over `/` and `/day/<today>`. Needs `oha`
(`cargo install oha`, preferred) or `hey`.

## CPU profiling

```sh
scripts/perf/flamegraph.sh
```

Profiles the `request_pipeline` bench. Needs `samply` (`cargo install samply`,
preferred — no root) or `flamegraph` (`cargo install flamegraph`, needs `perf`).
