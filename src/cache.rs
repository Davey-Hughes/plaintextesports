//! In-memory schedule cache + background poller + view builders (server-only).
//!
//! A single background task polls PandaScore on an interval and stores a
//! normalized, tier-1-filtered snapshot behind an `RwLock`. Page requests read
//! the snapshot and format it into [`ScheduleView`] for the requested timezone —
//! they never touch the network. When no API token is configured we populate a
//! demo fixture (relative to "now") so the UI is always usable.

use crate::config::config;
use crate::pandascore::{fetch_game, FetchResult, NormTeam, NormalizedMatch};
use crate::types::{
    event_name_eq, full_event_name, BracketMatch, BracketRound, DayGroup, EventInfo, Game,
    LeagueGroup, MatchStatus, MatchView, ScheduleView, StandingRow, StreamView, TeamView,
};
use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::{PoisonError, RwLock};

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

/// A resolved (or attempted) Liquipedia event link. `url: None` means we looked
/// and found no confident match (cached so we don't keep re-querying).
#[derive(Clone)]
struct EventLink {
    url: Option<String>,
    checked_at: DateTime<Utc>,
}

/// Cache of event-page links keyed by `event_key` (game|league|year). Read on
/// every render (to_view), written by the poll-time resolver.
static EVENT_LINKS: Lazy<RwLock<HashMap<String, EventLink>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

/// Re-query a "not found" event link only after this many days (catches pages
/// created after an event starts, without hammering Liquipedia).
const NEG_TTL_DAYS: i64 = 3;
/// Cap Liquipedia lookups per poll cycle (the rest resolve next cycle).
const MAX_RESOLVE_PER_CYCLE: usize = 10;

fn event_key(game: Game, league: &str, year: i32) -> String {
    format!("{}|{}|{}", game.slug(), league, year)
}

/// Per-tournament standings + bracket, fetched on demand and cached with a TTL.
static EVENTS: Lazy<RwLock<HashMap<i64, CachedEvent>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

/// Current MLB division standings (six tables), refreshed by the poller from the
/// keyless MLB Stats API. Empty until the first successful fetch.
static MLB_STANDINGS: Lazy<RwLock<Vec<EventInfo>>> = Lazy::new(|| RwLock::new(Vec::new()));

/// The MLB division standings (all six divisions), for the `/event/MLB` page.
pub fn mlb_standings() -> Vec<EventInfo> {
    MLB_STANDINGS
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// The standings tables for the divisions of `team_a`/`team_b` (one or two
/// tables), for an MLB match page. Order follows the league-wide ordering.
pub fn mlb_team_divisions(team_a: &str, team_b: &str) -> Vec<EventInfo> {
    MLB_STANDINGS
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .filter(|div| {
            div.standings
                .iter()
                .any(|r| r.team == team_a || r.team == team_b)
        })
        .cloned()
        .collect()
}

/// Per-game MLB series, fetched on demand (detail page) and cached with a short
/// TTL keyed by "gamePk|tz|hour24" (the tz/12h-24h pref affects the game times),
/// so repeated page views don't re-hit the MLB API.
static MLB_SERIES: Lazy<RwLock<HashMap<String, CachedSeries>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

struct CachedSeries {
    series: crate::types::Series,
    fetched_at: DateTime<Utc>,
}

/// Re-fetch a game's series at most this often. Short enough that a live series'
/// scores stay fresh, long enough that a burst of views is one fetch.
const SERIES_TTL_MIN: i64 = 2;

/// The MLB series between the two teams of `match_id`, fetched on demand and
/// cached with a short TTL. Each game's time is formatted in the display tz
/// (`tz_name`/`hour24`), so the cache key folds those in. `None` for esports, an
/// unknown match, an MLB match with no series ref, or when the fetch fails (the
/// detail page then omits the section). The snapshot read (for the ref + labels)
/// is released before the await so the future stays `Send`.
pub async fn mlb_series(match_id: i64, tz_name: &str, hour24: bool) -> Option<crate::types::Series> {
    let cfg = config();
    let tz = resolve_tz(tz_name, cfg.tz);
    // Fold the display-tz preference into the key, since the clock labels differ.
    let key = format!("{match_id}|{tz}|{hour24}");
    // Serve a fresh cached series without touching the network.
    {
        let cache = MLB_SERIES.read().unwrap_or_else(PoisonError::into_inner);
        if let Some(c) = cache.get(&key) {
            if Utc::now() - c.fetched_at < Duration::minutes(SERIES_TTL_MIN) {
                return Some(c.series.clone());
            }
        }
    }
    // Pull the series ref + headline orientation from the snapshot, then drop the
    // lock before any await.
    let (sref, team_a, team_b, begin_at) = {
        let snap = SNAPSHOT.read().unwrap_or_else(PoisonError::into_inner);
        let m = snap.matches.iter().find(|m| m.id == match_id)?;
        let sref = m.mlb_series?;
        (sref, m.team_a.label.clone(), m.team_b.label.clone(), m.begin_at)
    };
    // Build each game's labels: the date + time in the viewer's tz (always
    // mutually consistent), and the same instant in the ballpark's tz as a full
    // "date · time tz" for the venue toggle, which *swaps* the label. Both states
    // carry a tz abbreviation so the swap clearly reads as two timezones (and you
    // always know which one you're looking at). The venue label is omitted when
    // the venue tz is unknown or shows the very same wall-clock as the viewer's.
    let fmt_labels = move |utc: DateTime<Utc>, venue_tz: Option<&str>| {
        let local = utc.with_timezone(&tz);
        let venue = venue_tz
            .and_then(|id| id.parse::<Tz>().ok())
            .map(|vtz| utc.with_timezone(&vtz))
            .filter(|vlocal| {
                // Only worth swapping to when it differs from the viewer's clock.
                vlocal.naive_local() != local.naive_local()
            })
            .map(|vlocal| {
                format!(
                    "{} · {} {}",
                    short_day_label(vlocal),
                    time_label(vlocal, hour24),
                    vlocal.format("%Z"),
                )
            })
            .unwrap_or_default();
        // Suffix the viewer's tz abbreviation onto the local time *only* when a
        // venue swap is available, so the two states are symmetric (each names its
        // zone) without cluttering rows that have no venue toggle.
        let clock = if venue.is_empty() {
            time_label(local, hour24)
        } else {
            format!("{} {}", time_label(local, hour24), local.format("%Z"))
        };
        crate::mlb::SeriesLabels {
            day: short_day_label(local),
            clock,
            venue,
        }
    };
    match crate::mlb::fetch_series(&HTTP, match_id, begin_at, &team_a, &team_b, sref, fmt_labels)
        .await
    {
        Ok(series) if !series.games.is_empty() => {
            MLB_SERIES.write().unwrap_or_else(PoisonError::into_inner).insert(
                key,
                CachedSeries { series: series.clone(), fetched_at: Utc::now() },
            );
            Some(series)
        }
        Ok(_) => None,
        Err(e) => {
            leptos::logging::log!("MLB series fetch failed ({match_id}): {e}");
            None
        }
    }
}

struct CachedEvent {
    info: EventInfo,
    fetched_at: DateTime<Utc>,
}

/// Re-fetch a tournament's standings/bracket at most this often.
const EVENT_TTL_MIN: i64 = 15;

/// The poller pre-warms an active event's standings/bracket once it's older than
/// this (below the serve TTL, so a page request always finds a hot entry).
const PREWARM_TTL_MIN: i64 = 11;

/// Cap the pre-warm fetches per poll cycle so a burst of new events can't blow
/// the API budget; the rest catch up over the next cycles.
const MAX_PREWARM_PER_CYCLE: usize = 8;

/// Shared HTTP client for on-demand standings/bracket fetches.
static HTTP: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .user_agent("plaintextesports/0.1 (+https://github.com/ralphpotato/plaintextesports)")
        .build()
        .unwrap_or_default()
});


