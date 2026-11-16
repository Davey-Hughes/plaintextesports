# Development

Running the app locally, the tests and live smoke tests, and the performance
tooling.
Back to the [README](../README.md).

## Develop

```sh
LEPTOS_HASH_FILES=false cargo leptos watch # http://127.0.0.1:4000
DEMO=1 cargo leptos watch --release        # force fixture data (ignores token + cache db)
cargo test --lib --features ssr            # tiering + deserialization tests (fast loop)
cargo test --features ssr                  # + bin-target and doc tests, as CI runs it
cargo leptos test                           # both feature sets: ssr and hydrate
scripts/check-all.sh                        # everything CI gates on, in one command
scripts/check-all.sh --no-release           # ...minus the release wasm build
```

Reach for `--lib` while iterating. The tests themselves take 0.07s — essentially
all of a test run is recompiling and relinking the crate, and `--lib` skips two
things that contribute nothing: the `src/main.rs` test binary (0 tests, but a
full second link of a 44k-line crate) and the rustdoc pass (0 doc tests). With
the `[profile.test]` debuginfo setting in `Cargo.toml`, that is the difference
between ~8s and ~3.3s per edit. Run the plain form before pushing, since it is
what CI gates on and it is the one that would catch a doc test.

Run `cargo leptos test` when touching anything `#[cfg]`-gated. It is exactly two
`cargo test` invocations — ssr, then `--lib --no-default-features --features
hydrate` — and CI runs them as two parallel jobs. The hydrate side adds no new
test functions (all 82 are already among the 404), but it is the only thing that
compiles the `#[cfg(test)]` modules against the client feature set. Prefer it
over running the hydrate command by hand: cargo-leptos gives that build its own
`target/front` dir, whereas running it directly reuses `target/` and forces a
full rebuild every time you switch feature sets.

Twelve `#[ignore]`d live smoke tests hit the real upstreams. They stay out of CI
so network flakiness can never block a merge — run them by hand when touching a
fetcher:

```sh
cargo test --lib --features ssr -- --ignored --nocapture          # all twelve
cargo test --lib --features ssr mlb_live_bracket -- --ignored --nocapture
```

Most are season-bound and will fail out of season by design: they pin a year
(`fetch_bracket(&client, 2025)`), and `espn_live_soccer_bracket` wants a World
Cup in progress. They mostly print for eyeballing — the assertions are shallow
("a bracket exists", "it reaches a Final"), so they catch a hard break, not
quiet field-level drift.

`live_full_refresh` is the exception and the only one scheduled: it walks the
whole CompeteTFT discover → refresh → assemble pipeline and works off-broadcast,
so `.forgejo/workflows/live-smoke.yml` runs it weekly. It gates nothing — a
failure there means "go look", not "the build is broken".

`LEPTOS_HASH_FILES=false` is not optional in a watch loop. This project sets
`hash-files = true` for production cache-busting, but cargo-leptos only hashes
the site on its *first* build — `add_hashes_to_site` runs in `build_proj`, and
the incremental rebuild path (`watch::runner`) never calls it. So every edit
after startup rebuilds the server binary while `hash.txt` and the hashed
`pkg/*.wasm` stay at build #1: the page serves fresh SSR HTML that points the
browser at a stale client bundle.

The failure is nastier than it sounds. A structural edit panics hydration
(`tachys … hydration.rs: unreachable`) and the whole page goes dead — no
handlers, no reactivity — while a text-only edit just silently runs old client
code. Both look like bugs in your change.

The env var is read at build time (it overrides the manifest) and is forwarded
to the server, so both sides agree on unhashed names. Upstream:
[cargo-leptos#271](https://github.com/leptos-rs/cargo-leptos/issues/271), open
since 2024, still unfixed as of 0.3.11.

## Performance

Benchmarks and profiling tooling live in [`scripts/perf/`](../scripts/perf/README.md):
criterion benches for the per-request view pipeline (`cargo bench --features ssr`),
a dhat allocation profile, a WASM bundle-size budget check, an SSR load test, and
a CPU-profiling recipe.
