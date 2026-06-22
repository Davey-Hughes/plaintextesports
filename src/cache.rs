//! In-memory schedule cache + background poller + view builders (server-only).
//!
//! A single background task polls PandaScore on an interval and stores a
//! normalized, tier-1-filtered snapshot behind an `RwLock`. Page requests read
//! the snapshot and format it into [`ScheduleView`] for the requested timezone —
//! they never touch the network. When no API token is configured we populate a
//! demo fixture (relative to "now") so the UI is always usable.

use crate::config::Config;
use crate::pandascore::{fetch_game, FetchResult, NormTeam, NormalizedMatch};
use crate::types::{DayGroup, Game, LeagueGroup, MatchStatus, MatchView, ScheduleView, TeamView};
use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use once_cell::sync::Lazy;
use std::collections::HashMap;
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

/// Keep matches whose start is within this many days of "now" in the DB; older
/// ones are pruned after each poll.
const RETENTION_DAYS: i64 = 2;

/// Start the background poller. Loads any persisted matches first (so a restart
/// serves data immediately), then either polls PandaScore or, with no token,
/// falls back to demo data.
pub fn spawn_poller() {
    let cfg = Config::from_env();

    // DEMO mode: serve fixture data regardless of token/DB (and don't poll).
    if cfg.demo {
        leptos::logging::log!("DEMO mode — serving fixture data (ignoring token + cache db)");
        let now = Utc::now();
        let mut snap = SNAPSHOT.write().unwrap();
        snap.matches = demo_matches(now);
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
                let mut snap = SNAPSHOT.write().unwrap();
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
            let mut map = EVENT_LINKS.write().unwrap();
            for (key, url, checked_at_ms) in rows {
                let checked_at =
                    DateTime::from_timestamp_millis(checked_at_ms).unwrap_or_else(Utc::now);
                map.insert(key, EventLink { url, checked_at });
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

        let mut last_deep: Option<DateTime<Utc>> = None;
        loop {
            let now = Utc::now();
            let active = {
                let snap = SNAPSHOT.read().unwrap();
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
            // the (non-Sync) DB connection across an await point.
            let (cs_res, lol_res) = tokio::join!(
                fetch_game(&client, &token, Game::Cs2, window_end, deep),
                fetch_game(&client, &token, Game::Lol, window_end, deep),
            );
            apply_poll(cs_res, lol_res, store.as_mut());

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
/// Merge freshly fetched matches with the previous set, keeping the previous
/// matches for any game that failed this round. Sorted by start time.
fn merge_matches(
    prev: &[NormalizedMatch],
    mut fresh: Vec<NormalizedMatch>,
    errored: &[Game],
) -> Vec<NormalizedMatch> {
    for &game in errored {
        fresh.extend(prev.iter().filter(|m| m.game == game).cloned());
    }
    fresh.sort_by_key(|m| m.begin_at);
    fresh
}

fn merge_in_memory(
    matches: Vec<NormalizedMatch>,
    errored: &[Game],
    any_ok: bool,
    any_err: bool,
    now: DateTime<Utc>,
) {
    let mut snap = SNAPSHOT.write().unwrap();
    let merged = merge_matches(&snap.matches, matches, errored);
    snap.matches = merged;
    snap.using_fixture = false;
    snap.stale = any_err;
    if any_ok {
        snap.fetched_at = now;
    }
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
    let snap = SNAPSHOT.read().unwrap();
    let links = EVENT_LINKS.read().unwrap();
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for m in &snap.matches {
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
        let mut links = EVENT_LINKS.write().unwrap();
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
    // trust scores once a match is finished.
    let show_scores = status == MatchStatus::Finished;
    let mut team_a = TeamView {
        label: m.team_a.label.clone(),
        score: show_scores.then_some(m.team_a.score).flatten(),
        winner: false,
    };
    let mut team_b = TeamView {
        label: m.team_b.label.clone(),
        score: show_scores.then_some(m.team_b.score).flatten(),
        winner: false,
    };
    if let (Some(a), Some(b)) = (team_a.score, team_b.score) {
        team_a.winner = a > b;
        team_b.winner = b > a;
    }

    MatchView {
        id: m.id,
        game: m.game,
        league: m.league.clone(),
        tier: m.tier.clone(),
        status,
        clock_label: time_label(local, hour24),
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
    if let Some(EventLink { url: Some(u), .. }) = EVENT_LINKS.read().unwrap().get(&key) {
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
        if let Some(lg) = day.leagues.iter_mut().find(|l| l.league == v.league) {
            lg.matches.push(v);
        } else {
            day.leagues.push(LeagueGroup {
                league: v.league.clone(),
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
            None => 2,
        });
    }
    days
}

#[allow(clippy::too_many_arguments)]
fn matches_in_window(
    snap: &Snapshot,
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
        .filter(|m| game.is_none_or(|g| m.game == g))
        .filter(|m| m.begin_at >= start && m.begin_at < end)
        .map(|m| to_view(m, tz, now, hour24))
        .collect()
}

/// Parse an IANA tz name, falling back to the configured default.
fn resolve_tz(name: &str, default: Tz) -> Tz {
    name.parse::<Tz>().unwrap_or(default)
}

/// Homepage: live matches pinned, then the next `upcoming_days` grouped by day.
#[must_use]
pub fn homepage_view(game_filter: &str, tz_name: &str, hour24: bool) -> ScheduleView {
    let cfg = Config::from_env();
    let tz = resolve_tz(tz_name, cfg.tz);
    let game = Game::from_filter(game_filter);
    let now = Utc::now();
    let start = local_day_start(&tz, now.with_timezone(&tz).date_naive());
    let end = now + Duration::days(cfg.upcoming_days);

    let snap = SNAPSHOT.read().unwrap();
    let all = matches_in_window(&snap, game, start, end, &tz, now, hour24);

    ScheduleView {
        days: group_days(all, &tz),
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

/// Single-day view with prev/next navigation.
#[must_use]
pub fn day_view(date: &str, game_filter: &str, tz_name: &str, hour24: bool) -> ScheduleView {
    let cfg = Config::from_env();
    let tz = resolve_tz(tz_name, cfg.tz);
    let game = Game::from_filter(game_filter);
    let now = Utc::now();

    let day = NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .unwrap_or_else(|_| now.with_timezone(&tz).date_naive());
    let start = local_day_start(&tz, day);
    let end = local_day_start(&tz, day + Duration::days(1));

    let snap = SNAPSHOT.read().unwrap();
    let all = matches_in_window(&snap, game, start, end, &tz, now, hour24);

    let local_noon = tz
        .from_local_datetime(&day.and_hms_opt(12, 0, 0).unwrap())
        .earliest()
        .unwrap_or_else(|| tz.from_utc_datetime(&day.and_hms_opt(12, 0, 0).unwrap()));

    ScheduleView {
        days: group_days(all, &tz),
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

// ----- Game/event subscription expansion -----------------------------------

/// A reminder to create for one upcoming match (from a game/event subscription).
pub struct ReminderSeed {
    pub match_id: i64,
    pub game: String,
    pub league: String,
    pub notify_at_ms: i64,
    pub title: String,
    pub body: String,
    pub url: String,
}

/// Upcoming matches matching a subscription scope, as reminder seeds. `kind` is
/// "game" (value = "cs2"/"lol") or "league" (value = league name).
#[must_use]
pub fn scope_reminder_seeds(kind: &str, value: &str, lead_ms: i64) -> Vec<ReminderSeed> {
    let cfg = Config::from_env();
    let now = Utc::now();
    let snap = SNAPSHOT.read().unwrap();
    snap.matches
        .iter()
        .filter(|m| m.status != MatchStatus::Canceled && m.begin_at > now)
        .filter(|m| match kind {
            "game" => m.game.slug() == value,
            "league" => m.league == value,
            _ => false,
        })
        .map(|m| {
            let local = m.begin_at.with_timezone(&cfg.tz);
            ReminderSeed {
                match_id: m.id,
                game: m.game.slug().to_string(),
                league: m.league.clone(),
                notify_at_ms: m.begin_at.timestamp_millis() - lead_ms,
                title: format!("{} vs {}", m.team_a.label, m.team_b.label),
                body: format!("{} · {}", m.league, time_label(local, false)),
                url: resolved_event_url(m.game, &m.league, m.begin_at, m.league_url.as_deref()),
            }
        })
        .collect()
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
        league_url: None,
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
    use MatchStatus::{Finished, Live, Upcoming};
    let h = Duration::hours;
    let m = Duration::minutes;
    let d = Duration::days;
    vec![
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
            demo_team("G2", None), demo_team("MOUZ", None)),
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
            league_url: None,
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
            tier: "S".into(),
            status: MatchStatus::Upcoming,
            clock_label: String::new(),
            best_of: bo.into(),
            team_a: TeamView {
                label: "A".into(),
                score: None,
                winner: false,
            },
            team_b: TeamView {
                label: "B".into(),
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
            tier: "S".into(),
            status: MatchStatus::Upcoming,
            clock_label: String::new(),
            best_of: "Bo3".into(),
            team_a: TeamView {
                label: "A".into(),
                score: None,
                winner: false,
            },
            team_b: TeamView {
                label: "B".into(),
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
        let merged = merge_matches(&prev, fresh, &[Game::Lol]);
        // Old cs2 (id 1) replaced by fresh (id 3); lol (id 2) retained; sorted.
        assert_eq!(merged.iter().map(|m| m.id).collect::<Vec<_>>(), vec![2, 3]);
        assert!(!merged.iter().any(|m| m.id == 1));
    }
}
