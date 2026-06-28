//! Live smoke test of the Orange Cat Blacktop integration: hits the real WRC /
//! WEC / MotoGP calendar + standings endpoints once and reports what came back,
//! plus the poll tier each series' current data would select. ~10 requests.
//!
//! Run:  cargo run --example ocb_probe --features ssr
//!
//! Reads the key from config.toml / OCBLACKTOP_TOKEN, exactly like the server.

use chrono::{DateTime, Datelike, Duration, Utc};
use plaintextesports::feed::NormalizedMatch;
use plaintextesports::types::MatchStatus;

/// Mirrors `cache.rs::series_cadence`'s tiering — for reporting only (the real
/// poller's copy is what runs; this just shows what it would pick right now).
fn tier(rows: &[NormalizedMatch], now: DateTime<Utc>) -> &'static str {
    let mut soonest: Option<Duration> = None;
    for m in rows {
        match m.status {
            MatchStatus::Live => return "LIVE (fast)",
            MatchStatus::Upcoming => {
                let d = m.begin_at - now;
                if d >= Duration::zero() && soonest.is_none_or(|s| d < s) {
                    soonest = Some(d);
                }
            }
            _ => {}
        }
    }
    match soonest {
        Some(d) if d <= Duration::minutes(60) => "LIVE (fast)",
        Some(d) if d <= Duration::days(14) => "NEAR (medium)",
        _ => "IDLE (slow)",
    }
}

fn summarize(name: &str, rows: &[NormalizedMatch], now: DateTime<Utc>) {
    let live = rows
        .iter()
        .filter(|m| matches!(m.status, MatchStatus::Live))
        .count();
    let upcoming = rows
        .iter()
        .filter(|m| matches!(m.status, MatchStatus::Upcoming) && m.begin_at >= now)
        .count();
    println!(
        "  {name:<7} {:>3} rows ({live} live, {upcoming} upcoming) -> tier {}",
        rows.len(),
        tier(rows, now)
    );
    if let Some(m) = rows
        .iter()
        .filter(|m| m.begin_at >= now)
        .min_by_key(|m| m.begin_at)
    {
        let label = if m.series_name.is_empty() {
            m.team_a.label.as_str()
        } else {
            m.series_name.as_str()
        };
        let days = (m.begin_at - now).num_days();
        println!(
            "          next: {} \"{}\" in {days}d @ {}",
            m.league,
            label,
            m.begin_at.to_rfc3339()
        );
    }
}

fn main() {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    rt.block_on(run());
}

async fn run() {
    let cfg = plaintextesports::config::config();
    let Some(key) = cfg.ocblacktop_token.as_deref() else {
        eprintln!("OCBLACKTOP_TOKEN not set (config.toml / env) — nothing to probe");
        std::process::exit(1);
    };
    let client = reqwest::Client::builder()
        .user_agent(plaintextesports::http::USER_AGENT)
        .build()
        .expect("http client");
    let now = Utc::now();
    let year = now.year();

    println!("Orange Cat Blacktop live probe — {now}\n");
    println!("Calendars:");
    let mut cal_reqs: u64 = 0;
    match plaintextesports::ocblacktop::fetch_wrc(&client, key, year, now).await {
        Ok((rows, used)) => {
            cal_reqs += used;
            summarize("WRC", &rows, now);
            println!("          (WRC spent {used} request(s))");
        }
        Err(e) => {
            cal_reqs += 1;
            println!("  WRC FAILED: {e}");
        }
    }
    match plaintextesports::ocblacktop::fetch_wec(&client, key, year).await {
        Ok(rows) => {
            cal_reqs += 1;
            summarize("WEC", &rows, now);
        }
        Err(e) => println!("  WEC FAILED: {e}"),
    }
    match plaintextesports::ocblacktop::fetch_motogp(&client, key, year).await {
        Ok(rows) => {
            cal_reqs += 1;
            summarize("MotoGP", &rows, now);
        }
        Err(e) => println!("  MotoGP FAILED: {e}"),
    }

    println!("\nStandings:");
    let wrc_s = plaintextesports::ocblacktop::fetch_wrc_standings(&client, key).await;
    println!("  WRC    {} table(s)", wrc_s.tables.len());
    let wec_s = plaintextesports::ocblacktop::fetch_wec_standings(&client, key).await;
    println!("  WEC    {} table(s)", wec_s.tables.len());
    let motogp_s = plaintextesports::ocblacktop::fetch_motogp_standings(&client, key).await;
    println!("  MotoGP {} table(s)", motogp_s.tables.len());

    println!(
        "\n~{} calendar + 6 standings = ~{} requests spent by this probe",
        cal_reqs,
        cal_reqs + 6
    );
}
