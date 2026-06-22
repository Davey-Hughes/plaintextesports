//! In-memory schedule cache + background poller + view builders (server-only).
//!
//! A single background task polls PandaScore on an interval and stores a
//! normalized, tier-1-filtered snapshot behind an `RwLock`. Page requests read
//! the snapshot and format it into [`ScheduleView`] for the requested timezone —
//! they never touch the network. When no API token is configured we populate a
//! demo fixture (relative to "now") so the UI is always usable.

use crate::config::Config;
use crate::pandascore::{fetch_game, FetchResult, NormTeam, NormalizedMatch};
use crate::types::{DayGroup, Game, MatchStatus, MatchView, ScheduleView, TeamView};
use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use once_cell::sync::Lazy;
use std::sync::RwLock;

struct Snapshot {
    matches: Vec<NormalizedMatch>,
    fetched_at: DateTime<Utc>,
    stale: bool,
    using_fixture: bool,
}

static SNAPSHOT: Lazy<RwLock<Snapshot>> = Lazy::new(|| {
    RwLock::new(Snapshot {
        matches: Vec::new(),
        fetched_at: Utc::now(),
        stale: true,
        using_fixture: false,
    })
});

/// Keep matches whose start is within this many days of "now" in the DB; older
/// ones are pruned after each poll.
const RETENTION_DAYS: i64 = 2;

/// Start the background poller. Loads any persisted matches first (so a restart
/// serves data immediately), then either polls PandaScore or, with no token,
/// falls back to demo data.
pub fn spawn_poller() {
    let cfg = Config::from_env();

    // Open the persistent cache (best-effort; degrade to memory-only on error).
    let mut store = if cfg.db_path.is_empty() {
        None
    } else {
        match crate::store::open(&cfg.db_path) {
            Ok(conn) => Some(conn),
            Err(e) => {
                leptos::logging::log!("cache db open failed ({e}); running memory-only");
                None
            }
        }
    };

    // Serve persisted data immediately on boot.
    if let Some(conn) = store.as_ref() {
        if let Ok(stored) = crate::store::load_all(conn) {
            if !stored.is_empty() {
                let fetched = crate::store::load_fetched_at(conn)
                    .and_then(DateTime::from_timestamp_millis)
                    .unwrap_or_else(Utc::now);
                let mut snap = SNAPSHOT.write().unwrap();
                let n = stored.len();
                snap.matches = stored;
                snap.fetched_at = fetched;
                snap.stale = true; // refreshed by the first live poll
                snap.using_fixture = false;
                leptos::logging::log!("loaded {n} matches from cache db");
            }
        }
    }

    let Some(token) = cfg.token.clone() else {
        leptos::logging::log!("PANDASCORE_TOKEN not set — serving demo fixture data");
        let mut snap = SNAPSHOT.write().unwrap();
        if snap.matches.is_empty() {
            let now = Utc::now();
            snap.matches = demo_matches(now);
            snap.fetched_at = now;
            snap.stale = false;
            snap.using_fixture = true;
        }
        return;
    };

    tokio::spawn(async move {
        let client = match reqwest::Client::builder()
            .user_agent("plaintextesports/0.1 (+https://github.com/ralphpotato/plaintextesports)")
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                leptos::logging::log!("failed to build http client: {e}");
                return;
            }
        };

        loop {
            // Fetch (network) first, then apply synchronously so we never hold
            // the (non-Sync) DB connection across an await point.
            let (cs_res, lol_res) = tokio::join!(
                fetch_game(&client, &token, Game::Cs2),
                fetch_game(&client, &token, Game::Lol),
            );
            apply_poll(cs_res, lol_res, store.as_mut());
            tokio::time::sleep(next_poll_delay(&cfg)).await;
        }
    });
}

/// Choose the next poll delay: the short `active_poll` when any match is live
/// (started within the last few hours and not finished) or about to start,
/// otherwise the generous `idle_poll`. This keeps us light during downtime and
/// responsive around live events without ever approaching the rate limit.
/// True when any match is live (started within `grace` and not finished) or
/// starts within `lead`. `grace` bounds how long a match the free tier never
/// marks "finished" keeps us in the fast cadence.
fn is_active_window(
    matches: &[NormalizedMatch],
    now: DateTime<Utc>,
    grace: Duration,
    lead: Duration,
) -> bool {
    matches.iter().any(|m| {
        let live = m.begin_at <= now
            && now < m.begin_at + grace
            && !matches!(m.status, MatchStatus::Finished | MatchStatus::Canceled);
        let imminent = m.begin_at > now && m.begin_at <= now + lead;
        live || imminent
    })
}