/// Start the background poller. Loads any persisted matches first (so a restart
/// serves data immediately), then either polls PandaScore or, with no token,
/// falls back to demo data.
pub fn spawn_poller() {
    let cfg = config();

    // DEMO mode: serve fixture data regardless of token/DB (and don't poll).
    if cfg.demo {
        leptos::logging::log!("DEMO mode — serving fixture data (ignoring token + cache db)");
        let now = Utc::now();
        let demo = demo_matches(now);
        seed_demo_events(&demo, now);
        let mut snap = SNAPSHOT.write().unwrap_or_else(PoisonError::into_inner);
        snap.matches = demo;
        snap.fetched_at = now;
        snap.stale = false;
        snap.using_fixture = true;
        return;
    }

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
                let mut snap = SNAPSHOT.write().unwrap_or_else(PoisonError::into_inner);
                let n = stored.len();
                snap.matches = stored;
                snap.fetched_at = fetched;
                snap.stale = true; // refreshed by the first live poll
                snap.using_fixture = false;
                leptos::logging::log!("loaded {n} matches from cache db");
            }
        }
        // Load cached event-page links.
        if let Ok(rows) = crate::store::load_event_links(conn) {
            let mut map = EVENT_LINKS.write().unwrap_or_else(PoisonError::into_inner);
            for (key, url, checked_at_ms) in rows {
                let checked_at =
                    DateTime::from_timestamp_millis(checked_at_ms).unwrap_or_else(Utc::now);
                map.insert(key, EventLink { url, checked_at });
            }
        }
    }

    let Some(token) = cfg.token.clone() else {
        leptos::logging::log!("PANDASCORE_TOKEN not set — serving demo fixture data");
        let mut snap = SNAPSHOT.write().unwrap_or_else(PoisonError::into_inner);
        if snap.matches.is_empty() {
            let now = Utc::now();
            let demo = demo_matches(now);
            seed_demo_events(&demo, now);
            snap.matches = demo;
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

        let mut last_deep: Option<DateTime<Utc>> = None;
        // Last-seen API budget, used to throttle low-priority past work.
        let mut rate_remaining: Option<u64> = None;
        loop {
            let now = Utc::now();
            let active = {
                let snap = SNAPSHOT.read().unwrap_or_else(PoisonError::into_inner);
                is_active_window(&snap.matches, now, Duration::hours(5), Duration::minutes(15))
            };
            // Always deep-scan when idle; while active, deep-scan only every
            // `idle_poll` so a long live event still discovers far-out matches
            // without paginating the whole window every minute.
            let deep_interval = Duration::seconds(cfg.idle_poll.as_secs() as i64);
            let deep = !active || last_deep.is_none_or(|t| now - t >= deep_interval);
            if deep {
                last_deep = Some(now);
            }
            let window_end = now + Duration::days(cfg.upcoming_days);

            // Fetch (network) first, then apply synchronously so we never hold
            // the (non-Sync) DB connection across an await point. MLB comes from
            // its own (keyless) API and joins the same apply path.
            // Keep about a week of past MLB days so "show earlier days" has
            // something to reveal; the display still defaults to today onward.
            let mlb_window = ((now - Duration::days(8)).date_naive(), window_end.date_naive());
            // F1 comes from its own keyless API (Jolpica); the whole season's
            // sessions are fetched and the snapshot windowing decides what shows.
            let (cs_res, lol_res, mlb_raw, mlb_standings_raw, f1_raw) = tokio::join!(
                fetch_game(&client, &token, Game::Cs2, window_end, deep),
                fetch_game(&client, &token, Game::Lol, window_end, deep),
                crate::mlb::fetch_schedule(&client, mlb_window.0, mlb_window.1),
                crate::mlb::fetch_standings(&client, now.year()),
                crate::f1::fetch_schedule(&client, now.year()),
            );
            let mlb_res: FetchResult = mlb_raw.map_err(Into::into);
            let f1_res: FetchResult = f1_raw.map_err(Into::into);
            apply_poll(
                vec![
                    (Game::Cs2, cs_res),
                    (Game::Lol, lol_res),
                    (Game::Mlb, mlb_res),
                    (Game::F1, f1_res),
                ],
                store.as_mut(),
                cfg.archive_cutoff(now).timestamp_millis(),
            );
            // Refresh the MLB standings cache (keep the old tables on error or an
            // empty off-season response, rather than blanking the page).
            match mlb_standings_raw {
                Ok(divs) if !divs.is_empty() => {
                    *MLB_STANDINGS.write().unwrap_or_else(PoisonError::into_inner) = divs;
                }
                Ok(_) => {}
                Err(e) => leptos::logging::log!("MLB standings fetch failed: {e}"),
            }

            // Resolve exact Liquipedia links for any new events. The async
            // lookups hold no DB connection; persistence happens synchronously
            // after, so the future stays Send.
            if cfg.resolve_links {
                let candidates = collect_resolution_candidates();
                if !candidates.is_empty() {
                    let mut results: Vec<(String, Option<String>)> = Vec::new();
                    for c in candidates.into_iter().take(MAX_RESOLVE_PER_CYCLE) {
                        match crate::liquipedia::resolve_event(&client, c.game, &c.league, c.year)
                            .await
                        {
                            Ok(url) => results.push((c.key, url)),
                            Err(e) => leptos::logging::log!(
                                "liquipedia resolve failed ({} {}): {e}",
                                c.league,
                                c.year
                            ),
                        }
                        // Respect Liquipedia's API rate limit (~1 req / 2s).
                        tokio::time::sleep(std::time::Duration::from_millis(2100)).await;
                    }
                    apply_resolutions(&results, store.as_mut(), Utc::now());
                }
            }

            // Keep recent/upcoming events' standings + brackets hot so an event
            // page is a cache hit (~1ms) rather than a cold PandaScore fetch
            // (~800ms) during server render. Bounded + budget-gated.
            if headroom_ok(rate_remaining, cfg.rate_limit_floor) {
                prewarm_active(now).await;
            }

            // Low-priority past-day work: only when nothing is live/imminent and
            // the API budget has headroom, so current/future polling comes first.
            if !active && headroom_ok(rate_remaining, cfg.rate_limit_floor) {
                let cutoff_ms = cfg.archive_cutoff(now).timestamp_millis();

                // 1. Refresh one overdue recent-past day (≤1 request).
                if cfg.past_refresh {
                    let bucket = store.as_ref().and_then(|c| next_due_refresh_bucket(c, now));
                    if let Some((game, day)) = bucket {
                        let (from, to) = utc_day_bounds(day);
                        match crate::pandascore::fetch_past_range_all(&client, &token, game, from, to)
                            .await
                        {
                            Ok((matches, rate)) => {
                                rate_remaining = rate.or(rate_remaining);
                                apply_past(matches, store.as_mut(), cutoff_ms);
                                if let Some(conn) = store.as_ref() {
                                    let _ = crate::store::set_meta(
                                        conn,
                                        &refresh_key(game, day),
                                        &now.timestamp_millis().to_string(),
                                    );
                                }
                            }
                            Err(e) => {
                                leptos::logging::log!("past-refresh failed ({} {day}): {e}", game.slug());
                            }
                        }
                    }
                }

                // 2. Backfill one ~7-day chunk backwards (both games), if not done.
                if cfg.backfill && headroom_ok(rate_remaining, cfg.rate_limit_floor) {
                    let archive = cfg.archive_cutoff(now);
                    // Compute the next chunk from the persisted cursor (borrow released
                    // before the await): None ⇒ already done, Some(from,to,last).
                    let chunk = store.as_ref().and_then(|conn| {
                        if crate::store::get_meta(conn, "backfill_done").as_deref() == Some("1") {
                            return None;
                        }
                        let cursor = crate::store::get_meta(conn, "backfill_cursor_ms")
                            .and_then(|s| s.parse::<i64>().ok())
                            .and_then(DateTime::from_timestamp_millis)
                            .unwrap_or(now);
                        Some(if cursor <= archive {
                            None // reached horizon → mark done below
                        } else {
                            let from = (cursor - Duration::days(7)).max(archive);
                            Some((from, cursor, from <= archive))
                        })
                    });
                    match chunk {
                        Some(Some((from, to, last))) => {
                            let (cs, lol) = tokio::join!(
                                crate::pandascore::fetch_past_range_all(&client, &token, Game::Cs2, from, to),
                                crate::pandascore::fetch_past_range_all(&client, &token, Game::Lol, from, to),
                            );
                            let mut got = Vec::new();
                            for r in [cs, lol] {
                                match r {
                                    Ok((matches, rate)) => {
                                        rate_remaining = rate.or(rate_remaining);
                                        got.extend(matches);
                                    }
                                    Err(e) => leptos::logging::log!("backfill fetch failed: {e}"),
                                }
                            }
                            apply_past(got, store.as_mut(), cutoff_ms);
                            if let Some(conn) = store.as_ref() {
                                let _ = crate::store::set_meta(
                                    conn,
                                    "backfill_cursor_ms",
                                    &from.timestamp_millis().to_string(),
                                );
                                if last {
                                    let _ = crate::store::set_meta(conn, "backfill_done", "1");
                                }
                            }
                            leptos::logging::log!("backfilled {} … {}", from.date_naive(), to.date_naive());
                        }
                        Some(None) => {
                            if let Some(conn) = store.as_ref() {
                                let _ = crate::store::set_meta(conn, "backfill_done", "1");
                            }
                        }
                        None => {}
                    }
                }
            }

            let delay = if active { cfg.active_poll } else { cfg.idle_poll };
            leptos::logging::log!("next poll in {}s (active={active}, deep={deep})", delay.as_secs());
            tokio::time::sleep(delay).await;
        }
    });
}

/// True when any match is live (started within `grace` and not finished) or
/// starts within `lead`. Drives both the fast/slow cadence and shallow/deep
/// fetch depth. `grace` bounds how long a match the free tier never marks
/// "finished" keeps us in the fast path.
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

/// Persist freshly polled matches (if a store is present) and refresh the
/// in-memory snapshot. Synchronous — no `.await` while touching the DB.
fn apply_poll(
    results: Vec<(Game, FetchResult)>,
    store: Option<&mut rusqlite::Connection>,
    cutoff_ms: i64,
) {
    let now = Utc::now();
    let mut fresh: Vec<NormalizedMatch> = Vec::new();
    let mut errored: Vec<Game> = Vec::new();
    let mut any_ok = false;

    for (game, res) in results {
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
            if let Err(e) =
                crate::store::upsert_and_prune(conn, &fresh, now.timestamp_millis(), cutoff_ms)
            {
                leptos::logging::log!("cache db write failed: {e}");
            }
        }
        match crate::store::load_all(conn) {
            Ok(mut all) => {
                all.sort_by_key(|m| m.begin_at);
                // Streams/broadcasts and the MLB series ref aren't persisted to
                // the DB, so the reload drops them; re-attach them from this
                // poll's fresh fetch (keyed by match id) so the detail page still
                // has them.
                let mut live: HashMap<i64, (Vec<StreamView>, Option<crate::types::MlbSeriesRef>)> =
                    fresh
                        .into_iter()
                        .filter(|m| !m.streams.is_empty() || m.mlb_series.is_some())
                        .map(|m| (m.id, (m.streams, m.mlb_series)))
                        .collect();
                for m in &mut all {
                    if let Some((s, series)) = live.remove(&m.id) {
                        if !s.is_empty() {
                            m.streams = s;
                        }
                        m.mlb_series = series;
                    }
                }
                let mut snap = SNAPSHOT.write().unwrap_or_else(PoisonError::into_inner);
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
                merge_in_memory(fresh, &errored, any_ok, any_err, now, cutoff_ms);
            }
        }
    } else {
        merge_in_memory(fresh, &errored, any_ok, any_err, now, cutoff_ms);
    }
}

/// Memory-only update path (no DB): replace successful games' matches and keep
/// the previous snapshot's matches for any game that failed this round.
/// Merge freshly fetched matches with the previous set, keeping the previous
/// matches for any game that failed this round. Sorted by start time.
fn merge_matches(
    prev: &[NormalizedMatch],
    mut fresh: Vec<NormalizedMatch>,
    errored: &[Game],
    cutoff_ms: i64,
) -> Vec<NormalizedMatch> {
    for &game in errored {
        fresh.extend(prev.iter().filter(|m| m.game == game).cloned());
    }
    // Mirror the DB prune so memory-only mode doesn't accumulate stale matches.
    fresh.retain(|m| m.begin_at.timestamp_millis() >= cutoff_ms);
    fresh.sort_by_key(|m| m.begin_at);
    fresh
}

fn merge_in_memory(
    matches: Vec<NormalizedMatch>,
    errored: &[Game],
    any_ok: bool,
    any_err: bool,
    now: DateTime<Utc>,
    cutoff_ms: i64,
) {
    let mut snap = SNAPSHOT.write().unwrap_or_else(PoisonError::into_inner);
    let merged = merge_matches(&snap.matches, matches, errored, cutoff_ms);
    snap.matches = merged;
    snap.using_fixture = false;
    snap.stale = any_err;
    if any_ok {
        snap.fetched_at = now;
    }
}

// ----- Past-day refresh + backfill -----------------------------------------

/// Upsert a batch of past matches, prune by cutoff, and reload the snapshot —
/// the success tail of `apply_poll` for the single-purpose past fetches. Does
/// not touch `fetched_at`/`stale` (those track the live current/future poll).
fn apply_past(matches: Vec<NormalizedMatch>, store: Option<&mut rusqlite::Connection>, cutoff_ms: i64) {
    if matches.is_empty() {
        return;
    }
    // Persist (best effort), then merge the small batch into the in-memory
    // snapshot in place — no full table reload (the snapshot is already in sync
    // from the main poll's load_all this same cycle).
    if let Some(conn) = store {
        let now = Utc::now();
        if let Err(e) =
            crate::store::upsert_and_prune(conn, &matches, now.timestamp_millis(), cutoff_ms)
        {
            leptos::logging::log!("past write failed: {e}");
            return;
        }
    }
    let replaced: std::collections::HashSet<(i64, Game)> =
        matches.iter().map(|m| (m.id, m.game)).collect();
    let mut snap = SNAPSHOT.write().unwrap_or_else(PoisonError::into_inner);
    snap.matches.retain(|m| {
        !replaced.contains(&(m.id, m.game)) && m.begin_at.timestamp_millis() >= cutoff_ms
    });
    snap.matches
        .extend(matches.into_iter().filter(|m| m.begin_at.timestamp_millis() >= cutoff_ms));
    snap.matches.sort_by_key(|m| m.begin_at);
}

/// How often to re-fetch a past day's scores, by age in whole days vs today
/// (UTC). `None` = never: age 0 (today) is the main poll's job, and matches over
/// a week old are locked (scores no longer change, frees budget for backfill).
fn refresh_interval(age_days: i64) -> Option<Duration> {
    match age_days {
        1 => Some(Duration::hours(6)),
        2 => Some(Duration::hours(12)),
        3 => Some(Duration::hours(24)),
        4..=7 => Some(Duration::hours(48)),
        _ => None,
    }
}

/// Enough API budget left for low-priority past work? Unknown (`None`) ⇒ yes
/// (the main poll just succeeded, so headroom exists).
fn headroom_ok(remaining: Option<u64>, floor: u64) -> bool {
    remaining.is_none_or(|r| r >= floor)
}

/// UTC calendar-day bounds: `[00:00, next 00:00)`.
fn utc_day_bounds(day: NaiveDate) -> (DateTime<Utc>, DateTime<Utc>) {
    let start = day.and_hms_opt(0, 0, 0).unwrap().and_utc();
    (start, start + Duration::days(1))
}

fn refresh_key(game: Game, day: NaiveDate) -> String {
    format!("past_refresh:{}:{}", game.slug(), day.format("%Y-%m-%d"))
}

