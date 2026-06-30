//! Allocation profile for one homepage render.
//!
//! Timing benches (`cargo bench`) don't surface allocations, but the per-request
//! cost is dominated by clones building the `ScheduleView`. This installs dhat as
//! the global allocator, renders one homepage view over a synthetic snapshot, and
//! prints total allocations + bytes — a hard, re-runnable number to verify
//! clone-reduction work against. Also writes `dhat-heap.json` for the dhat viewer
//! (https://nnethercote.github.io/dh_view/dh_view.html).
//!
//! Run: `cargo run --release --example alloc_profile --features ssr`

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use plaintextesports::cache;
use plaintextesports::config::config;

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

fn main() {
    let now: DateTime<Utc> = "2026-06-30T18:00:00Z".parse().unwrap();
    let tz: Tz = "America/New_York".parse().unwrap();
    let cfg = config();
    let n: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1500);
    let snap = cache::synthetic_snapshot(n, now);

    let profiler = dhat::Profiler::builder().build();
    // The measured unit of work: one full homepage render + its JSON serialization.
    let view = cache::homepage_render(&snap, cfg, now, "all", &tz, false);
    let json = serde_json::to_string(&view).unwrap();
    std::hint::black_box(&json);

    let stats = dhat::HeapStats::get();
    drop(profiler); // flushes dhat-heap.json
    println!("--- homepage render alloc profile (n={n}) ---");
    println!("total allocations: {}", stats.total_blocks);
    println!("total bytes:       {}", stats.total_bytes);
    println!("peak bytes:        {}", stats.max_bytes);
    println!("json size:         {} bytes", json.len());
    println!("wrote dhat-heap.json");
}