fn next_poll_delay(cfg: &Config) -> std::time::Duration {
    let now = Utc::now();
    let active = {
        let snap = SNAPSHOT.read().unwrap();
        is_active_window(&snap.matches, now, Duration::hours(5), Duration::minutes(15))
    };
    let delay = if active { cfg.active_poll } else { cfg.idle_poll };
    leptos::logging::log!("next poll in {}s (active={active})", delay.as_secs());
    delay
}

/// Persist freshly polled matches (if a store is present) and refresh the
/// in-memory snapshot. Synchronous — no `.await` while touching the DB.
fn apply_poll(cs_res: FetchResult, lol_res: FetchResult, store: Option<&mut rusqlite::Connection>) {
    let now = Utc::now();
    let mut fresh: Vec<NormalizedMatch> = Vec::new();
    let mut errored: Vec<Game> = Vec::new();
    let mut any_ok = false;

    for (game, res) in [(Game::Cs2, cs_res), (Game::Lol, lol_res)] {
        match res {
            Ok(mut v) => {
                any_ok = true;
                fresh.append(&mut v);
            }
            Err(e) => {
                errored.push(game);
                leptos::logging::log!("poll failed for {}: {e}", game.slug());
            }
        }
    }
    let any_err = !errored.is_empty();

    if let Some(conn) = store {
        // Upsert successes; matches for an errored game stay in the DB (we only
        // prune by age), so the reload below preserves them.
        if any_ok {
            let cutoff = (now - Duration::days(RETENTION_DAYS)).timestamp_millis();
            if let Err(e) = crate::store::upsert_and_prune(conn, &fresh, now.timestamp_millis(), cutoff)
            {
                leptos::logging::log!("cache db write failed: {e}");
            }
        }
        match crate::store::load_all(conn) {
            Ok(mut all) => {
                all.sort_by_key(|m| m.begin_at);
                let mut snap = SNAPSHOT.write().unwrap();
                let n = all.len();
                snap.matches = all;
                snap.using_fixture = false;
                snap.stale = any_err;
                if any_ok {
                    snap.fetched_at = now;
                }
                leptos::logging::log!("poll complete: {n} matches in cache (stale={any_err})");
            }
            Err(e) => {
                leptos::logging::log!("cache db read failed: {e}");
                merge_in_memory(fresh, &errored, any_ok, any_err, now);
            }
        }
    } else {
        merge_in_memory(fresh, &errored, any_ok, any_err, now);
    }
}

/// Memory-only update path (no DB): replace successful games' matches and keep
/// the previous snapshot's matches for any game that failed this round.
fn merge_in_memory(
    mut matches: Vec<NormalizedMatch>,
    errored: &[Game],
    any_ok: bool,
    any_err: bool,
    now: DateTime<Utc>,
) {
    let mut snap = SNAPSHOT.write().unwrap();
    for &game in errored {
        matches.extend(snap.matches.iter().filter(|m| m.game == game).cloned());
    }
    matches.sort_by_key(|m| m.begin_at);
    snap.matches = matches;
    snap.using_fixture = false;
    snap.stale = any_err;
    if any_ok {
        snap.fetched_at = now;
    }
}

// ----- View building -------------------------------------------------------

fn local_day_start(tz: &Tz, date: NaiveDate) -> DateTime<Utc> {
    let naive = date.and_hms_opt(0, 0, 0).expect("valid midnight");
    tz.from_local_datetime(&naive)
        .earliest()
        .map_or_else(|| Utc.from_utc_datetime(&naive), |dt| dt.with_timezone(&Utc))
}

fn time_label(local: DateTime<Tz>) -> String {
    let h24 = local.hour();
    let (h12, ampm) = match h24 {
        0 => (12, "AM"),
        1..=11 => (h24, "AM"),
        12 => (12, "PM"),
        _ => (h24 - 12, "PM"),
    };
    format!("{h12}:{:02} {ampm}", local.minute())
}

fn day_label(local: DateTime<Tz>) -> String {
    format!(
        "{}, {} {}",
        local.format("%A"),
        local.format("%B"),
        local.day()
    )
}

/// Resolve the effective view status, applying the "started but not marked
/// finished" live heuristic (the free tier has no real-time feed).
fn effective_status(m: &NormalizedMatch, now: DateTime<Utc>) -> MatchStatus {
    match m.status {
        MatchStatus::Upcoming if m.begin_at <= now => MatchStatus::Live,
        other => other,
    }
}

fn status_label(status: MatchStatus, local: DateTime<Tz>) -> String {
    match status {
        MatchStatus::Live => "LIVE".to_string(),
        MatchStatus::Finished => "Final".to_string(),
        MatchStatus::Canceled => "Canceled".to_string(),
        MatchStatus::Upcoming => time_label(local),
    }
}