/// The single most-overdue past `(game, day)` bucket due for a score refresh,
/// or `None`. Only buckets that actually have matches in the snapshot and fall
/// in the 1–7-day refresh window are considered.
fn next_due_refresh_bucket(conn: &rusqlite::Connection, now: DateTime<Utc>) -> Option<(Game, NaiveDate)> {
    let today = now.date_naive();
    let buckets: Vec<(Game, NaiveDate)> = {
        let snap = SNAPSHOT.read().unwrap_or_else(PoisonError::into_inner);
        let mut set = std::collections::HashSet::new();
        for m in &snap.matches {
            // Past-refresh re-fetches a day from PandaScore; traditional sports
            // (MLB) come from their own feed and have no PandaScore endpoint.
            if m.game.traditional() {
                continue;
            }
            let day = m.begin_at.date_naive();
            if (1..=7).contains(&(today - day).num_days()) {
                set.insert((m.game, day));
            }
        }
        set.into_iter().collect()
    };
    let mut best: Option<(Game, NaiveDate, i64)> = None;
    for (game, day) in buckets {
        let Some(interval) = refresh_interval((today - day).num_days()) else {
            continue;
        };
        let overdue = match crate::store::get_meta(conn, &refresh_key(game, day))
            .and_then(|s| s.parse::<i64>().ok())
        {
            None => i64::MAX, // never refreshed → top priority
            Some(t) => (now.timestamp_millis() - t) - interval.num_milliseconds(),
        };
        if overdue >= 0 && best.as_ref().is_none_or(|b| overdue > b.2) {
            best = Some((game, day, overdue));
        }
    }
    best.map(|(g, d, _)| (g, d))
}

struct Candidate {
    key: String,
    game: Game,
    league: String,
    year: i32,
}

/// Distinct events in the snapshot that still need a Liquipedia lookup (unseen,
/// or a stale "not found"). Pure read of SNAPSHOT + EVENT_LINKS.
fn collect_resolution_candidates() -> Vec<Candidate> {
    let now = Utc::now();
    let snap = SNAPSHOT.read().unwrap_or_else(PoisonError::into_inner);
    let links = EVENT_LINKS.read().unwrap_or_else(PoisonError::into_inner);
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for m in &snap.matches {
        // Traditional sports (MLB) aren't on Liquipedia — never resolve them.
        if m.game.traditional() {
            continue;
        }
        let year = m.begin_at.year();
        let key = event_key(m.game, &m.league, year);
        if !seen.insert(key.clone()) {
            continue;
        }
        let needs = match links.get(&key) {
            None => true,
            Some(e) => e.url.is_none() && (now - e.checked_at) > Duration::days(NEG_TTL_DAYS),
        };
        if needs {
            out.push(Candidate {
                key,
                game: m.game,
                league: m.league.clone(),
                year,
            });
        }
    }
    out
}

/// Persist resolved links to the in-memory cache and (if present) SQLite.
fn apply_resolutions(
    results: &[(String, Option<String>)],
    store: Option<&mut rusqlite::Connection>,
    now: DateTime<Utc>,
) {
    if results.is_empty() {
        return;
    }
    {
        let mut links = EVENT_LINKS.write().unwrap_or_else(PoisonError::into_inner);
        for (key, url) in results {
            links.insert(
                key.clone(),
                EventLink {
                    url: url.clone(),
                    checked_at: now,
                },
            );
        }
    }
    if let Some(conn) = store {
        for (key, url) in results {
            if let Err(e) =
                crate::store::set_event_link(conn, key, url.as_deref(), now.timestamp_millis())
            {
                leptos::logging::log!("event_link persist failed: {e}");
            }
        }
    }
    let found = results.iter().filter(|(_, u)| u.is_some()).count();
    leptos::logging::log!("resolved {found}/{} event link(s)", results.len());
}

// ----- View building -------------------------------------------------------

fn local_day_start(tz: &Tz, date: NaiveDate) -> DateTime<Utc> {
    let naive = date.and_hms_opt(0, 0, 0).expect("valid midnight");
    tz.from_local_datetime(&naive)
        .earliest()
        .map_or_else(|| Utc.from_utc_datetime(&naive), |dt| dt.with_timezone(&Utc))
}

fn time_label(local: DateTime<Tz>, hour24: bool) -> String {
    if hour24 {
        return format!("{:02}:{:02}", local.hour(), local.minute());
    }
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

/// Compact day label, e.g. "Mon, Jun 23" — used by the series rows, where the
/// date sits inline with the time rather than as a day heading.
fn short_day_label(local: DateTime<Tz>) -> String {
    format!("{}, {} {}", local.format("%a"), local.format("%b"), local.day())
}

/// Resolve the effective view status, applying the "started but not marked
/// finished" live heuristic (the free tier has no real-time feed).
fn effective_status(m: &NormalizedMatch, now: DateTime<Utc>) -> MatchStatus {
    match m.status {
        MatchStatus::Upcoming if m.begin_at <= now => MatchStatus::Live,
        other => other,
    }
}

fn to_view(m: &NormalizedMatch, tz: &Tz, now: DateTime<Utc>, hour24: bool) -> MatchView {
    let local = m.begin_at.with_timezone(tz);
    let status = effective_status(m, now);

    // PandaScore returns placeholder 0-0 results on unplayed matches, so only
    // trust a finished match's score, or a live match's score once it's nonzero
    // (a real in-progress map count rather than the 0-0 placeholder).
    let trust_scores = match status {
        MatchStatus::Finished => true,
        MatchStatus::Live => {
            matches!((m.team_a.score, m.team_b.score), (Some(a), Some(b)) if a > 0 || b > 0)
        }
        _ => false,
    };
    let mut team_a = TeamView {
        label: m.team_a.label.clone(),
        name: m.team_a.name.clone(),
        score: trust_scores.then_some(m.team_a.score).flatten(),
        winner: false,
    };
    let mut team_b = TeamView {
        label: m.team_b.label.clone(),
        name: m.team_b.name.clone(),
        score: trust_scores.then_some(m.team_b.score).flatten(),
        winner: false,
    };
    // Only a finished match has a winner; a leader mid-match isn't one yet.
    if status == MatchStatus::Finished {
        if let (Some(a), Some(b)) = (team_a.score, team_b.score) {
            team_a.winner = a > b;
            team_b.winner = b > a;
        }
    }

    // When the venue's timezone is known (MLB), the same instant in that zone,
    // labelled with its abbreviation (e.g. "6:00 PM EDT") — the local time at the
    // ballpark. Empty otherwise (esports has no venue tz).
    let venue_label = m
        .venue_tz
        .as_deref()
        .and_then(|id| id.parse::<Tz>().ok())
        .map(|vtz| {
            let vlocal = m.begin_at.with_timezone(&vtz);
            format!("{} {}", time_label(vlocal, hour24), vlocal.format("%Z"))
        })
        .unwrap_or_default();

    MatchView {
        id: m.id,
        game: m.game,
        league: m.league.clone(),
        serie_name: m.serie_name.clone(),
        tier: m.tier.clone(),
        status,
        clock_label: time_label(local, hour24),
        venue_label,
        best_of: m.best_of.map(|n| format!("Bo{n}")).unwrap_or_default(),
        team_a,
        team_b,
        stream_url: m.stream_url.clone(),
        event_url: resolved_event_url(m.game, &m.league, m.begin_at, m.league_url.as_deref()),
        begin_at_ms: m.begin_at.timestamp_millis(),
    }
}

/// The event link shown in the UI: a resolved exact Liquipedia page when one is
/// cached, else the generic fallback (official site or Liquipedia search).
fn resolved_event_url(
    game: Game,
    league: &str,
    begin_at: DateTime<Utc>,
    official: Option<&str>,
) -> String {
    let key = event_key(game, league, begin_at.year());
    if let Some(EventLink { url: Some(u), .. }) = EVENT_LINKS.read().unwrap_or_else(PoisonError::into_inner).get(&key) {
        return u.clone();
    }
    event_link(game, league, official)
}

/// Generic fallback link: the official site when the API provides one, else a
/// game-specific Liquipedia search built from the event name.
fn event_link(game: Game, league: &str, official: Option<&str>) -> String {
    if let Some(url) = official.filter(|u| !u.trim().is_empty()) {
        return url.to_string();
    }
    let wiki = match game {
        Game::Cs2 => "counterstrike",
        Game::Lol => "leagueoflegends",
        // Traditional sports have no Liquipedia link; unused.
        Game::Mlb | Game::F1 => "counterstrike",
    };
    format!(
        "https://liquipedia.net/{wiki}/index.php?search={}",
        pct_encode(league)
    )
}

/// Minimal percent-encoding for a URL query value.
fn pct_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Group already-time-sorted views into per-day buckets, and within each day
/// into per-league groups (leagues ordered by their earliest match). A league
/// group's `bo` is set only when every match in it shares the same best-of.
fn group_days(views: Vec<MatchView>, tz: &Tz) -> Vec<DayGroup> {
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
                leagues: Vec::new(),
            });
        }
        let day = days.last_mut().unwrap();
        // Group by edition (league + serie), so two editions of the same circuit
        // form distinct groups with their own header and event link.
        if let Some(lg) = day
            .leagues
            .iter_mut()
            .find(|l| l.league == v.league && l.serie_name == v.serie_name)
        {
            lg.matches.push(v);
        } else {
            day.leagues.push(LeagueGroup {
                league: v.league.clone(),
                serie_name: v.serie_name.clone(),
                event_url: String::new(),
                bo: None,
                matches: vec![v],
            });
        }
    }
    for day in &mut days {
        for lg in &mut day.leagues {
            let first = lg.matches.first().map(|m| m.best_of.clone()).unwrap_or_default();
            let uniform = !first.is_empty() && lg.matches.iter().all(|m| m.best_of == first);
            lg.bo = uniform.then_some(first);
            lg.event_url = lg
                .matches
                .first()
                .map(|m| m.event_url.clone())
                .unwrap_or_default();
        }
        // Keep each game's events together within the day (CS2, then LoL).
        // Stable sort preserves the time order within a game.
        day.leagues.sort_by_key(|lg| match lg.matches.first().map(|m| m.game) {
            Some(Game::Cs2) => 0u8,
            Some(Game::Lol) => 1,
            Some(Game::Mlb) => 2,
            Some(Game::F1) => 3,
            None => 4,
        });
    }
    days
}

#[allow(clippy::too_many_arguments)]
fn matches_in_window(
    snap: &Snapshot,
    traditional: bool,
    game: Option<Game>,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    tz: &Tz,
    now: DateTime<Utc>,
    hour24: bool,
) -> Vec<MatchView> {
    snap.matches
        .iter()
        .filter(|m| m.status != MatchStatus::Canceled)
        // Esports and traditional sports are separate top-level modes.
        .filter(|m| m.game.traditional() == traditional)
        .filter(|m| game.is_none_or(|g| m.game == g))
        .filter(|m| m.begin_at >= start && m.begin_at < end)
        .map(|m| to_view(m, tz, now, hour24))
        .collect()
}

/// The mode (traditional vs esports) and optional within-mode game for a filter
/// slug: "trad" ⇒ all traditional sports; "mlb"/"f1" ⇒ that one traditional
/// sport; "cs2"/"lol" ⇒ that esports title; anything else (incl. "all") ⇒ all
/// esports. The client filters traditional sports further (the sport tabs), so
/// the homepage uses "trad" and narrows client-side.
fn mode_filter(game_filter: &str) -> (bool, Option<Game>) {
    if game_filter == "trad" {
        return (true, None);
    }
    let game = Game::from_filter(game_filter);
    (game.is_some_and(Game::traditional), game)
}

/// Parse an IANA tz name, falling back to the configured default.
fn resolve_tz(name: &str, default: Tz) -> Tz {
    name.parse::<Tz>().unwrap_or(default)
}

