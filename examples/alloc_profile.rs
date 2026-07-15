//! Allocation profile for one homepage request.
//!
//! Timing benches (`cargo bench`) don't surface allocations, but the per-request
//! cost is dominated by clones. This installs dhat as the global allocator, does
//! the work one homepage request does over a synthetic snapshot, and prints total
//! allocations + bytes — a hard, re-runnable number to verify clone-reduction work
//! against. Also writes `dhat-heap.json` for the dhat viewer
//! (https://nnethercote.github.io/dh_view/dh_view.html).
//!
//! A request has two halves, and they're profiled separately because they regress
//! independently:
//!
//!   1. **build** — `cache::homepage_render`: turn the snapshot into a
//!      `ScheduleView`, plus the JSON serialization that ships it.
//!   2. **render** — `app::schedule_render_work`: what the schedule component then
//!      does with that view, on every SSR render and again on every hydrate.
//!
//! Only half 1 used to be measured, which meant a regression in half 2 was
//! invisible here — `chip_key` allocating per league group didn't move these
//! numbers at all, because it lives in the component.
//!
//! Run: `cargo run --release --example alloc_profile --features ssr`
//!      `cargo run --release --example alloc_profile --features ssr 3000`

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use plaintextesports::app::schedule_render_work;
use plaintextesports::cache;
use plaintextesports::config::config;
use std::collections::HashSet;

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

/// Allocations, total bytes, and peak bytes for one measured region.
struct Stats {
    blocks: u64,
    bytes: u64,
    peak: usize,
}

/// Run `f` under a fresh dhat profiler and report what it allocated. Each region
/// gets its own profiler so the numbers are per-half rather than cumulative.
fn measure<T>(f: impl FnOnce() -> T) -> (T, Stats) {
    let profiler = dhat::Profiler::builder().testing().build();
    let out = f();
    let s = dhat::HeapStats::get();
    drop(profiler);
    (
        out,
        Stats {
            blocks: s.total_blocks,
            bytes: s.total_bytes,
            peak: s.max_bytes,
        },
    )
}

fn print_stats(label: &str, s: &Stats) {
    println!("{label}");
    println!("  total allocations: {}", s.blocks);
    println!("  total bytes:       {}", s.bytes);
    println!("  peak bytes:        {}", s.peak);
}

fn main() {
    let now: DateTime<Utc> = "2026-06-30T18:00:00Z".parse().unwrap();
    let tz: Tz = "America/New_York".parse().unwrap();
    let cfg = config();
    let n: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1500);
    let snap = cache::synthetic_snapshot(n, now);

    // 1) Build the view + serialize it — the half that was always measured here.
    let ((view, json_len), build) = measure(|| {
        let view = cache::homepage_render(&snap, cfg, now, "all", &tz, false);
        let json = serde_json::to_string(&view).unwrap();
        let len = json.len();
        std::hint::black_box(&json);
        (view, len)
    });

    // 2) The component's per-render work over that view. Unfiltered ("all" games,
    //    no league selection) — the default homepage, and the widest case: every
    //    day and league group survives the filter.
    let games: HashSet<String> = HashSet::new();
    let leagues: HashSet<String> = HashSet::new();
    let (days, render) = measure(|| schedule_render_work(&view, &games, &leagues, false));

    println!("--- homepage request alloc profile (n={n}) ---");
    print_stats("build  (homepage_render + JSON):", &build);
    print_stats("render (schedule component):", &render);
    println!("total allocations: {}", build.blocks + render.blocks);
    println!("total bytes:       {}", build.bytes + render.bytes);
    println!("json size:         {json_len} bytes");
    println!("prepared days:     {days}");
    println!("wrote dhat-heap.json");
}