fn to_view(m: &NormalizedMatch, tz: &Tz, now: DateTime<Utc>) -> MatchView {
    let local = m.begin_at.with_timezone(tz);
    let status = effective_status(m, now);

    let mut team_a = TeamView {
        label: m.team_a.label.clone(),
        score: m.team_a.score,
        winner: false,
    };
    let mut team_b = TeamView {
        label: m.team_b.label.clone(),
        score: m.team_b.score,
        winner: false,
    };
    if status == MatchStatus::Finished {
        if let (Some(a), Some(b)) = (team_a.score, team_b.score) {
            team_a.winner = a > b;
            team_b.winner = b > a;
        }
    }

    MatchView {
        id: m.id,
        game: m.game,
        league: m.league.clone(),
        tier: m.tier.clone(),
        status,
        time_label: status_label(status, local),
        best_of: m.best_of.map(|n| format!("Bo{n}")).unwrap_or_default(),
        team_a,
        team_b,
        stream_url: m.stream_url.clone(),
        begin_at_ms: m.begin_at.timestamp_millis(),
    }
}

/// Group already-sorted views into per-day buckets keyed by the local date.
fn group_by_day(views: Vec<MatchView>, tz: &Tz) -> Vec<DayGroup> {
    let mut days: Vec<DayGroup> = Vec::new();
    for v in views {
        let local = DateTime::from_timestamp_millis(v.begin_at_ms)
            .unwrap_or_else(Utc::now)
            .with_timezone(tz);
        let key = local.format("%Y-%m-%d").to_string();
        if days.last().map(|d| &d.day_key) != Some(&key) {
            days.push(DayGroup {
                day_key: key,
                day_label: day_label(local),
                matches: Vec::new(),
            });
        }
        days.last_mut().unwrap().matches.push(v);
    }
    days
}

fn matches_in_window(
    snap: &Snapshot,
    game: Option<Game>,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    tz: &Tz,
    now: DateTime<Utc>,
) -> Vec<MatchView> {
    snap.matches
        .iter()
        .filter(|m| game.is_none_or(|g| m.game == g))
        .filter(|m| m.begin_at >= start && m.begin_at < end)
        .map(|m| to_view(m, tz, now))
        .collect()
}

/// Homepage: live matches pinned, then the next `upcoming_days` grouped by day.
#[must_use]
pub fn homepage_view(game_filter: &str) -> ScheduleView {
    let cfg = Config::from_env();
    let game = Game::from_filter(game_filter);
    let now = Utc::now();
    let start = local_day_start(&cfg.tz, now.with_timezone(&cfg.tz).date_naive());
    let end = now + Duration::days(cfg.upcoming_days);

    let snap = SNAPSHOT.read().unwrap();
    let all = matches_in_window(&snap, game, start, end, &cfg.tz, now);

    let (live, upcoming): (Vec<_>, Vec<_>) =
        all.into_iter().partition(|m| m.status == MatchStatus::Live);

    ScheduleView {
        live,
        days: group_by_day(upcoming, &cfg.tz),
        fetched_at_ms: snap.fetched_at.timestamp_millis(),
        fetched_label: time_label(snap.fetched_at.with_timezone(&cfg.tz)),
        stale: snap.stale,
        using_fixture: snap.using_fixture,
        date_label: None,
        prev_date: None,
        next_date: None,
    }
}

/// Single-day view with prev/next navigation.
#[must_use]
pub fn day_view(date: &str, game_filter: &str) -> ScheduleView {
    let cfg = Config::from_env();
    let game = Game::from_filter(game_filter);
    let now = Utc::now();

    let day = NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .unwrap_or_else(|_| now.with_timezone(&cfg.tz).date_naive());
    let start = local_day_start(&cfg.tz, day);
    let end = local_day_start(&cfg.tz, day + Duration::days(1));

    let snap = SNAPSHOT.read().unwrap();
    let all = matches_in_window(&snap, game, start, end, &cfg.tz, now);
    let (live, rest): (Vec<_>, Vec<_>) =
        all.into_iter().partition(|m| m.status == MatchStatus::Live);

    let local_noon = cfg
        .tz
        .from_local_datetime(&day.and_hms_opt(12, 0, 0).unwrap())
        .earliest()
        .unwrap_or_else(|| cfg.tz.from_utc_datetime(&day.and_hms_opt(12, 0, 0).unwrap()));

    ScheduleView {
        live,
        days: group_by_day(rest, &cfg.tz),
        fetched_at_ms: snap.fetched_at.timestamp_millis(),
        fetched_label: time_label(snap.fetched_at.with_timezone(&cfg.tz)),
        stale: snap.stale,
        using_fixture: snap.using_fixture,
        date_label: Some(day_label(local_noon)),
        prev_date: Some((day - Duration::days(1)).format("%Y-%m-%d").to_string()),
        next_date: Some((day + Duration::days(1)).format("%Y-%m-%d").to_string()),
    }
}