/// Homepage: live matches pinned, then the next `upcoming_days` grouped by day.
#[must_use]
pub fn homepage_view(game_filter: &str, tz_name: &str, hour24: bool) -> ScheduleView {
    let cfg = config();
    let tz = resolve_tz(tz_name, cfg.tz);
    let (traditional, game) = mode_filter(game_filter);
    let now = Utc::now();
    let today = now.with_timezone(&tz).date_naive();
    let start = local_day_start(&tz, today);
    // Traditional sports play daily, so cap the default forward window (the
    // client's "show later days" extends it up to TRAD_FORWARD_MAX). Esports
    // show the full upcoming horizon.
    let end = if traditional {
        local_day_start(&tz, today + Duration::days(crate::types::TRAD_FORWARD_DAYS + 1))
    } else {
        now + Duration::days(cfg.upcoming_days)
    };

    let snap = SNAPSHOT.read().unwrap_or_else(PoisonError::into_inner);
    let mut all = matches_in_window(&snap, traditional, game, start, end, &tz, now, hour24);
    // F1 races are weekly, so the short daily window catches only the first
    // session(s). Always surface the next Grand Prix's whole weekend (MLB stays
    // compact). The client sport-tabs still filter it down if F1 isn't selected.
    if traditional {
        let have: std::collections::HashSet<i64> = all.iter().map(|m| m.id).collect();
        for mv in next_gp_sessions(&snap, now, &tz, hour24) {
            if !have.contains(&mv.id) {
                all.push(mv);
            }
        }
    }

    ScheduleView {
        days: group_days(all, &tz),
        today_key: today.format("%Y-%m-%d").to_string(),
        fetched_at_ms: snap.fetched_at.timestamp_millis(),
        fetched_label: time_label(snap.fetched_at.with_timezone(&tz), hour24),
        stale: snap.stale,
        using_fixture: snap.using_fixture,
        demo_forced: cfg.demo,
        date_label: None,
        prev_date: None,
        next_date: None,
    }
}

/// Every session of the next upcoming Grand Prix (its whole weekend) as views.
/// Empty when no F1 session is upcoming (off-season).
fn next_gp_sessions(
    snap: &Snapshot,
    now: DateTime<Utc>,
    tz: &Tz,
    hour24: bool,
) -> Vec<MatchView> {
    let next_gp = snap
        .matches
        .iter()
        .filter(|m| {
            m.game == Game::F1 && m.status != MatchStatus::Canceled && m.begin_at >= now
        })
        .min_by_key(|m| m.begin_at)
        .map(|m| m.serie_name.clone());
    let Some(next_gp) = next_gp else {
        return Vec::new();
    };
    snap.matches
        .iter()
        .filter(|m| {
            m.game == Game::F1 && m.status != MatchStatus::Canceled && m.serie_name == next_gp
        })
        .map(|m| to_view(m, tz, now, hour24))
        .collect()
}

/// Single-day view with prev/next navigation.
#[must_use]
pub fn day_view(date: &str, game_filter: &str, tz_name: &str, hour24: bool) -> ScheduleView {
    let cfg = config();
    let tz = resolve_tz(tz_name, cfg.tz);
    let (traditional, game) = mode_filter(game_filter);
    let now = Utc::now();

    let day = NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .unwrap_or_else(|_| now.with_timezone(&tz).date_naive());
    let start = local_day_start(&tz, day);
    let end = local_day_start(&tz, day + Duration::days(1));

    let snap = SNAPSHOT.read().unwrap_or_else(PoisonError::into_inner);
    let all = matches_in_window(&snap, traditional, game, start, end, &tz, now, hour24);

    let local_noon = tz
        .from_local_datetime(&day.and_hms_opt(12, 0, 0).unwrap())
        .earliest()
        .unwrap_or_else(|| tz.from_utc_datetime(&day.and_hms_opt(12, 0, 0).unwrap()));

    ScheduleView {
        days: group_days(all, &tz),
        today_key: now.with_timezone(&tz).date_naive().format("%Y-%m-%d").to_string(),
        fetched_at_ms: snap.fetched_at.timestamp_millis(),
        fetched_label: time_label(snap.fetched_at.with_timezone(&tz), hour24),
        stale: snap.stale,
        using_fixture: snap.using_fixture,
        demo_forced: cfg.demo,
        date_label: Some(day_label(local_noon)),
        prev_date: Some((day - Duration::days(1)).format("%Y-%m-%d").to_string()),
        next_date: Some((day + Duration::days(1)).format("%Y-%m-%d").to_string()),
    }
}

/// Schedule for an inclusive date range `[start_date, end_date]` (ISO dates in
/// the display tz). Backs the "show earlier days" view and the calendar picker.
#[must_use]
pub fn range_view(
    start_date: &str,
    end_date: &str,
    game_filter: &str,
    tz_name: &str,
    hour24: bool,
) -> ScheduleView {
    let cfg = config();
    let tz = resolve_tz(tz_name, cfg.tz);
    let (traditional, game) = mode_filter(game_filter);
    let now = Utc::now();
    let today = now.with_timezone(&tz).date_naive();

    let parse = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d");
    let start_day = parse(start_date).unwrap_or(today);
    let end_day = parse(end_date).unwrap_or(today).max(start_day);
    let start = local_day_start(&tz, start_day);
    // Inclusive end day → window runs to the start of the following day.
    let end = local_day_start(&tz, end_day + Duration::days(1));

    let snap = SNAPSHOT.read().unwrap_or_else(PoisonError::into_inner);
    let all = matches_in_window(&snap, traditional, game, start, end, &tz, now, hour24);

    ScheduleView {
        days: group_days(all, &tz),
        today_key: now.with_timezone(&tz).date_naive().format("%Y-%m-%d").to_string(),
        fetched_at_ms: snap.fetched_at.timestamp_millis(),
        fetched_label: time_label(snap.fetched_at.with_timezone(&tz), hour24),
        stale: snap.stale,
        using_fixture: snap.using_fixture,
        demo_forced: cfg.demo,
        date_label: None,
        prev_date: None,
        next_date: None,
    }
}

/// All cached matches for one event/league (past + upcoming), grouped by day.
/// Backs the internal event page.
#[must_use]
pub fn event_view(event: &str, tz_name: &str, hour24: bool) -> ScheduleView {
    let cfg = config();
    let tz = resolve_tz(tz_name, cfg.tz);
    let now = Utc::now();

    // `event` is the full edition name (league + serie), so two editions of one
    // circuit resolve to distinct pages.
    let snap = SNAPSHOT.read().unwrap_or_else(PoisonError::into_inner);
    let all: Vec<MatchView> = snap
        .matches
        .iter()
        .filter(|m| m.status != MatchStatus::Canceled)
        .filter(|m| event_name_eq(&m.league, &m.serie_name, event))
        .map(|m| to_view(m, &tz, now, hour24))
        .collect();

    ScheduleView {
        days: group_days(all, &tz),
        today_key: now.with_timezone(&tz).date_naive().format("%Y-%m-%d").to_string(),
        fetched_at_ms: snap.fetched_at.timestamp_millis(),
        fetched_label: time_label(snap.fetched_at.with_timezone(&tz), hour24),
        stale: snap.stale,
        using_fixture: snap.using_fixture,
        demo_forced: cfg.demo,
        date_label: None,
        prev_date: None,
        next_date: None,
    }
}

/// All cached matches involving a team (matched on its full name), past +
/// upcoming, grouped by day. Backs the team page.
#[must_use]
pub fn team_view(team: &str, tz_name: &str, hour24: bool) -> ScheduleView {
    let cfg = config();
    let tz = resolve_tz(tz_name, cfg.tz);
    let now = Utc::now();

    let snap = SNAPSHOT.read().unwrap_or_else(PoisonError::into_inner);
    let all: Vec<MatchView> = snap
        .matches
        .iter()
        .filter(|m| m.status != MatchStatus::Canceled)
        .filter(|m| m.team_a.name == team || m.team_b.name == team)
        .map(|m| to_view(m, &tz, now, hour24))
        .collect();

    ScheduleView {
        days: group_days(all, &tz),
        today_key: now.with_timezone(&tz).date_naive().format("%Y-%m-%d").to_string(),
        fetched_at_ms: snap.fetched_at.timestamp_millis(),
        fetched_label: time_label(snap.fetched_at.with_timezone(&tz), hour24),
        stale: snap.stale,
        using_fixture: snap.using_fixture,
        demo_forced: cfg.demo,
        date_label: None,
        prev_date: None,
        next_date: None,
    }
}

/// The given match ids that are still upcoming, as views sorted by start time.
/// Backs the notifications page — started/finished/canceled matches are dropped
/// so the list only shows reminders that can still fire.
#[must_use]
pub fn upcoming_matches_by_ids(ids: &[i64], tz_name: &str, hour24: bool) -> Vec<MatchView> {
    let cfg = config();
    let tz = resolve_tz(tz_name, cfg.tz);
    let now = Utc::now();
    let wanted: std::collections::HashSet<i64> = ids.iter().copied().collect();
    let snap = SNAPSHOT.read().unwrap_or_else(PoisonError::into_inner);
    let mut out: Vec<MatchView> = snap
        .matches
        .iter()
        .filter(|m| wanted.contains(&m.id))
        .filter(|m| m.status == MatchStatus::Upcoming && m.begin_at > now)
        .map(|m| to_view(m, &tz, now, hour24))
        .collect();
    out.sort_by_key(|v| v.begin_at_ms);
    out
}

// ----- Game/event subscription expansion -----------------------------------

/// A reminder to create for one upcoming match (from a game/event subscription).
pub struct ReminderSeed {
    pub match_id: i64,
    pub game: String,
    pub league: String,
    /// Both teams' full names, so a team subscription's reminders can be cleaned
    /// up by team on unsubscribe (mirrors the game/league cleanup columns).
    pub team_a: String,
    pub team_b: String,
    pub notify_at_ms: i64,
    pub title: String,
    pub body: String,
    pub url: String,
}

/// Build a reminder seed for one upcoming match. Centralizes the time/title/
/// body/url derivation so every reminder (single-match ★ and game/event
/// subscription) is produced server-side from the same snapshot data.
fn reminder_seed(m: &NormalizedMatch, lead_ms: i64, tz: &Tz) -> ReminderSeed {
    let local = m.begin_at.with_timezone(tz);
    ReminderSeed {
        match_id: m.id,
        game: m.game.slug().to_string(),
        league: m.league.clone(),
        team_a: m.team_a.name.clone(),
        team_b: m.team_b.name.clone(),
        notify_at_ms: m.begin_at.timestamp_millis() - lead_ms,
        // Traditional sports read "away at home"; esports "vs".
        title: format!(
            "{} {} {}",
            m.team_a.label,
            if m.game.traditional() { "at" } else { "vs" },
            m.team_b.label
        ),
        body: format!("{} · {}", m.league, time_label(local, false)),
        url: resolved_event_url(m.game, &m.league, m.begin_at, m.league_url.as_deref()),
    }
}

/// Upcoming matches matching a subscription scope, as reminder seeds. `kind` is
/// "game" (value = "cs2"/"lol"), "league" (value = league name), or "team"
/// (value = full team name, matched against either side).
#[must_use]
pub fn scope_reminder_seeds(kind: &str, value: &str, lead_ms: i64) -> Vec<ReminderSeed> {
    let cfg = config();
    let now = Utc::now();
    let snap = SNAPSHOT.read().unwrap_or_else(PoisonError::into_inner);
    snap.matches
        .iter()
        .filter(|m| m.status != MatchStatus::Canceled && m.begin_at > now)
        .filter(|m| match kind {
            "game" => m.game.slug() == value,
            "league" => m.league == value,
            "team" => m.team_a.name == value || m.team_b.name == value,
            _ => false,
        })
        .map(|m| reminder_seed(m, lead_ms, &cfg.tz))
        .collect()
}

/// A reminder seed for a single upcoming match by id — `None` if it's unknown,
/// already started, or canceled. Lets the server (not the client) decide the
/// notification's time/title/body/url for a starred match.
#[must_use]
pub fn reminder_seed_for_match(match_id: i64, lead_ms: i64) -> Option<ReminderSeed> {
    let cfg = config();
    let now = Utc::now();
    let snap = SNAPSHOT.read().unwrap_or_else(PoisonError::into_inner);
    snap.matches
        .iter()
        .find(|m| m.id == match_id && m.status != MatchStatus::Canceled && m.begin_at > now)
        .map(|m| reminder_seed(m, lead_ms, &cfg.tz))
}

