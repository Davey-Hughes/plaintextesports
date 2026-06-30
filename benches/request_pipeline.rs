//! Microbenchmarks for the per-request view pipeline.
//!
//! Every page request reads the in-memory snapshot and builds a `ScheduleView`
//! (requests never touch SQLite), so this is the real user-facing hot path:
//! `matches_in_window` -> `to_view` (per row) -> `collapse_wrc_days` ->
//! `group_days`, then JSON serialization over the server-fn boundary. Uses a
//! fixed `now` + deterministic `synthetic_snapshot` so numbers compare across
//! runs.
//!
//! Run: `cargo bench --features ssr`

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use plaintextesports::cache;
use plaintextesports::config::config;

fn fixed_now() -> DateTime<Utc> {
    "2026-06-30T18:00:00Z".parse().unwrap()
}

fn tz() -> Tz {
    "America/New_York".parse().unwrap()
}

/// End-to-end homepage render, scaled by snapshot size.
fn bench_homepage(c: &mut Criterion) {
    let cfg = config();
    let now = fixed_now();
    let tz = tz();
    let mut g = c.benchmark_group("homepage_render");
    for &n in &[100usize, 500, 1500, 3000] {
        let snap = cache::synthetic_snapshot(n, now);
        g.throughput(Throughput::Elements(n as u64));
        // Esports mode ("all") and traditional mode ("trad", exercises the
        // motorsport-event + WRC-collapse branch) have different hot paths.
        g.bench_with_input(BenchmarkId::new("all", n), &snap, |b, snap| {
            b.iter(|| cache::homepage_render(black_box(snap), cfg, now, "all", &tz, false));
        });
        g.bench_with_input(BenchmarkId::new("trad", n), &snap, |b, snap| {
            b.iter(|| cache::homepage_render(black_box(snap), cfg, now, "trad", &tz, false));
        });
    }
    g.finish();
}

/// Individual building blocks at a representative snapshot size.
fn bench_building_blocks(c: &mut Criterion) {
    let now = fixed_now();
    let tz = tz();
    let n = 1500usize;
    let matches = cache::synthetic_matches(n, now);
    let snap = cache::snapshot_from(matches.clone(), now);

    let mut g = c.benchmark_group("building_blocks");

    g.bench_function("to_view_single", |b| {
        let m = &matches[0];
        b.iter(|| cache::to_view(black_box(m), &tz, now, false));
    });

    g.bench_function("matches_in_window_esports", |b| {
        let start = now - chrono::Duration::days(5);
        let end = now + chrono::Duration::days(15);
        b.iter(|| {
            cache::matches_in_window(black_box(&snap), false, None, start, end, &tz, now, false)
        });
    });

    g.bench_function("collapse_wrc_days", |b| {
        // Pre-build the views once; measure only the collapse pass.
        let start = now - chrono::Duration::days(5);
        let end = now + chrono::Duration::days(15);
        let views = cache::matches_in_window(&snap, true, None, start, end, &tz, now, false);
        b.iter_batched(
            || views.clone(),
            |v| cache::collapse_wrc_days(black_box(v), &tz, &snap),
            criterion::BatchSize::SmallInput,
        );
    });

    g.bench_function("group_days", |b| {
        let start = now - chrono::Duration::days(5);
        let end = now + chrono::Duration::days(15);
        let views = cache::matches_in_window(&snap, false, None, start, end, &tz, now, false);
        b.iter_batched(
            || views.clone(),
            |v| cache::group_days(black_box(v), &tz),
            criterion::BatchSize::SmallInput,
        );
    });

    g.bench_function("next_motorsport_events", |b| {
        b.iter(|| cache::next_motorsport_events(black_box(&snap)));
    });

    g.finish();
}

/// Wire-serialization cost paid on every request (server-fn boundary).
fn bench_serialize(c: &mut Criterion) {
    let cfg = config();
    let now = fixed_now();
    let tz = tz();
    let view = cache::homepage_render(
        &cache::synthetic_snapshot(1500, now),
        cfg,
        now,
        "all",
        &tz,
        false,
    );
    c.bench_function("serialize_schedule_view_1500", |b| {
        b.iter(|| serde_json::to_string(black_box(&view)).unwrap());
    });
}

criterion_group!(
    benches,
    bench_homepage,
    bench_building_blocks,
    bench_serialize
);
criterion_main!(benches);
