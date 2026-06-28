//! Microbenchmark for the per-match tier-1 filter.
//!
//! `tiering::is_tier_one` runs once per raw match before normalization in every
//! PandaScore scan (a deep upcoming scan is up to ~3000 matches, plus
//! running/past/backfill). Today `decide()` allocates up to three short Strings
//! plus a Vec per call purely to do case-insensitive equality/substring checks,
//! so this bench exists to measure that cost and verify an allocation-free
//! rewrite is actually faster.
//!
//! Run: `cargo bench --features ssr`

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use plaintextesports::tiering::{is_tier_one, TierInput};
use plaintextesports::types::Sport;

fn bench_is_tier_one(c: &mut Criterion) {
    // One input per decision branch, mirroring the spread a real scan sees:
    // a base-keep (tier S), a denylist hit (substring), and a base-reject.
    let base_keep = TierInput {
        league_slug: Some("LEC"),
        series_slug: Some("lec-summer-2026"),
        tournament_slug: Some("playoffs"),
        tier: Some("s"),
    };
    let deny_hit = TierInput {
        league_slug: Some("LCK CL"),
        series_slug: Some("lck-challengers-league-2026"),
        tournament_slug: Some("academy-qualifier"),
        tier: Some("a"),
    };
    let base_reject = TierInput {
        league_slug: Some("some-minor-league"),
        series_slug: Some("minor-series-2026"),
        tournament_slug: Some("week-1"),
        tier: Some("b"),
    };
    let inputs = [&base_keep, &deny_hit, &base_reject];

    let mut g = c.benchmark_group("is_tier_one");
    g.bench_function("lol_mixed_branches", |b| {
        b.iter(|| {
            let mut kept = 0u32;
            for inp in inputs {
                if is_tier_one(black_box(Sport::Lol), black_box(inp)) {
                    kept += 1;
                }
            }
            kept
        })
    });
    g.finish();
}

criterion_group!(benches, bench_is_tier_one);
criterion_main!(benches);