// ----- Match detail + event standings/bracket ------------------------------

/// Basics for the match detail page: the formatted view, its streams, the
/// tournament id, and league name. `None` when the match isn't in the window.
#[must_use]
pub fn match_basics(
    match_id: i64,
    tz_name: &str,
    hour24: bool,
) -> Option<(MatchView, Vec<StreamView>, Option<i64>, String)> {
    let cfg = config();
    let tz = resolve_tz(tz_name, cfg.tz);
    let now = Utc::now();
    let snap = SNAPSHOT.read().unwrap_or_else(PoisonError::into_inner);
    let m = snap.matches.iter().find(|m| m.id == match_id)?;
    Some((
        to_view(m, &tz, now, hour24),
        m.streams.clone(),
        m.tournament_id,
        m.league.clone(),
    ))
}

/// The tournament (stage) ids of an event's cached matches, ordered by when each
/// stage started — so a multi-stage event (e.g. Swiss stages → playoffs) renders
/// its stages in order. `event` is the full edition name (league + serie).
#[must_use]
pub fn event_stages(event: &str) -> Vec<(i64, Game)> {
    stages_matching(|m| event_name_eq(&m.league, &m.serie_name, event))
}

/// The stage tournament ids of a league filter's cached matches. Like
/// [`event_stages`] but keyed on the (short) league name, which is what the
/// front-page event filter selects.
#[must_use]
pub fn league_stages(league: &str) -> Vec<(i64, Game)> {
    stages_matching(|m| m.league == league)
}

/// Distinct `(tournament id, game)` of the cached matches passing `pred`, ordered
/// by when each stage started (its earliest match). Capturing the game here saves
/// [`event_info`] a per-stage snapshot scan to look it up.
fn stages_matching(pred: impl Fn(&NormalizedMatch) -> bool) -> Vec<(i64, Game)> {
    let snap = SNAPSHOT.read().unwrap_or_else(PoisonError::into_inner);
    let mut earliest: std::collections::HashMap<i64, (i64, Game)> = std::collections::HashMap::new();
    for m in &snap.matches {
        if pred(m) {
            if let Some(tid) = m.tournament_id {
                let ms = m.begin_at.timestamp_millis();
                earliest
                    .entry(tid)
                    .and_modify(|e| e.0 = e.0.min(ms))
                    .or_insert((ms, m.game));
            }
        }
    }
    let mut stages: Vec<(i64, (i64, Game))> = earliest.into_iter().collect();
    stages.sort_by_key(|&(_, (ms, _))| ms);
    stages.into_iter().map(|(tid, (_, game))| (tid, game)).collect()
}

/// Resolve a list of `(stage tournament id, game)` to their [`EventInfo`]s
/// (standings + brackets), dropping empties and tagging each with the event
/// name. Order is preserved so stages render Swiss → playoffs.
pub async fn stages_info(stages: Vec<(i64, Game)>, event: &str) -> Vec<EventInfo> {
    // Fetch every stage concurrently (each is an independent set of API calls),
    // preserving order, so a multi-stage event isn't N round-trips deep.
    futures::future::join_all(stages.into_iter().map(|(tid, game)| event_info(tid, game)))
        .await
        .into_iter()
        .filter(|info| !info.is_empty())
        .map(|mut info| {
            info.event = event.to_string();
            info
        })
        .collect()
}

/// Standings + bracket for a tournament, cached with a TTL. Fetches live when a
/// token is configured; in demo mode returns the pre-seeded fixture.
pub async fn event_info(tournament_id: i64, game: Game) -> EventInfo {
    {
        let cache = EVENTS.read().unwrap_or_else(PoisonError::into_inner);
        if let Some(c) = cache.get(&tournament_id) {
            if Utc::now() - c.fetched_at < Duration::minutes(EVENT_TTL_MIN) {
                return c.info.clone();
            }
        }
    }
    fetch_event(tournament_id, game).await
}

/// Fetch a tournament's standings + bracket from the API unconditionally and
/// store it in the EVENTS cache. The poller's pre-warm calls this directly (with
/// its own freshness check) to keep active events hot below the serve TTL.
async fn fetch_event(tournament_id: i64, game: Game) -> EventInfo {
    let now = Utc::now();
    // `game` (for game-appropriate standings labels) is passed in by the caller,
    // which already scanned the snapshot to find this stage.
    let Some(token) = config().token.clone() else {
        // No token (demo / unconfigured): serve whatever's seeded, else nothing.
        return EVENTS
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&tournament_id)
            .map(|c| c.info.clone())
            .unwrap_or_default();
    };
    // Stage meta (name + whether it's an elimination bracket) and standings are
    // independent, so fetch them together (best-effort; a failure just yields an
    // empty section). The bracket call depends on both, so it follows.
    let (meta_res, standings_res) = tokio::join!(
        crate::pandascore::fetch_tournament_meta(&HTTP, &token, tournament_id),
        crate::pandascore::fetch_standings(&HTTP, &token, tournament_id),
    );
    let (stage, has_bracket) = match meta_res {
        Ok((name, hb)) => (name, Some(hb)),
        Err(e) => {
            leptos::logging::log!("tournament meta failed (tournament {tournament_id}): {e}");
            (String::new(), None)
        }
    };
    let standings = standings_res.unwrap_or_else(|e| {
        leptos::logging::log!("standings fetch failed (tournament {tournament_id}): {e}");
        Vec::new()
    });
    // Fetch the brackets endpoint when the stage has a tree bracket *or* a
    // standings table — a Swiss stage reports `has_bracket=false` but still
    // exposes its matches there (which `build_swiss` turns into the grid).
    // Truly-empty stages (no bracket, no standings) skip the call.
    let (rounds, swiss) = if has_bracket == Some(true) || !standings.is_empty() {
        crate::pandascore::fetch_bracket(&HTTP, &token, tournament_id)
            .await
            .unwrap_or_else(|e| {
                leptos::logging::log!("bracket fetch failed (tournament {tournament_id}): {e}");
                (Vec::new(), Vec::new())
            })
    } else {
        (Vec::new(), Vec::new())
    };
    let info = EventInfo {
        event: String::new(),
        tournament_id,
        stage,
        game,
        standings,
        rounds,
        swiss,
    };
    EVENTS.write().unwrap_or_else(PoisonError::into_inner).insert(
        tournament_id,
        CachedEvent {
            info: info.clone(),
            fetched_at: now,
        },
    );
    info
}

/// Every stage `(tournament id, game)` of an edition that has a recent or
/// upcoming match — the events a visitor is likely to open. All stages, so a
/// recently-finished event's older group stages are warmed too (the event page
/// loads them all).
fn active_tournaments(now: DateTime<Utc>) -> Vec<(i64, Game)> {
    let lo = (now - Duration::days(4)).timestamp_millis();
    let hi = (now + Duration::days(config().upcoming_days)).timestamp_millis();
    let snap = SNAPSHOT.read().unwrap_or_else(PoisonError::into_inner);
    // Editions with a recent/upcoming match.
    let mut active: std::collections::HashSet<String> = std::collections::HashSet::new();
    for m in &snap.matches {
        if (lo..=hi).contains(&m.begin_at.timestamp_millis()) {
            active.insert(full_event_name(&m.league, &m.serie_name));
        }
    }
    // Every stage tournament of those editions.
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for m in &snap.matches {
        if let Some(tid) = m.tournament_id {
            if !seen.contains(&tid) && active.contains(&full_event_name(&m.league, &m.serie_name)) {
                seen.insert(tid);
                out.push((tid, m.game));
            }
        }
    }
    out
}

/// Keep recent/upcoming events' standings + brackets hot in the EVENTS cache so
/// a page request is a cache hit, not a cold ~800 ms PandaScore fetch. Refreshes
/// stale entries (past the pre-warm TTL) concurrently, bounded per cycle so a
/// cold start can't blow the API budget; the rest catch up over the next cycles.
async fn prewarm_active(now: DateTime<Utc>) {
    let prewarm = Duration::minutes(PREWARM_TTL_MIN);
    let stale: Vec<(i64, Game)> = active_tournaments(now)
        .into_iter()
        .filter(|(tid, _)| {
            let cache = EVENTS.read().unwrap_or_else(PoisonError::into_inner);
            cache.get(tid).is_none_or(|c| now - c.fetched_at >= prewarm)
        })
        .take(MAX_PREWARM_PER_CYCLE)
        .collect();
    futures::future::join_all(stale.into_iter().map(|(tid, game)| fetch_event(tid, game))).await;
}

/// Seed the EVENTS cache with demo standings/brackets for the demo tournaments.
fn seed_demo_events(matches: &[NormalizedMatch], now: DateTime<Utc>) {
    let mut ev = EVENTS.write().unwrap_or_else(PoisonError::into_inner);
    let mut seen = std::collections::HashSet::new();
    for m in matches {
        if let Some(tid) = m.tournament_id {
            if seen.insert(tid) {
                let mut info = demo_event_info(&m.league);
                info.tournament_id = tid;
                ev.insert(tid, CachedEvent { info, fetched_at: now });
            }
        }
    }
}

/// A demo standings table + single-elimination bracket for one event.
fn demo_event_info(league: &str) -> EventInfo {
    // Extra demo events exercising bigger / contrived brackets.
    match league {
        "ESL One" => return demo_event_large_de(),
        "PGL Major" => return demo_event_large_se(),
        "Bye Cup" => return demo_event_byes(),
        "Reset Cup" => return demo_event_reset(),
        "Gauntlet" => return demo_event_gauntlet(),
        "Mega Bracket" => return demo_event_wide(),
        "Long Names Cup" => return demo_event_long_names(),
        _ => {}
    }
    let lol = matches!(
        league,
        "LCK" | "LEC" | "LPL" | "MSI" | "Worlds" | "LTA" | "First Stand"
    );
    let t: [&str; 8] = if lol {
        ["T1", "GEN", "HLE", "DK", "BLG", "TES", "G2", "FNC"]
    } else {
        ["NAVI", "VIT", "FaZe", "MOUZ", "G2", "SPIRIT", "BIG", "NIP"]
    };
    let bm = |a: &str, b: &str, sa, sb, w: &str, feeders: Vec<(usize, usize)>| BracketMatch {
        team_a: a.to_string(),
        team_b: b.to_string(),
        score_a: sa,
        score_b: sb,
        winner: w.to_string(),
        match_id: 0,
        feeders,
    };
    // LoL events demo a single-elimination playoff (also showing the
    // decided-but-unplayed and TBD-locked states); CS2 events demo a
    // double-elimination playoff (upper/lower bracket + grand final). Feeders are
    // `(round, index)` of the matches whose scores decide each later lineup.
    let br = |title: &str, section: &str, matches: Vec<BracketMatch>| BracketRound {
        title: title.to_string(),
        matches,
        section: section.to_string(),
    };
    let rounds = if lol {
        vec![
            br(
                "Quarterfinals",
                "",
                vec![
                    bm(t[0], t[1], Some(2), Some(0), "a", vec![]),
                    bm(t[2], t[3], Some(2), Some(1), "a", vec![]),
                    bm(t[4], t[5], Some(1), Some(2), "b", vec![]),
                    bm(t[6], t[7], Some(2), Some(0), "a", vec![]),
                ],
            ),
            br(
                "Semifinals",
                "",
                vec![
                    // One played, one decided-but-not-yet-played (names only).
                    bm(t[0], t[2], Some(1), Some(2), "b", vec![(0, 0), (0, 1)]),
                    bm(t[5], t[6], None, None, "", vec![(0, 2), (0, 3)]),
                ],
            ),
            // Participants not decided yet → stays locked/hidden.
            br("Final", "", vec![bm("TBD", "TBD", None, None, "", vec![(1, 0), (1, 1)])]),
        ]
    } else {
        vec![
            br(
                "Semifinals",
                "upper",
                vec![
                    bm(t[0], t[1], Some(2), Some(1), "a", vec![]),
                    bm(t[2], t[3], Some(2), Some(0), "a", vec![]),
                ],
            ),
            br("Final", "upper", vec![bm(t[0], t[2], Some(1), Some(2), "b", vec![(0, 0), (0, 1)])]),
            // Losers of the upper semis drop here.
            br("Round 1", "lower", vec![bm(t[1], t[3], Some(2), Some(0), "a", vec![(0, 0), (0, 1)])]),
            // Lower-round winner vs the upper-final loser.
            br("Final", "lower", vec![bm(t[1], t[0], Some(1), Some(2), "b", vec![(2, 0), (1, 0)])]),
            br("Final", "final", vec![bm(t[2], t[0], Some(3), Some(1), "a", vec![(1, 0), (3, 0)])]),
        ]
    };
    let row = |rank, team: &str, w, l, gw, gl| StandingRow {
        rank,
        team: team.to_string(),
        wins: w,
        losses: l,
        ties: 0,
        game_wins: gw,
        game_losses: gl,
        ..Default::default()
    };
    let standings = vec![
        row(1, t[2], 3, 0, 9, 3),
        row(2, t[5], 2, 1, 7, 5),
        row(3, t[0], 2, 1, 6, 4),
        row(4, t[6], 1, 2, 5, 6),
        row(5, t[3], 1, 2, 4, 6),
        row(6, t[1], 0, 3, 2, 9),
    ];
    EventInfo {
        event: league.to_string(),
        tournament_id: 0, // set by the caller (seed_demo_events)
        stage: String::new(),
        game: if lol { Game::Lol } else { Game::Cs2 },
        standings,
        rounds,
        swiss: Vec::new(),
    }
}