// ----- Demo fixture (no token) ---------------------------------------------

fn demo_team(label: &str, score: Option<i64>) -> NormTeam {
    NormTeam {
        label: label.to_string(),
        score,
    }
}

#[allow(clippy::too_many_arguments)]
fn demo_match(
    id: i64,
    game: Game,
    league: &str,
    tier: &str,
    begin_at: DateTime<Utc>,
    status: MatchStatus,
    best_of: i64,
    a: NormTeam,
    b: NormTeam,
) -> NormalizedMatch {
    NormalizedMatch {
        id,
        game,
        league: league.to_string(),
        tier: tier.to_string(),
        begin_at,
        status,
        best_of: Some(best_of),
        team_a: a,
        team_b: b,
        stream_url: Some("https://www.twitch.tv/".to_string()),
    }
}

/// Demo data anchored to `now` so the page is populated without a token.
fn demo_matches(now: DateTime<Utc>) -> Vec<NormalizedMatch> {
    let h = Duration::hours;
    let d = Duration::days;
    vec![
        demo_match(
            1,
            Game::Cs2,
            "IEM",
            "S",
            now - h(1),
            MatchStatus::Live,
            3,
            demo_team("NAVI", Some(1)),
            demo_team("FaZe", Some(0)),
        ),
        demo_match(
            2,
            Game::Lol,
            "LCK",
            "A",
            now - h(2),
            MatchStatus::Finished,
            3,
            demo_team("T1", Some(2)),
            demo_team("GEN", Some(1)),
        ),
        demo_match(
            3,
            Game::Lol,
            "LCK",
            "A",
            now + h(3),
            MatchStatus::Upcoming,
            3,
            demo_team("HLE", None),
            demo_team("DK", None),
        ),
        demo_match(
            4,
            Game::Cs2,
            "BLAST",
            "S",
            now + h(5),
            MatchStatus::Upcoming,
            3,
            demo_team("VIT", None),
            demo_team("SPIRIT", None),
        ),
        demo_match(
            5,
            Game::Lol,
            "LEC",
            "A",
            now + d(1),
            MatchStatus::Upcoming,
            3,
            demo_team("G2", None),
            demo_team("FNC", None),
        ),
        demo_match(
            6,
            Game::Cs2,
            "BLAST",
            "S",
            now + d(1) + h(2),
            MatchStatus::Upcoming,
            1,
            demo_team("MOUZ", None),
            demo_team("G2", None),
        ),
        demo_match(
            7,
            Game::Lol,
            "Worlds",
            "S",
            now + d(3),
            MatchStatus::Upcoming,
            5,
            demo_team("T1", None),
            demo_team("BLG", None),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(begin: DateTime<Utc>, status: MatchStatus) -> NormalizedMatch {
        NormalizedMatch {
            id: 1,
            game: Game::Cs2,
            league: "X".into(),
            tier: "S".into(),
            begin_at: begin,
            status,
            best_of: Some(3),
            team_a: NormTeam {
                label: "A".into(),
                score: None,
            },
            team_b: NormTeam {
                label: "B".into(),
                score: None,
            },
            stream_url: None,
        }
    }

    #[test]
    fn active_window_detection() {
        let now = Utc::now();
        let grace = Duration::hours(5);
        let lead = Duration::minutes(15);
        let active = |m: NormalizedMatch| is_active_window(&[m], now, grace, lead);

        assert!(!is_active_window(&[], now, grace, lead), "nothing scheduled");
        assert!(
            active(at(now - Duration::hours(1), MatchStatus::Upcoming)),
            "started 1h ago, not finished => live"
        );
        assert!(
            !active(at(now - Duration::hours(1), MatchStatus::Finished)),
            "finished match is not active"
        );
        assert!(
            !active(at(now - Duration::hours(6), MatchStatus::Upcoming)),
            "started 6h ago => past grace, assume over"
        );
        assert!(
            active(at(now + Duration::minutes(10), MatchStatus::Upcoming)),
            "starts in 10 min => imminent"
        );
        assert!(
            !active(at(now + Duration::hours(2), MatchStatus::Upcoming)),
            "starts in 2h => not yet active"
        );
    }
}