// Shared builders for the larger demo brackets.
fn dbm(a: &str, b: &str, sa: Option<i64>, sb: Option<i64>, w: &str, feeders: &[(usize, usize)]) -> BracketMatch {
    BracketMatch {
        team_a: a.to_string(),
        team_b: b.to_string(),
        score_a: sa,
        score_b: sb,
        winner: w.to_string(),
        match_id: 0,
        feeders: feeders.to_vec(),
    }
}
fn dbr(title: &str, section: &str, matches: Vec<BracketMatch>) -> BracketRound {
    BracketRound {
        title: title.to_string(),
        matches,
        section: section.to_string(),
    }
}
fn drow(rank: i32, team: &str, w: i32, l: i32, gw: i32, gl: i32) -> StandingRow {
    StandingRow {
        rank,
        team: team.to_string(),
        wins: w,
        losses: l,
        ties: 0,
        game_wins: gw,
        game_losses: gl,
        ..Default::default()
    }
}

/// A larger CS2 double-elimination demo: 8-team winner's bracket (QF→SF→Final),
/// a loser's bracket (Round 1→Final) of different width, and the grand final.
fn demo_event_large_de() -> EventInfo {
    let rounds = vec![
        // Winner's bracket.
        dbr(
            "Quarterfinals",
            "upper",
            vec![
                dbm("NAVI", "VIT", Some(2), Some(0), "a", &[]),
                dbm("FaZe", "MOUZ", Some(2), Some(1), "a", &[]),
                dbm("G2", "SPIRIT", Some(1), Some(2), "b", &[]),
                dbm("BIG", "NIP", Some(2), Some(0), "a", &[]),
            ],
        ),
        dbr(
            "Semifinals",
            "upper",
            vec![
                dbm("NAVI", "FaZe", Some(1), Some(2), "b", &[(0, 0), (0, 1)]),
                dbm("SPIRIT", "BIG", Some(2), Some(0), "a", &[(0, 2), (0, 3)]),
            ],
        ),
        dbr("Final", "upper", vec![dbm("FaZe", "SPIRIT", Some(2), Some(1), "a", &[(1, 0), (1, 1)])]),
        // Loser's bracket (the quarterfinal losers drop in).
        dbr(
            "Round 1",
            "lower",
            vec![
                dbm("VIT", "MOUZ", Some(2), Some(0), "a", &[(0, 0), (0, 1)]),
                dbm("G2", "NIP", Some(2), Some(1), "a", &[(0, 2), (0, 3)]),
            ],
        ),
        dbr("Final", "lower", vec![dbm("VIT", "G2", Some(2), Some(1), "a", &[(3, 0), (3, 1)])]),
        // Grand final.
        dbr("Final", "final", vec![dbm("FaZe", "VIT", Some(3), Some(1), "a", &[(2, 0), (4, 0)])]),
    ];
    let standings = vec![
        drow(1, "FaZe", 3, 0, 8, 2),
        drow(2, "VIT", 2, 2, 7, 6),
        drow(3, "SPIRIT", 2, 1, 6, 4),
        drow(4, "NAVI", 1, 2, 4, 5),
        drow(5, "G2", 1, 2, 5, 6),
        drow(6, "BIG", 1, 2, 4, 5),
        drow(7, "MOUZ", 0, 2, 1, 4),
        drow(8, "NIP", 0, 2, 2, 4),
    ];
    EventInfo {
        event: "ESL One".to_string(),
        tournament_id: 0,
        stage: String::new(),
        game: Game::Cs2,
        standings,
        rounds,
        swiss: Vec::new(),
    }
}

/// A larger CS2 single-elimination demo: 16 teams (Round of 16 → QF → SF → Final).
fn demo_event_large_se() -> EventInfo {
    let rounds = vec![
        dbr(
            "Round of 16",
            "",
            vec![
                dbm("NAVI", "ENCE", Some(2), Some(0), "a", &[]),
                dbm("VIT", "COL", Some(2), Some(1), "a", &[]),
                dbm("FaZe", "FUR", Some(2), Some(0), "a", &[]),
                dbm("MOUZ", "HER", Some(1), Some(2), "b", &[]),
                dbm("G2", "C9", Some(2), Some(0), "a", &[]),
                dbm("SPIRIT", "AST", Some(2), Some(1), "a", &[]),
                dbm("BIG", "TL", Some(0), Some(2), "b", &[]),
                dbm("NIP", "VP", Some(2), Some(1), "a", &[]),
            ],
        ),
        dbr(
            "Quarterfinals",
            "",
            vec![
                dbm("NAVI", "VIT", Some(2), Some(1), "a", &[(0, 0), (0, 1)]),
                dbm("FaZe", "HER", Some(2), Some(0), "a", &[(0, 2), (0, 3)]),
                dbm("G2", "SPIRIT", Some(1), Some(2), "b", &[(0, 4), (0, 5)]),
                dbm("TL", "NIP", Some(2), Some(1), "a", &[(0, 6), (0, 7)]),
            ],
        ),
        dbr(
            "Semifinals",
            "",
            vec![
                dbm("NAVI", "FaZe", Some(1), Some(2), "b", &[(1, 0), (1, 1)]),
                dbm("SPIRIT", "TL", Some(2), Some(0), "a", &[(1, 2), (1, 3)]),
            ],
        ),
        dbr("Final", "", vec![dbm("FaZe", "SPIRIT", None, None, "", &[(2, 0), (2, 1)])]),
    ];
    let standings = vec![
        drow(1, "FaZe", 3, 1, 8, 4),
        drow(2, "SPIRIT", 3, 1, 7, 4),
        drow(3, "NAVI", 2, 1, 5, 3),
        drow(4, "TL", 2, 1, 4, 3),
        drow(5, "VIT", 1, 1, 3, 3),
        drow(6, "G2", 1, 1, 3, 3),
        drow(7, "HER", 1, 1, 2, 3),
        drow(8, "NIP", 1, 1, 3, 3),
    ];
    EventInfo {
        event: "PGL Major".to_string(),
        tournament_id: 0,
        stage: String::new(),
        game: Game::Cs2,
        standings,
        rounds,
        swiss: Vec::new(),
    }
}

// ----- Contrived demo brackets (stress the layout engine) ------------------

/// Single-elim with byes: two first-round matches feed the semifinals, where the
/// other two semifinalists got byes — so each semifinal has a single feeder.
/// Exercises lone-feeder centring (a fed match sitting level with its one parent).
fn demo_event_byes() -> EventInfo {
    let rounds = vec![
        dbr(
            "First Round",
            "",
            vec![
                dbm("Alpha", "Bravo", Some(2), Some(0), "a", &[]),
                dbm("Charlie", "Delta", Some(2), Some(1), "a", &[]),
            ],
        ),
        dbr(
            "Semifinals",
            "",
            vec![
                // The second seed in each got a bye (no feeder for that slot).
                dbm("Alpha", "Echo", Some(2), Some(1), "a", &[(0, 0)]),
                dbm("Foxtrot", "Charlie", Some(2), Some(0), "a", &[(0, 1)]),
            ],
        ),
        dbr("Final", "", vec![dbm("Alpha", "Foxtrot", None, None, "", &[(1, 0), (1, 1)])]),
    ];
    demo_bracket_event("Bye Cup", rounds)
}

/// Double-elim ending in a bracket reset: the lower-bracket team wins the first
/// grand final, forcing a decider — so the "final" section has two columns.
fn demo_event_reset() -> EventInfo {
    let rounds = vec![
        dbr(
            "Semifinals",
            "upper",
            vec![
                dbm("Sun", "Moon", Some(2), Some(0), "a", &[]),
                dbm("Star", "Comet", Some(2), Some(1), "a", &[]),
            ],
        ),
        dbr("Final", "upper", vec![dbm("Sun", "Star", Some(2), Some(1), "a", &[(0, 0), (0, 1)])]),
        dbr("Round 1", "lower", vec![dbm("Moon", "Comet", Some(2), Some(0), "a", &[(0, 0), (0, 1)])]),
        dbr("Final", "lower", vec![dbm("Moon", "Star", Some(2), Some(1), "a", &[(2, 0), (1, 0)])]),
        // Grand final (lower-bracket team wins) → reset.
        dbr("Grand Final", "final", vec![dbm("Sun", "Moon", Some(2), Some(3), "b", &[(1, 0), (3, 0)])]),
        dbr("Reset", "final", vec![dbm("Sun", "Moon", Some(3), Some(2), "a", &[(4, 0)])]),
    ];
    demo_bracket_event("Reset Cup", rounds)
}

/// The full 8-team double-elim: a 3-round winner's bracket and a 4-round loser's
/// bracket whose major rounds each take a winner's-bracket dropout (feeders from
/// two different columns) — the shape that broke the old nth-child connectors.
fn demo_event_gauntlet() -> EventInfo {
    let rounds = vec![
        // Winner's bracket: QF → SF → Final.
        dbr(
            "Quarterfinals",
            "upper",
            vec![
                dbm("A", "B", Some(2), Some(0), "a", &[]),
                dbm("C", "D", Some(2), Some(1), "a", &[]),
                dbm("E", "F", Some(2), Some(0), "a", &[]),
                dbm("G", "H", Some(2), Some(1), "a", &[]),
            ],
        ),
        dbr(
            "Semifinals",
            "upper",
            vec![
                dbm("A", "C", Some(2), Some(1), "a", &[(0, 0), (0, 1)]),
                dbm("E", "G", Some(2), Some(0), "a", &[(0, 2), (0, 3)]),
            ],
        ),
        dbr("Final", "upper", vec![dbm("A", "E", Some(2), Some(1), "a", &[(1, 0), (1, 1)])]),
        // Loser's bracket: R1 (QF dropouts) → R2 (+ SF dropouts) → R3 → Final (+ WB final dropout).
        dbr(
            "Round 1",
            "lower",
            vec![
                dbm("B", "D", Some(2), Some(1), "a", &[(0, 0), (0, 1)]),
                dbm("F", "H", Some(2), Some(0), "a", &[(0, 2), (0, 3)]),
            ],
        ),
        dbr(
            "Round 2",
            "lower",
            vec![
                // LB-R1 winner vs the SF dropout (cross-section feeder, not drawn).
                dbm("B", "C", Some(2), Some(1), "a", &[(3, 0), (1, 0)]),
                dbm("F", "G", Some(2), Some(0), "a", &[(3, 1), (1, 1)]),
            ],
        ),
        dbr("Round 3", "lower", vec![dbm("B", "F", Some(2), Some(1), "a", &[(4, 0), (4, 1)])]),
        // LB final: LB-R3 winner vs the WB-final dropout.
        dbr("Final", "lower", vec![dbm("B", "E", Some(2), Some(0), "a", &[(5, 0), (2, 0)])]),
        dbr("Grand Final", "final", vec![dbm("A", "B", Some(3), Some(2), "a", &[(2, 0), (6, 0)])]),
    ];
    demo_bracket_event("Gauntlet", rounds)
}

/// A wide 32-team single-elim (Round of 32 → Final), to stress canvas height and
/// horizontal scroll.
fn demo_event_wide() -> EventInfo {
    let teams: Vec<String> = (1..=32).map(|i| format!("T{i:02}")).collect();
    demo_bracket_event("Mega Bracket", demo_se_rounds(&teams))
}

/// Long / multi-word team names, to check they truncate inside the fixed box
/// rather than overflowing it (the score) or wrapping past its height.
fn demo_event_long_names() -> EventInfo {
    let teams: Vec<String> = [
        "kukkosoosi",
        "EEC",
        "EnRo GRIFFINS",
        "dobrywieczor",
        "Power Rangers",
        "LeoGaming",
        "ENCE Academy",
        "FcottoNd",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect();
    demo_bracket_event("Long Names Cup", demo_se_rounds(&teams))
}

/// Build a clean single-elimination tree from a power-of-two team list; each
/// match's winner is its `team_a`, folded up to the final.
fn demo_se_rounds(teams: &[String]) -> Vec<BracketRound> {
    fn title(matches: usize) -> &'static str {
        match matches {
            16 => "Round of 32",
            8 => "Round of 16",
            4 => "Quarterfinals",
            2 => "Semifinals",
            1 => "Final",
            _ => "Round",
        }
    }
    let first: Vec<BracketMatch> = teams
        .chunks(2)
        .map(|c| dbm(&c[0], &c[1], Some(2), Some(1), "a", &[]))
        .collect();
    let mut rounds = vec![dbr(title(first.len()), "", first)];
    while rounds.last().map_or(0, |r| r.matches.len()) > 1 {
        let r = rounds.len() - 1;
        let winners: Vec<(String, String)> = rounds[r]
            .matches
            .chunks(2)
            .map(|c| (c[0].team_a.clone(), c[1].team_a.clone()))
            .collect();
        let ms: Vec<BracketMatch> = winners
            .iter()
            .enumerate()
            .map(|(i, (a, b))| dbm(a, b, Some(2), Some(1), "a", &[(r, 2 * i), (r, 2 * i + 1)]))
            .collect();
        rounds.push(dbr(title(ms.len()), "", ms));
    }
    rounds
}

/// A bracket-only demo event (no standings) for the contrived layouts above.
fn demo_bracket_event(name: &str, rounds: Vec<BracketRound>) -> EventInfo {
    EventInfo {
        event: name.to_string(),
        tournament_id: 0,
        stage: String::new(),
        game: Game::Cs2,
        standings: Vec::new(),
        rounds,
        swiss: Vec::new(),
    }
}

// ----- Demo fixture (no token) ---------------------------------------------

fn demo_team(label: &str, score: Option<i64>) -> NormTeam {
    NormTeam {
        label: label.to_string(),
        name: label.to_string(),
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
        league_url: None,
        // Give the demo IEM a dated edition so the event/match pages can show
        // the combined "IEM Katowice 2026" name; league seasons stay bare.
        serie_name: if league == "IEM" { "Katowice 2026".to_string() } else { String::new() },
        tier: tier.to_string(),
        begin_at,
        status,
        best_of: Some(best_of),
        team_a: a,
        team_b: b,
        stream_url: Some("https://www.twitch.tv/".to_string()),
        tournament_id: Some(demo_tournament_id(league)),
        venue_tz: None,
        streams: demo_streams(),
        mlb_series: None,
    }
}

/// Stable per-league tournament id for the demo, so all of an event's matches
/// share one tournament (and thus one standings/bracket).
fn demo_tournament_id(league: &str) -> i64 {
    9000 + i64::from(league.bytes().map(u32::from).sum::<u32>() % 900)
}

/// A handful of demo broadcasts (official + a language variant + a co-streamer).
fn demo_streams() -> Vec<StreamView> {
    let s = |url: &str, language: &str, official: bool, main: bool| StreamView {
        url: url.to_string(),
        language: language.to_string(),
        official,
        main,
        group: if official { "official" } else { "costream" }.to_string(),
        ..Default::default()
    };
    vec![
        s("https://www.twitch.tv/esl_csgo", "en", true, true),
        s("https://www.twitch.tv/esl_csgob", "en", true, false),
        s("https://www.twitch.tv/ru_esl", "ru", true, false),
        s("https://www.twitch.tv/ohnepixel", "en", false, false),
    ]
}

/// Demo data anchored to `now` so the page is populated without a token.
fn demo_matches(now: DateTime<Utc>) -> Vec<NormalizedMatch> {
    use MatchStatus::{Finished, Live, Upcoming};
    let h = Duration::hours;
    let m = Duration::minutes;
    let d = Duration::days;
    let mut out = vec![
        // CS2 — IEM: a playoff day — three finished results, a live game, and an
        // upcoming one (shows an event whose matches are mostly final). The
        // finals stay within ~2h of now so they fall in the homepage's window.
        demo_match(14, Game::Cs2, "IEM", "S", now - m(105), Finished, 3,
            demo_team("NAVI", Some(2)), demo_team("VIT", Some(0))),
        demo_match(15, Game::Cs2, "IEM", "S", now - m(70), Finished, 3,
            demo_team("FaZe", Some(2)), demo_team("MOUZ", Some(1))),
        demo_match(1, Game::Cs2, "IEM", "S", now - m(35), Finished, 3,
            demo_team("NAVI", Some(1)), demo_team("FaZe", Some(2))),
        demo_match(2, Game::Cs2, "IEM", "S", now - m(10), Live, 3,
            demo_team("G2", Some(1)), demo_team("MOUZ", Some(0))),
        demo_match(3, Game::Cs2, "IEM", "S", now + h(2), Upcoming, 3,
            demo_team("VIT", None), demo_team("SPIRIT", None)),
        // CS2 — BLAST: a result + upcoming.
        demo_match(4, Game::Cs2, "BLAST", "S", now - m(50), Finished, 3,
            demo_team("SPIRIT", Some(2)), demo_team("G2", Some(0))),
        demo_match(5, Game::Cs2, "BLAST", "S", now + h(4), Upcoming, 1,
            demo_team("FaZe", None), demo_team("MOUZ", None)),
        // CS2 — ESL Pro League: upcoming only.
        demo_match(6, Game::Cs2, "ESL Pro League", "A", now + h(5), Upcoming, 3,
            demo_team("NIP", None), demo_team("BIG", None)),
        // CS2 — ESL One: a larger double-elimination event.
        demo_match(30, Game::Cs2, "ESL One", "S", now - m(80), Finished, 3,
            demo_team("FaZe", Some(2)), demo_team("SPIRIT", Some(1))),
        demo_match(31, Game::Cs2, "ESL One", "S", now + h(3), Upcoming, 5,
            demo_team("FaZe", None), demo_team("VIT", None)),
        // CS2 — PGL Major: a larger single-elimination event (Round of 16+).
        demo_match(32, Game::Cs2, "PGL Major", "S", now - m(95), Finished, 3,
            demo_team("NAVI", Some(2)), demo_team("FaZe", Some(1))),
        demo_match(33, Game::Cs2, "PGL Major", "S", now + h(6), Upcoming, 3,
            demo_team("SPIRIT", None), demo_team("TL", None)),
        // CS2 — contrived events to examine the bracket layout engine (byes, a
        // grand-final reset, a full double-elim, and a wide 32-team tree).
        demo_match(40, Game::Cs2, "Bye Cup", "S", now + h(7), Upcoming, 3,
            demo_team("Alpha", None), demo_team("Foxtrot", None)),
        demo_match(41, Game::Cs2, "Reset Cup", "S", now + h(8), Upcoming, 5,
            demo_team("Sun", None), demo_team("Moon", None)),
        demo_match(42, Game::Cs2, "Gauntlet", "S", now + h(9), Upcoming, 3,
            demo_team("A", None), demo_team("B", None)),
        demo_match(43, Game::Cs2, "Mega Bracket", "S", now + h(10), Upcoming, 3,
            demo_team("T01", None), demo_team("T32", None)),
        demo_match(44, Game::Cs2, "Long Names Cup", "S", now + h(11), Upcoming, 3,
            demo_team("kukkosoosi", None), demo_team("FcottoNd", None)),
        // LoL — LCK: a result + upcoming.
        demo_match(7, Game::Lol, "LCK", "A", now - m(35), Finished, 3,
            demo_team("T1", Some(2)), demo_team("GEN", Some(1))),
        demo_match(8, Game::Lol, "LCK", "A", now + h(3), Upcoming, 3,
            demo_team("HLE", None), demo_team("DK", None)),
        // LoL — LEC: a result (FNC win) + upcoming next day.
        demo_match(9, Game::Lol, "LEC", "A", now - m(20), Finished, 3,
            demo_team("G2", Some(1)), demo_team("FNC", Some(2))),
        demo_match(10, Game::Lol, "LEC", "A", now + d(1), Upcoming, 3,
            demo_team("MAD", None), demo_team("VIT", None)),
        // LoL — LPL: upcoming.
        demo_match(11, Game::Lol, "LPL", "A", now + h(6), Upcoming, 3,
            demo_team("BLG", None), demo_team("TES", None)),
        // LoL — MSI / Worlds: bigger upcoming events, further out.
        demo_match(12, Game::Lol, "MSI", "S", now + d(1) + h(2), Upcoming, 5,
            demo_team("T1", None), demo_team("BLG", None)),
        demo_match(13, Game::Lol, "Worlds", "S", now + d(3), Upcoming, 5,
            demo_team("GEN", None), demo_team("JDG", None)),
    ];

    // Finished results across the prior ~2 weeks so "show earlier days" and the
    // calendar have history to render in demo mode. Reuse the same leagues so
    // their seeded standings/brackets apply to the detail pages.
    let cs_events = ["IEM", "BLAST", "ESL Pro League"];
    let lol_events = ["LCK", "LEC", "LPL"];
    let cs_teams = [
        ("NAVI", "FaZe"), ("VIT", "MOUZ"), ("G2", "SPIRIT"),
        ("BIG", "NIP"), ("FaZe", "VIT"), ("MOUZ", "NAVI"),
    ];
    let lol_teams = [
        ("T1", "GEN"), ("HLE", "DK"), ("BLG", "TES"),
        ("G2", "FNC"), ("GEN", "T1"), ("DK", "BLG"),
    ];
    let mut id = 100;
    for day in 1..=12i64 {
        let i = (day - 1) as usize;
        let (a, b) = cs_teams[i % cs_teams.len()];
        out.push(demo_match(id, Game::Cs2, cs_events[i % cs_events.len()], "S",
            now - d(day) - h(3), Finished, 3, demo_team(a, Some(2)), demo_team(b, Some(1))));
        id += 1;
        let (a, b) = lol_teams[i % lol_teams.len()];
        out.push(demo_match(id, Game::Lol, lol_events[i % lol_events.len()], "A",
            now - d(day) - h(2), Finished, 3, demo_team(a, Some(2)), demo_team(b, Some(0))));
        id += 1;
    }
    // Match the live poll path (which loads DB-sorted), so day grouping — which
    // relies on time order — produces one group per day.
    out.sort_by_key(|m| m.begin_at);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(begin: DateTime<Utc>, status: MatchStatus) -> NormalizedMatch {
        NormalizedMatch {
            id: 1,
            game: Game::Cs2,
            league: "X".into(),
            league_url: None,
            serie_name: String::new(),
            tier: "S".into(),
            begin_at: begin,
            status,
            best_of: Some(3),
            team_a: NormTeam {
                label: "A".into(),
                name: "A".into(),
                score: None,
            },
            team_b: NormTeam {
                label: "B".into(),
                name: "B".into(),
                score: None,
            },
            stream_url: None,
            tournament_id: None,
            venue_tz: None,
            streams: Vec::new(),
            mlb_series: None,
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

    #[test]
    fn time_label_24h_and_12h() {
        let t = |h, mi| Tz::UTC.with_ymd_and_hms(2026, 6, 21, h, mi, 0).unwrap();
        assert_eq!(time_label(t(18, 30), true), "18:30");
        assert_eq!(time_label(t(18, 30), false), "6:30 PM");
        assert_eq!(time_label(t(0, 5), true), "00:05");
        assert_eq!(time_label(t(0, 5), false), "12:05 AM");
        assert_eq!(time_label(t(12, 0), false), "12:00 PM");
    }

    #[test]
    fn effective_status_live_heuristic() {
        let now = Utc::now();
        let mut m = at(now - Duration::hours(1), MatchStatus::Upcoming);
        assert_eq!(effective_status(&m, now), MatchStatus::Live);
        m.begin_at = now + Duration::hours(1);
        assert_eq!(effective_status(&m, now), MatchStatus::Upcoming);
        m.status = MatchStatus::Finished;
        m.begin_at = now - Duration::hours(1);
        assert_eq!(effective_status(&m, now), MatchStatus::Finished);
    }

    #[test]
    fn to_view_only_shows_scores_when_finished() {
        let now = Utc::now();
        let mut fin = at(now - Duration::hours(2), MatchStatus::Finished);
        fin.team_a.score = Some(2);
        fin.team_b.score = Some(1);
        let v = to_view(&fin, &Tz::UTC, now, true);
        assert_eq!(v.team_a.score, Some(2));
        assert!(v.team_a.winner && !v.team_b.winner);

        // Placeholder 0-0 on an unplayed match must be hidden.
        let mut up = at(now + Duration::hours(2), MatchStatus::Upcoming);
        up.team_a.score = Some(0);
        up.team_b.score = Some(0);
        let v = to_view(&up, &Tz::UTC, now, true);
        assert_eq!(v.team_a.score, None);
        assert_eq!(v.team_b.score, None);
    }

    #[test]
    fn to_view_shows_live_scores_only_when_nonzero() {
        let now = Utc::now();
        // A live match with a real partial score: shown, but no winner yet.
        let mut live = at(now - Duration::minutes(10), MatchStatus::Live);
        live.team_a.score = Some(1);
        live.team_b.score = Some(0);
        let v = to_view(&live, &Tz::UTC, now, true);
        assert_eq!(v.team_a.score, Some(1));
        assert!(!v.team_a.winner && !v.team_b.winner, "no winner mid-match");

        // A live match still at the 0-0 placeholder stays hidden.
        let mut live0 = at(now - Duration::minutes(10), MatchStatus::Live);
        live0.team_a.score = Some(0);
        live0.team_b.score = Some(0);
        let v = to_view(&live0, &Tz::UTC, now, true);
        assert_eq!(v.team_a.score, None);
    }

    #[test]
    fn group_days_buckets_by_day_then_league() {
        let ms = |h| {
            Tz::UTC
                .with_ymd_and_hms(2026, 6, 21, h, 0, 0)
                .unwrap()
                .timestamp_millis()
        };
        let day2 = Tz::UTC
            .with_ymd_and_hms(2026, 6, 22, 1, 0, 0)
            .unwrap()
            .timestamp_millis();
        let mk = |at_ms: i64, league: &str, bo: &str| MatchView {
            id: at_ms,
            game: Game::Lol,
            league: league.into(),
            serie_name: String::new(),
            tier: "S".into(),
            status: MatchStatus::Upcoming,
            clock_label: String::new(),
            venue_label: String::new(),
            best_of: bo.into(),
            team_a: TeamView {
                label: "A".into(),
                name: "A".into(),
                score: None,
                winner: false,
            },
            team_b: TeamView {
                label: "B".into(),
                name: "B".into(),
                score: None,
                winner: false,
            },
            stream_url: None,
            event_url: String::new(),
            begin_at_ms: at_ms,
        };
        let views = vec![
            mk(ms(1), "LCK", "Bo3"),
            mk(ms(2), "LEC", "Bo3"),
            mk(ms(3), "LCK", "Bo5"),
            mk(day2, "LCK", "Bo3"),
        ];
        let days = group_days(views, &Tz::UTC);
        assert_eq!(days.len(), 2);
        assert_eq!(days[0].leagues.len(), 2);
        assert_eq!(days[0].leagues[0].league, "LCK");
        assert_eq!(days[0].leagues[0].matches.len(), 2);
        assert_eq!(days[0].leagues[0].bo, None, "mixed Bo3/Bo5 => no header bo");
        assert_eq!(days[0].leagues[1].league, "LEC");
        assert_eq!(days[0].leagues[1].bo, Some("Bo3".to_string()));
        assert_eq!(days[1].leagues[0].bo, Some("Bo3".to_string()));
    }

    #[test]
    fn group_days_keeps_each_game_together() {
        let ms = |h| {
            Tz::UTC
                .with_ymd_and_hms(2026, 6, 21, h, 0, 0)
                .unwrap()
                .timestamp_millis()
        };
        let mk = |at_ms: i64, game: Game, league: &str| MatchView {
            id: at_ms,
            game,
            league: league.into(),
            serie_name: String::new(),
            tier: "S".into(),
            status: MatchStatus::Upcoming,
            clock_label: String::new(),
            venue_label: String::new(),
            best_of: "Bo3".into(),
            team_a: TeamView {
                label: "A".into(),
                name: "A".into(),
                score: None,
                winner: false,
            },
            team_b: TeamView {
                label: "B".into(),
                name: "B".into(),
                score: None,
                winner: false,
            },
            stream_url: None,
            event_url: String::new(),
            begin_at_ms: at_ms,
        };
        // Interleaved by time: LoL, CS2, LoL, CS2.
        let views = vec![
            mk(ms(1), Game::Lol, "LCK"),
            mk(ms(2), Game::Cs2, "IEM"),
            mk(ms(3), Game::Lol, "LEC"),
            mk(ms(4), Game::Cs2, "BLAST"),
        ];
        let days = group_days(views, &Tz::UTC);
        let order: Vec<&str> = days[0].leagues.iter().map(|l| l.league.as_str()).collect();
        // CS2 events first (time order), then LoL events.
        assert_eq!(order, vec!["IEM", "BLAST", "LCK", "LEC"]);
    }

    #[test]
    fn group_days_splits_editions_of_one_league() {
        let ms = |h| {
            Tz::UTC
                .with_ymd_and_hms(2026, 6, 21, h, 0, 0)
                .unwrap()
                .timestamp_millis()
        };
        let mk = |at_ms: i64, serie: &str| MatchView {
            id: at_ms,
            game: Game::Cs2,
            league: "IEM".into(),
            serie_name: serie.into(),
            tier: "S".into(),
            status: MatchStatus::Upcoming,
            clock_label: String::new(),
            venue_label: String::new(),
            best_of: "Bo3".into(),
            team_a: TeamView { label: "A".into(), name: "A".into(), score: None, winner: false },
            team_b: TeamView { label: "B".into(), name: "B".into(), score: None, winner: false },
            stream_url: None,
            event_url: String::new(),
            begin_at_ms: at_ms,
        };
        let views = vec![
            mk(ms(1), "Cologne Major"),
            mk(ms(2), "Katowice"),
            mk(ms(3), "Cologne Major"),
        ];
        let days = group_days(views, &Tz::UTC);
        assert_eq!(days.len(), 1);
        // Same league, two editions => two groups (one per edition).
        assert_eq!(days[0].leagues.len(), 2);
        let cologne = days[0]
            .leagues
            .iter()
            .find(|l| l.serie_name == "Cologne Major")
            .expect("Cologne group");
        assert_eq!(cologne.matches.len(), 2, "both Cologne matches grouped");
        assert_eq!(
            crate::types::full_event_name(&cologne.league, &cologne.serie_name),
            "IEM Cologne Major"
        );
    }

    #[test]
    fn resolve_tz_falls_back() {
        assert_eq!(resolve_tz("Europe/London", Tz::UTC), chrono_tz::Europe::London);
        assert_eq!(resolve_tz("", Tz::UTC), Tz::UTC);
        assert_eq!(resolve_tz("Not/AZone", Tz::UTC), Tz::UTC);
    }

    #[test]
    fn event_link_official_else_liquipedia() {
        assert_eq!(
            event_link(Game::Lol, "MSI", Some("https://lolesports.com")),
            "https://lolesports.com"
        );
        let lol = event_link(Game::Lol, "Mid-Season Invitational", None);
        assert!(lol.starts_with("https://liquipedia.net/leagueoflegends/index.php?search="));
        assert!(lol.contains("Mid-Season%20Invitational"));
        // Blank official falls back too, with the CS wiki.
        let cs = event_link(Game::Cs2, "BLAST Premier", Some("  "));
        assert!(cs.starts_with("https://liquipedia.net/counterstrike/index.php?search="));
        assert!(cs.contains("BLAST%20Premier"));
    }

    #[test]
    fn merge_matches_replaces_ok_games_keeps_errored() {
        let now = Utc::now();
        let mk = |id, game, h| {
            let mut m = at(now + Duration::hours(h), MatchStatus::Upcoming);
            m.id = id;
            m.game = game;
            m
        };
        let prev = vec![mk(1, Game::Cs2, 1), mk(2, Game::Lol, 2)];
        let fresh = vec![mk(3, Game::Cs2, 3)]; // cs2 refetched; lol failed this round
        let old_cutoff = (now - Duration::days(365)).timestamp_millis();
        let merged = merge_matches(&prev, fresh, &[Game::Lol], old_cutoff);
        // Old cs2 (id 1) replaced by fresh (id 3); lol (id 2) retained; sorted.
        assert_eq!(merged.iter().map(|m| m.id).collect::<Vec<_>>(), vec![2, 3]);
        assert!(!merged.iter().any(|m| m.id == 1));
    }

    #[test]
    fn refresh_interval_curve() {
        assert_eq!(refresh_interval(1), Some(Duration::hours(6)));
        assert_eq!(refresh_interval(2), Some(Duration::hours(12)));
        assert_eq!(refresh_interval(3), Some(Duration::hours(24)));
        assert_eq!(refresh_interval(7), Some(Duration::hours(48)));
        // Today (0) is the main poll's job; > 1 week is locked.
        assert_eq!(refresh_interval(0), None);
        assert_eq!(refresh_interval(8), None);
    }

    #[test]
    fn headroom_respects_floor() {
        assert!(headroom_ok(None, 200)); // unknown → assume OK
        assert!(headroom_ok(Some(200), 200));
        assert!(headroom_ok(Some(500), 200));
        assert!(!headroom_ok(Some(199), 200));
    }

    #[test]
    fn utc_day_bounds_are_half_open() {
        let day = NaiveDate::from_ymd_opt(2026, 5, 1).unwrap();
        let (start, end) = utc_day_bounds(day);
        assert_eq!(start.to_rfc3339(), "2026-05-01T00:00:00+00:00");
        assert_eq!(end.to_rfc3339(), "2026-05-02T00:00:00+00:00");
    }

    #[test]
    fn merge_matches_drops_matches_older_than_cutoff() {
        let now = Utc::now();
        let mk = |id, h| {
            let mut m = at(now + Duration::hours(h), MatchStatus::Finished);
            m.id = id;
            m
        };
        let prev = vec![];
        let fresh = vec![mk(1, -240), mk(2, -2)]; // 10 days ago, 2 hours ago
        let cutoff = (now - Duration::days(7)).timestamp_millis();
        let merged = merge_matches(&prev, fresh, &[], cutoff);
        assert_eq!(merged.iter().map(|m| m.id).collect::<Vec<_>>(), vec![2]);
    }
}
