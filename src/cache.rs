//! In-memory schedule cache + background poller + view builders (server-only).
//!
//! A single background task polls PandaScore on an interval and stores a
//! normalized, tier-1-filtered snapshot behind an `RwLock`. Page requests read
//! the snapshot and format it into [`ScheduleView`] for the requested timezone —
//! they never touch the network. When no API token is configured we populate a
//! demo fixture (relative to "now") so the UI is always usable.

use crate::config::{config, Config};
use crate::feed::{FetchResult, NormalizedMatch, NormalizedTeam};
use crate::pandascore::fetch_game;
use crate::twitch::{login_of, LiveInfo};
use crate::types::{
    event_name_eq, full_event_name, BracketMatch, BracketRound, DayGroup, EventInfo, LeagueGroup,
    MatchStatus, MatchView, ScheduleView, SourceLink, Sport, StandingRow, StreamView, TeamView,
};
use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::{Mutex, PoisonError, RwLock};

#[doc(hidden)]
pub struct Snapshot {
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

/// Cache of event-page links keyed by `event_key` (sport|league|year). Read on
/// every render (to_view), written by the poll-time resolver.
static EVENT_LINKS: Lazy<RwLock<HashMap<String, EventLink>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

/// Re-query a "not found" event link only after this many days (catches pages
/// created after an event starts, without hammering Liquipedia).
const NEG_TTL_DAYS: i64 = 3;
/// Cap Liquipedia lookups per poll cycle (the rest resolve next cycle).
const MAX_RESOLVE_PER_CYCLE: usize = 10;

fn event_key(sport: Sport, league: &str, year: i32) -> String {
    format!("{}|{}|{}", sport.slug(), league, year)
}

/// Per-tournament standings + bracket, fetched on demand and cached with a TTL.
static EVENTS: Lazy<RwLock<HashMap<i64, CachedEvent>>> = Lazy::new(|| RwLock::new(HashMap::new()));

// ----- Traditional-sport standings caches ---------------------------------
//
// Each team sport keeps its current standings tables in a process-wide cache the
// poller refreshes (empty until the first successful fetch). MLB/NHL are division
// lists, NBA/NFL conference lists; soccer spans multiple competitions under one
// `Sport`, so its tables are keyed by competition name. They share the accessors
// below: `standings_for_event_name` (the whole-league `/event` page) and
// `team_standings_for_match` (the table[s] a match's two sides belong to).
static MLB_STANDINGS: Lazy<RwLock<Vec<EventInfo>>> = Lazy::new(|| RwLock::new(Vec::new()));
static NHL_STANDINGS: Lazy<RwLock<Vec<EventInfo>>> = Lazy::new(|| RwLock::new(Vec::new()));
static NBA_STANDINGS: Lazy<RwLock<Vec<EventInfo>>> = Lazy::new(|| RwLock::new(Vec::new()));
static NFL_STANDINGS: Lazy<RwLock<Vec<EventInfo>>> = Lazy::new(|| RwLock::new(Vec::new()));
static SOCCER_STANDINGS: Lazy<RwLock<HashMap<String, Vec<EventInfo>>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));
// WRC/WEC championship standings (Orange Cat Blacktop), refreshed by the poller
// and served from here — never fetched per request, to respect the daily quota.
static WRC_STANDINGS: Lazy<RwLock<crate::types::MotorStandings>> =
    Lazy::new(|| RwLock::new(crate::types::MotorStandings::default()));
static WEC_STANDINGS: Lazy<RwLock<crate::types::MotorStandings>> =
    Lazy::new(|| RwLock::new(crate::types::MotorStandings::default()));
static MOTOGP_STANDINGS: Lazy<RwLock<crate::types::MotorStandings>> =
    Lazy::new(|| RwLock::new(crate::types::MotorStandings::default()));

/// The WRC, WEC, or MotoGP championship standings (empty until first fetched), by
/// series league name. Any other name yields empty tables.
#[must_use]
pub fn motor_standings(league: &str) -> crate::types::MotorStandings {
    let store = match league {
        "WRC" => &WRC_STANDINGS,
        "WEC" => &WEC_STANDINGS,
        "MotoGP" => &MOTOGP_STANDINGS,
        _ => return crate::types::MotorStandings::default(),
    };
    store.read().unwrap_or_else(PoisonError::into_inner).clone()
}

/// Snapshot a standings store.
fn read_tables(store: &RwLock<Vec<EventInfo>>) -> Vec<EventInfo> {
    store.read().unwrap_or_else(PoisonError::into_inner).clone()
}

/// The soccer tables for one competition (empty when none are cached yet).
fn soccer_tables(league: &str) -> Vec<EventInfo> {
    SOCCER_STANDINGS
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .get(league)
        .cloned()
        .unwrap_or_default()
}

/// Traditional-sport postseason brackets (soccer World Cup, NFL/NBA/NHL/MLB
/// playoffs), each a bracket-only [`EventInfo`] stage keyed by event/league name.
/// Built from each sport's source in the background loop and appended to the event
/// page after any group/division tables. Empty until first built.
static PLAYOFF_BRACKETS: Lazy<RwLock<HashMap<String, EventInfo>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

// Synthetic tournament-id bases for the playoff brackets — reveal state keys off
// `tournament_id`, so it must be stable across page loads and disjoint from the
// ESPN standings ids (400–700) and PandaScore's real ids. One base per sport
// (1M apart); the season year (added in) keeps editions distinct.
const BRACKET_ID_SOCCER: i64 = 92_000_000;
const BRACKET_ID_NFL: i64 = 93_000_000;
const BRACKET_ID_NBA: i64 = 94_000_000;
const BRACKET_ID_NHL: i64 = 95_000_000;
const BRACKET_ID_MLB: i64 = 96_000_000;

/// The cached bracket stage for an event name, if one has been built.
fn playoff_bracket_stage(event: &str) -> Option<EventInfo> {
    PLAYOFF_BRACKETS
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .get(event)
        .cloned()
}

/// Wrap freshly-built rounds into a bracket-only stage and replace the cache entry
/// — keeping the previous bracket on a fetch error or an empty (off-season / not
/// yet started) result, so the page never blanks a bracket that was showing.
fn update_bracket<E: std::fmt::Display>(
    event: &str,
    tid: i64,
    stage: &str,
    sport: Sport,
    raw: Result<Vec<crate::types::BracketRound>, E>,
) {
    match raw {
        Ok(rounds) if !rounds.is_empty() => {
            let ev = EventInfo {
                event: event.to_string(),
                tournament_id: tid,
                stage: stage.to_string(),
                sport,
                standings: Vec::new(),
                rounds,
                swiss: Vec::new(),
            };
            db_cache_put("playoff_bracket", event, &ev, Utc::now());
            PLAYOFF_BRACKETS
                .write()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(event.to_string(), ev);
        }
        Ok(_) => {}
        Err(e) => leptos::logging::log!("{event} bracket fetch failed: {e}"),
    }
}

/// Fetch every available traditional-sport playoff bracket and refresh the cache.
/// Each builder returns an empty bracket off-season, which `update_bracket` keeps
/// out of the cache. Run on the same cadence as standings (a bracket only changes
/// when a game/series finishes).
#[allow(clippy::let_underscore_future)]
async fn refresh_brackets(client: &reqwest::Client, now: DateTime<Utc>) {
    use crate::espn::{season_year, NBA, NFL, WORLD_CUP};
    let wc_year = now.year();
    // NFL labels its season by the start year but plays the postseason the next
    // January/February; NBA/NHL/MLB postseasons fall in their end year.
    let nfl_year = season_year(Sport::Nfl, now);
    let hoops_year = season_year(Sport::Nba, now);
    let (soccer, nfl, nba, nhl, mlb) = tokio::join!(
        crate::espn::fetch_soccer_bracket(client, &WORLD_CUP, wc_year),
        crate::espn::fetch_nfl_bracket(client, &NFL, nfl_year),
        crate::espn::fetch_nba_bracket(client, &NBA, hoops_year),
        crate::nhl::fetch_bracket(client, hoops_year),
        crate::mlb::fetch_bracket(client, hoops_year),
    );
    update_bracket(
        "World Cup",
        BRACKET_ID_SOCCER + wc_year as i64,
        "Knockout Stage",
        Sport::Soccer,
        soccer,
    );
    update_bracket(
        "NFL",
        BRACKET_ID_NFL + nfl_year as i64,
        "Playoffs",
        Sport::Nfl,
        nfl,
    );
    update_bracket(
        "NBA",
        BRACKET_ID_NBA + hoops_year as i64,
        "Playoffs",
        Sport::Nba,
        nba,
    );
    update_bracket(
        "NHL",
        BRACKET_ID_NHL + hoops_year as i64,
        "Stanley Cup Playoffs",
        Sport::Nhl,
        nhl,
    );
    update_bracket(
        "MLB",
        BRACKET_ID_MLB + hoops_year as i64,
        "Postseason",
        Sport::Mlb,
        mlb,
    );
}

/// The subset of `tables` that contain either team — the one or two
/// divisions/conferences/groups the two sides of a match belong to. Order
/// follows the league-wide ordering.
fn tables_with_team(tables: &[EventInfo], team_a: &str, team_b: &str) -> Vec<EventInfo> {
    tables
        .iter()
        .filter(|t| {
            t.standings
                .iter()
                .any(|r| r.team == team_a || r.team == team_b)
        })
        .cloned()
        .collect()
}

/// League-wide standings for a traditional event page, by event name (e.g. the
/// six MLB divisions for `/event/MLB`). `None` for an esports event, whose
/// standings come from its tournament instead.
pub fn standings_for_event_name(name: &str) -> Option<Vec<EventInfo>> {
    // The league/group tables, then (in postseason) a bracket-only stage appended
    // after them. `bracket_key` is the event name the bracket is cached under.
    let (mut tables, bracket_key) = match name {
        "MLB" => (read_tables(&MLB_STANDINGS), "MLB"),
        "NHL" => (read_tables(&NHL_STANDINGS), "NHL"),
        "NBA" => (read_tables(&NBA_STANDINGS), "NBA"),
        "NFL" => (read_tables(&NFL_STANDINGS), "NFL"),
        "Premier League" => (soccer_tables("Premier League"), ""),
        // The World Cup event name carries its year ("World Cup 2026"); its
        // standings and bracket are keyed on the bare league name.
        n if n.starts_with("World Cup") => (soccer_tables("World Cup"), "World Cup"),
        _ => return None,
    };
    if let Some(bracket) = (!bracket_key.is_empty())
        .then(|| playoff_bracket_stage(bracket_key))
        .flatten()
    {
        tables.push(bracket);
    }
    Some(tables)
}

/// The standings table(s) the two sides of a traditional match belong to (their
/// divisions/conferences/groups), for the match detail page. Empty for esports
/// and F1. MLB keys its standings by short label; the others by full team name.
pub fn team_standings_for_match(
    sport: Sport,
    league: &str,
    a_label: &str,
    a_name: &str,
    b_label: &str,
    b_name: &str,
) -> Vec<EventInfo> {
    match sport {
        Sport::Mlb => tables_with_team(&read_tables(&MLB_STANDINGS), a_label, b_label),
        Sport::Nhl => tables_with_team(&read_tables(&NHL_STANDINGS), a_name, b_name),
        Sport::Nba => tables_with_team(&read_tables(&NBA_STANDINGS), a_name, b_name),
        Sport::Nfl => tables_with_team(&read_tables(&NFL_STANDINGS), a_name, b_name),
        Sport::Soccer => tables_with_team(&soccer_tables(league), a_name, b_name),
        Sport::Cs2 | Sport::Lol | Sport::Motorsport => Vec::new(),
    }
}

/// Replace a standings cache with a freshly fetched set, keeping the old tables
/// on a fetch error or an empty (off-season) response so the page never blanks.
fn update_standings<E: std::fmt::Display>(
    store: &RwLock<Vec<EventInfo>>,
    raw: Result<Vec<EventInfo>, E>,
    name: &str,
    ns: &str,
) {
    match raw {
        Ok(tables) if !tables.is_empty() => {
            db_cache_put(ns, "", &tables, Utc::now());
            *store.write().unwrap_or_else(PoisonError::into_inner) = tables;
        }
        Ok(_) => {}
        Err(e) => leptos::logging::log!("{name} standings fetch failed: {e}"),
    }
}

/// As [`update_standings`], for the competition-keyed soccer cache.
fn update_soccer_standings<E: std::fmt::Display>(league: &str, raw: Result<Vec<EventInfo>, E>) {
    match raw {
        Ok(tables) if !tables.is_empty() => {
            db_cache_put("soccer_standings", league, &tables, Utc::now());
            SOCCER_STANDINGS
                .write()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(league.to_string(), tables);
        }
        Ok(_) => {}
        Err(e) => leptos::logging::log!("{league} standings fetch failed: {e}"),
    }
}

/// Per-game MLB series, fetched on demand (detail page) and cached with a short
/// TTL keyed by `match_id` (tz-agnostic raw series — formatted on read per
/// request), so repeated page views don't re-hit the MLB API and the same raw
/// series is reused across all timezones.
static MLB_SERIES: Lazy<RwLock<HashMap<i64, CachedRawSeries>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

struct CachedRawSeries {
    raw: crate::mlb::RawSeries,
    fetched_at: DateTime<Utc>,
}

/// Re-fetch a game's series at most this often. Short enough that a live series'
/// scores stay fresh, long enough that a burst of views is one fetch.
const SERIES_TTL_MIN: i64 = 2;

/// Request-time cache of a match's live-enriched streams, keyed by `(sport, id)`
/// — the same key `live_streams_for` looks the match up by — so a burst of views /
/// refetches is one Twitch call. Short TTL — live status is volatile.
static STREAM_STATUS: Lazy<RwLock<HashMap<(Sport, i64), CachedStreams>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

struct CachedStreams {
    streams: Vec<StreamView>,
    fetched_at: DateTime<Utc>,
}

const STREAM_STATUS_TTL_SECS: i64 = 90;
/// How close in time (± hours) a same-event match must be scheduled to count as
/// part of a match's "day slate" for widening co-stream discovery keywords.
const DISCOVERY_SLATE_WINDOW_HOURS: i64 = 12;

/// A match's streams with Twitch live status + seeded co-streamers merged in.
/// Returns the base streams unchanged when there's nothing to enrich (no Twitch
/// logins and no seeds) or when Twitch is unconfigured/erroring. The `SNAPSHOT`
/// read lock is dropped before any network await.
pub async fn live_streams_for(sport: Sport, id: i64) -> Vec<StreamView> {
    let (base, league, series_name, begin_at, status) = {
        let snap = SNAPSHOT.read().unwrap_or_else(PoisonError::into_inner);
        match snap.matches.iter().find(|m| m.id == id && m.sport == sport) {
            Some(m) => (
                m.streams.clone(),
                m.league.clone(),
                m.series_name.clone(),
                m.begin_at,
                m.status,
            ),
            None => return Vec::new(),
        }
    };
    // Demo/fixture mode: serve the mocked (already-ordered) streams as-is, without
    // touching Twitch/YouTube, so the styling is deterministic.
    if config().demo {
        return base;
    }
    let tw_seeds = costreamer_seeds(&config().costreamers, &league, sport);
    let base_logins: Vec<String> = base.iter().filter_map(|s| login_of(&s.url)).collect();

    // Curated YouTube co-streamer seeds. Base YouTube streams are resolved to
    // channels inside the enrichment phase, so they need no pre-extracted idents.
    let yt_seed_idents: Vec<String> =
        costreamer_seeds(&config().youtube_costreamers, &league, sport)
            .iter()
            .filter_map(|s| crate::youtube::channel_ident(s))
            .collect();
    let has_youtube_base = base.iter().any(|s| {
        let u = s.url.to_ascii_lowercase();
        u.contains("youtube.com") || u.contains("youtu.be")
    });
    let yt_enabled =
        config().youtube_api_key.is_some() && (!yt_seed_idents.is_empty() || has_youtube_base);

    // SOOP (Korean streaming) enrichment: opt-in, no key. Runs when there's a SOOP
    // stream to mark live or a curated SOOP co-streamer to surface.
    let soop_seeds = costreamer_seeds(&config().soop_costreamers, &league, sport);
    let has_soop_base = base.iter().any(|s| crate::soop::bid_of(&s.url).is_some());
    let soop_on = config().soop_enabled && (!soop_seeds.is_empty() || has_soop_base);

    // Twitch category discovery runs only for a live esports match with the sport
    // enabled — so an otherwise-inert match still triggers a scan when it's on.
    let discovery_on = matches!(status, MatchStatus::Live)
        && matches!(sport, Sport::Cs2 | Sport::Lol)
        && config().discovery_enabled(sport);
    if base_logins.is_empty() && tw_seeds.is_empty() && !yt_enabled && !discovery_on && !soop_on {
        return base;
    }
    {
        let g = STREAM_STATUS.read().unwrap_or_else(PoisonError::into_inner);
        if let Some(c) = g.get(&(sport, id)) {
            if c.fetched_at + Duration::seconds(STREAM_STATUS_TTL_SECS) > Utc::now() {
                return c.streams.clone();
            }
        }
    }

    // Twitch (from #3).
    let mut tw_logins = base_logins;
    tw_logins.extend(tw_seeds.iter().cloned());
    let tw_live = crate::twitch::live_streams(&tw_logins).await;
    let mut enriched = enrich_streams(base, &tw_live, &tw_seeds, game_needle(sport));

    // YouTube: resolve channels, collapse duplicates, mark live + append seeds.
    if yt_enabled {
        enriched = enrich_youtube_channels(enriched, &yt_seed_idents).await;
    }

    // SOOP: mark base SOOP streams live + append curated SOOP co-streamers.
    if soop_on {
        let mut bids: Vec<String> = enriched
            .iter()
            .filter_map(|s| crate::soop::bid_of(&s.url))
            .collect();
        bids.extend(soop_seeds.iter().cloned());
        let soop_live = crate::soop::live_streams(&bids).await;
        enriched = enrich_soop(enriched, &soop_live, &soop_seeds);
    }

    // Twitch category discovery (opt-in): surface unlisted co-streamers for a
    // live esports match, attributed by league keyword.
    if discovery_on {
        let cfg = config();
        if let Some(gid) = crate::twitch_discover::game_id(sport, &cfg.twitch_discovery.game_ids) {
            let scan =
                crate::twitch_discover::discover(&gid, &cfg.twitch_discovery.languages).await;
            if !scan.is_empty() {
                // Widen keywords to the whole day's event slate: co-streamers often
                // keep a stale title naming a different (or earlier) matchup.
                let keywords = {
                    let snap = SNAPSHOT.read().unwrap_or_else(PoisonError::into_inner);
                    event_day_keywords(
                        sport,
                        &league,
                        &series_name,
                        begin_at,
                        &snap.matches,
                        &cfg.twitch_league_aliases,
                        Duration::hours(DISCOVERY_SLATE_WINDOW_HOURS),
                    )
                };
                let present: std::collections::HashSet<String> =
                    enriched.iter().filter_map(|s| login_of(&s.url)).collect();
                let extra = attribute_costreams(
                    &scan,
                    &keywords,
                    &present,
                    cfg.twitch_discovery.min_viewers,
                    cfg.twitch_discovery.max_per_match,
                );
                enriched.extend(extra);
            }
        }
    }

    // Phase 2 (opt-in, off by default): official co-streamers via the unofficial
    // GQL API, keyed off the match's official Twitch broadcast channel(s).
    if config().twitch_discovery.gql_costreamers
        && matches!(status, MatchStatus::Live)
        && matches!(sport, Sport::Cs2 | Sport::Lol)
    {
        let officials: Vec<String> = enriched
            .iter()
            .filter(|s| s.official)
            .filter_map(|s| login_of(&s.url))
            .collect();
        let mut cand: Vec<String> = Vec::new();
        for ch in &officials {
            for cs in crate::twitch_gql::costreamers(ch).await {
                if !cand.contains(&cs) {
                    cand.push(cs);
                }
            }
        }
        if !cand.is_empty() {
            let live = crate::twitch::live_streams(&cand).await;
            let present: std::collections::HashSet<String> =
                enriched.iter().filter_map(|s| login_of(&s.url)).collect();
            for login in cand {
                if present.contains(&login) {
                    continue;
                }
                if let Some(info) = live.get(&login) {
                    enriched.push(StreamView {
                        url: format!("https://www.twitch.tv/{login}"),
                        language: info.language.clone(),
                        live: true,
                        viewers: Some(info.viewers),
                        official: false,
                        group: "costream".to_string(),
                        ..Default::default()
                    });
                }
            }
        }
    }

    // Final display order: official broadcasts lead, then co-streams grouped by
    // language (main-broadcast language first) with viewers descending.
    crate::types::order_streams(&mut enriched);

    STREAM_STATUS
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(
            (sport, id),
            CachedStreams {
                streams: enriched.clone(),
                fetched_at: Utc::now(),
            },
        );
    enriched
}

/// On-demand F1 results, keyed by (season, round). Fetched when a GP event page
/// is viewed; once a session is final its result is stable, so a longer TTL is
/// fine.
static F1_RESULTS: Lazy<RwLock<HashMap<(i64, i64), CachedF1Results>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

struct CachedF1Results {
    results: Vec<crate::types::F1Result>,
    fetched_at: DateTime<Utc>,
}

/// Re-fetch a Grand Prix's results at most this often.
const F1_RESULTS_TTL_MIN: i64 = 10;

/// On-demand WRC/MotoGP results (a rally's overall classification, a stage's
/// times, a MotoGP session order), keyed by the result ref. Fetched when a match
/// page is viewed and — unlike the poller's calendar/standings fetches — billed
/// outside the daily-cap accounting; a non-empty (finished, immutable) result is
/// therefore kept indefinitely so the season's handful of distinct classifications
/// each cost one request total. An empty result (not yet posted) re-checks once
/// the short TTL lapses.
static MOTOR_RESULTS: Lazy<RwLock<HashMap<String, CachedMotorResult>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

struct CachedMotorResult {
    rows: Vec<crate::types::MotorResultRow>,
    fetched_at: DateTime<Utc>,
    /// The last fetch failed (HTTP/parse error) rather than returning an empty
    /// list. Negative-cached briefly so a slow-failing endpoint isn't re-hit on
    /// every page view (Orange Cat Blacktop returns slow ~11 s 500s for some
    /// sessions — e.g. several at one WEC round), while still recovering quickly
    /// if the failure was transient.
    errored: bool,
}

/// Re-check an as-yet-empty motorsport result at most this often (a non-empty one
/// is immutable and kept until restart).
const MOTOR_RESULT_TTL_MIN: i64 = 15;

/// A finished game's box score is immutable, so cache it long.
const BOXSCORE_TTL_MIN: i64 = 360;

/// Re-check a *failed* box-score fetch at most this often.
const BOXSCORE_ERR_TTL_MIN: i64 = 5;

/// Re-check a *failed* fetch at most this often. Shorter than the empty TTL so a
/// transient error recovers fast, but long enough that a persistently-failing
/// (slow) endpoint doesn't block every page load — the page still shows every
/// other session that did succeed.
const MOTOR_RESULT_ERR_TTL_MIN: i64 = 5;

/// Whether a cached motorsport result is still fresh enough to serve without a
/// refetch: a non-empty (finished, immutable) result always is; an errored or an
/// empty one only until its TTL lapses (errors re-check sooner). Pure, so it's
/// unit-tested.
fn motor_cache_fresh(c: &CachedMotorResult, now: DateTime<Utc>) -> bool {
    if !c.rows.is_empty() {
        return true;
    }
    let ttl = if c.errored {
        MOTOR_RESULT_ERR_TTL_MIN
    } else {
        MOTOR_RESULT_TTL_MIN
    };
    now - c.fetched_at < Duration::minutes(ttl)
}

/// On-demand F1 championship standings, keyed by the (season, round) of the GP
/// whose page asked for them.
static F1_STANDINGS: Lazy<RwLock<HashMap<(i64, i64), CachedF1Standings>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));

struct CachedF1Standings {
    standings: crate::types::F1Standings,
    fetched_at: DateTime<Utc>,
}

/// Re-fetch standings at most this often.
const F1_STANDINGS_TTL_MIN: i64 = 10;

/// The MLB series between the two teams of `match_id`, fetched on demand and
/// cached with a short TTL. The raw (tz-agnostic) series is keyed by `match_id`
/// and formatted on read per request tz (`tz_name`/`hour24`), so the same cached
/// data is reused across timezones. `None` for esports, an unknown match, an MLB
/// match with no series ref, or when the fetch fails (the detail page then omits
/// the section). The snapshot read (for the ref + labels) is released before the
/// await so the future stays `Send`.
pub async fn mlb_series(
    match_id: i64,
    tz_name: &str,
    hour24: bool,
) -> Option<crate::types::Series> {
    let cfg = config();
    let tz = resolve_tz(tz_name, cfg.tz);
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
    // 1. Memory tier — keyed by match_id (tz-agnostic); format on read.
    {
        let cache = MLB_SERIES.read().unwrap_or_else(PoisonError::into_inner);
        if let Some(c) = cache.get(&match_id) {
            if Utc::now() - c.fetched_at < Duration::minutes(SERIES_TTL_MIN) {
                return Some(crate::mlb::format_series(&c.raw, fmt_labels));
            }
        }
    }
    // Pull the series ref + headline orientation from the snapshot, then drop the
    // lock before any await.
    let (sref, team_a, team_b, begin_at) = {
        let snap = SNAPSHOT.read().unwrap_or_else(PoisonError::into_inner);
        let m = snap
            .matches
            .iter()
            .find(|m| m.id == match_id && m.sport == Sport::Mlb)?;
        let sref = m.mlb_series?;
        (
            sref,
            m.team_a.label.clone(),
            m.team_b.label.clone(),
            m.begin_at,
        )
    };
    // 2. DB tier — persist non-empty raw series across restarts.
    if let Some((raw, fetched_at)) =
        db_cache_get::<crate::mlb::RawSeries>("mlb_series", &match_id.to_string())
    {
        if Utc::now() - fetched_at < Duration::minutes(SERIES_TTL_MIN) {
            MLB_SERIES
                .write()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(
                    match_id,
                    CachedRawSeries {
                        raw: raw.clone(),
                        fetched_at,
                    },
                );
            return Some(crate::mlb::format_series(&raw, fmt_labels));
        }
    }
    // 3. API fetch.
    let now = Utc::now();
    match crate::mlb::fetch_series(&HTTP, match_id, begin_at, &team_a, &team_b, sref).await {
        Ok(raw) if !raw.games.is_empty() => {
            MLB_SERIES
                .write()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(
                    match_id,
                    CachedRawSeries {
                        raw: raw.clone(),
                        fetched_at: now,
                    },
                );
            db_cache_put("mlb_series", &match_id.to_string(), &raw, now);
            Some(crate::mlb::format_series(&raw, fmt_labels))
        }
        Ok(_) => None,
        Err(e) => {
            leptos::logging::log!("MLB series fetch failed ({match_id}): {e}");
            None
        }
    }
}

/// A Grand Prix's finished-session results, fetched on demand and TTL-cached.
pub async fn f1_results(season: i64, round: i64) -> Vec<crate::types::F1Result> {
    let key = format!("{season}:{round}");
    {
        let cache = F1_RESULTS.read().unwrap_or_else(PoisonError::into_inner);
        if let Some(c) = cache.get(&(season, round)) {
            if Utc::now() - c.fetched_at < Duration::minutes(F1_RESULTS_TTL_MIN) {
                return c.results.clone();
            }
        }
    }
    if let Some((results, fetched_at)) =
        db_cache_get::<Vec<crate::types::F1Result>>("f1_result", &key)
    {
        if Utc::now() - fetched_at < Duration::minutes(F1_RESULTS_TTL_MIN) {
            F1_RESULTS
                .write()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(
                    (season, round),
                    CachedF1Results {
                        results: results.clone(),
                        fetched_at,
                    },
                );
            return results;
        }
    }
    let now = Utc::now();
    let results = crate::f1::fetch_results(&HTTP, season, round).await;
    F1_RESULTS
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(
            (season, round),
            CachedF1Results {
                results: results.clone(),
                fetched_at: now,
            },
        );
    if !results.is_empty() {
        db_cache_put("f1_result", &key, &results, now);
    }
    results
}

/// A motorsport classification (WRC stage/overall, MotoGP session) for the match
/// page, fetched on demand from Orange Cat Blacktop and cached. Empty when no
/// token is set or the source returns nothing.
pub async fn motor_result(r: &crate::types::MotorResultRef) -> Vec<crate::types::MotorResultRow> {
    let cache_key = format!("{}|{}|{}", r.series, r.event_id, r.session_id);
    // 1. in-memory hot tier (still negative-caches errors/empties).
    {
        let cache = MOTOR_RESULTS.read().unwrap_or_else(PoisonError::into_inner);
        if let Some(c) = cache.get(&cache_key) {
            if motor_cache_fresh(c, Utc::now()) {
                return c.rows.clone();
            }
        }
    }
    // 2. durable tier: the DB only holds non-empty results (errored: false).
    if let Some((rows, fetched_at)) =
        db_cache_get::<Vec<crate::types::MotorResultRow>>("motor_result", &cache_key)
    {
        let c = CachedMotorResult {
            rows,
            fetched_at,
            errored: false,
        };
        if motor_cache_fresh(&c, Utc::now()) {
            MOTOR_RESULTS
                .write()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(
                    cache_key.clone(),
                    CachedMotorResult {
                        rows: c.rows.clone(),
                        fetched_at,
                        errored: false,
                    },
                );
            return c.rows;
        }
    }
    // 3. API, then write through to memory (always) and the DB (durable only).
    let Some(token) = config().ocblacktop_token.as_deref() else {
        return Vec::new();
    };
    let now = Utc::now();
    let (rows, errored) = match crate::ocblacktop::fetch_motor_result(&HTTP, token, r).await {
        Some(rows) => (rows, false),
        None => (Vec::new(), true),
    };
    MOTOR_RESULTS
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(
            cache_key.clone(),
            CachedMotorResult {
                rows: rows.clone(),
                fetched_at: now,
                errored,
            },
        );
    if !errored && !rows.is_empty() {
        db_cache_put("motor_result", &cache_key, &rows, now);
    }
    rows
}

/// In-memory hot tier for box scores, keyed by `"{slug}:{id}"`. Stores both
/// successful and errored results so DEMO mode (where the DB cache is a no-op)
/// still has a cache, and errored results get a short negative-cache backoff.
static BOX_SCORES: Lazy<std::sync::Mutex<HashMap<String, CachedBoxScore>>> =
    Lazy::new(|| std::sync::Mutex::new(HashMap::new()));

struct CachedBoxScore {
    bs: crate::types::BoxScore,
    fetched_at: DateTime<Utc>,
    /// True when the last fetch returned an HTTP/parse error.
    errored: bool,
}

/// Whether a cached box score is fresh enough to serve: a finished real result
/// (`!unavailable && line.is_some()`) is kept until process restart; an errored
/// or still-unavailable one re-checks after its shorter TTL.
fn box_score_fresh(c: &CachedBoxScore, now: DateTime<Utc>) -> bool {
    if !c.bs.unavailable && c.bs.line.is_some() {
        return true;
    }
    let ttl = if c.errored {
        BOXSCORE_ERR_TTL_MIN
    } else {
        BOXSCORE_TTL_MIN
    };
    now - c.fetched_at < Duration::minutes(ttl)
}

/// Cap on the in-memory box-score hot tier. Each entry is a few KB; this bounds a
/// long-running process's growth (one entry per unique finished game ever viewed)
/// without churning hot games. Box scores are immutable once final, so an evicted
/// one just re-fetches on its next view.
const BOX_SCORE_CACHE_CAP: usize = 512;

/// Insert into `map`, first evicting the oldest entry (by fetch time) when the map
/// is at `cap` and the key is new — so it stays bounded. In-place updates of an
/// existing key never evict.
fn insert_capped(
    map: &mut HashMap<String, CachedBoxScore>,
    key: String,
    val: CachedBoxScore,
    cap: usize,
) {
    if map.len() >= cap && !map.contains_key(&key) {
        if let Some(oldest) = map
            .iter()
            .min_by_key(|(_, c)| c.fetched_at)
            .map(|(k, _)| k.clone())
        {
            map.remove(&oldest);
        }
    }
    map.insert(key, val);
}

/// Write a box score into the hot tier under the global cap.
fn cache_box_score(key: String, val: CachedBoxScore) {
    let mut map = BOX_SCORES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    insert_capped(&mut map, key, val, BOX_SCORE_CACHE_CAP);
}

/// A finished traditional team-sports game's box score. Fetched once from the
/// sport's upstream API, normalized to `BoxScore`, and persisted to
/// `result_cache` (namespace `"box_score"`, key `"{slug}:{id}"`). MLB is
/// supported; other sports fall through to an empty `BoxScore` until their
/// normalizers land.
pub async fn box_score(sport: crate::types::Sport, id: i64) -> crate::types::BoxScore {
    let key = format!("{}:{}", sport.slug(), id);
    // 1. In-memory hot tier (also negative-caches errors/unavailable).
    {
        let cache = BOX_SCORES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(c) = cache.get(&key) {
            if box_score_fresh(c, Utc::now()) {
                return c.bs.clone();
            }
        }
    }
    // 2. Durable tier: the DB only holds real results (not errors).
    if let Some((bs, at)) = db_cache_get::<crate::types::BoxScore>("box_score", &key) {
        if Utc::now() - at < Duration::minutes(BOXSCORE_TTL_MIN) {
            let c = CachedBoxScore {
                bs: bs.clone(),
                fetched_at: at,
                errored: false,
            };
            cache_box_score(key.clone(), c);
            return bs;
        }
    }
    // 3. Fetch from upstream, then write through to memory (always) and DB
    //    (only for real results).
    let now = Utc::now();
    let (bs, errored) = match sport {
        crate::types::Sport::Mlb => match crate::mlb::fetch_box_score(&HTTP, id).await {
            Ok((ls, raw)) => (crate::mlb::to_box_score(&ls, &raw), false),
            Err(_) => (
                crate::types::BoxScore {
                    unavailable: true,
                    ..Default::default()
                },
                true,
            ),
        },
        crate::types::Sport::Nhl => match crate::nhl::fetch_box_score(&HTTP, id).await {
            Ok((landing, rr, bs)) => (crate::nhl::to_box_score(&landing, &rr, &bs), false),
            Err(_) => (
                crate::types::BoxScore {
                    unavailable: true,
                    ..Default::default()
                },
                true,
            ),
        },
        crate::types::Sport::Nfl => {
            match crate::espn::fetch_box_score(&HTTP, &crate::espn::NFL, id).await {
                Ok(s) => (crate::espn::to_box_score(&s), false),
                Err(_) => (
                    crate::types::BoxScore {
                        unavailable: true,
                        ..Default::default()
                    },
                    true,
                ),
            }
        }
        crate::types::Sport::Nba => {
            match crate::espn::fetch_box_score(&HTTP, &crate::espn::NBA, id).await {
                Ok(s) => (crate::espn::to_box_score(&s), false),
                Err(_) => (
                    crate::types::BoxScore {
                        unavailable: true,
                        ..Default::default()
                    },
                    true,
                ),
            }
        }
        // Soccer is deferred: its ESPN summary needs the specific competition
        // slug (eng.1 / World Cup / …), which isn't derivable from (sport, id).
        crate::types::Sport::Soccer => (crate::types::BoxScore::default(), false),
        _ => (crate::types::BoxScore::default(), false),
    };
    cache_box_score(
        key.clone(),
        CachedBoxScore {
            bs: bs.clone(),
            fetched_at: now,
            errored,
        },
    );
    if !bs.unavailable && bs.line.is_some() {
        db_cache_put("box_score", &key, &bs, now);
    }
    bs
}

/// The ordered (section title, result ref) list a motorsport event page shows,
/// from that edition's rows. WRC → the overall classification then the Power
/// Stage, once the rally's last session has finished; WEC/MotoGP → each finished
/// classified session in start order. Pure (no I/O), so it's unit-tested;
/// [`motor_results`] fetches what it returns.
fn motor_result_plan(rows: &[NormalizedMatch]) -> Vec<(String, crate::types::MotorResultRef)> {
    let series = rows
        .iter()
        .map(|m| m.league.as_str())
        .find(|l| matches!(*l, "WRC" | "WEC" | "MotoGP"));
    match series {
        Some("WRC") => wrc_result_plan(rows),
        Some("WEC" | "MotoGP") => session_result_plan(rows),
        _ => Vec::new(),
    }
}

/// WRC: the overall classification (synthesized from the rally's event id, so it
/// survives the placeholder's removal during stage expansion) then the Power
/// Stage — shown only once the rally's latest-starting row has finished.
fn wrc_result_plan(rows: &[NormalizedMatch]) -> Vec<(String, crate::types::MotorResultRef)> {
    let wrc = || rows.iter().filter(|m| m.league == "WRC");
    let finished = wrc()
        .max_by_key(|m| m.begin_at)
        .is_some_and(|m| m.status == MatchStatus::Finished);
    if !finished {
        return Vec::new();
    }
    let mut plan = Vec::new();
    if let Some(event_id) =
        wrc().find_map(|m| m.motor_result_ref.as_ref().map(|r| r.event_id.clone()))
    {
        plan.push((
            "Overall classification".to_string(),
            crate::types::MotorResultRef {
                series: "WRC".to_string(),
                event_id,
                session_id: String::new(),
            },
        ));
    }
    if let Some(ps) = wrc().find(|m| m.team_a.label.contains("Power Stage")) {
        if let Some(r) = &ps.motor_result_ref {
            plan.push((ps.team_a.label.clone(), r.clone()));
        }
    }
    plan
}

/// WEC/MotoGP: each finished session that carries a results ref, in start order.
fn session_result_plan(rows: &[NormalizedMatch]) -> Vec<(String, crate::types::MotorResultRef)> {
    let mut sessions: Vec<&NormalizedMatch> = rows
        .iter()
        .filter(|m| {
            m.status == MatchStatus::Finished
                && m.motor_result_ref
                    .as_ref()
                    .is_some_and(|r| !r.session_id.is_empty())
        })
        .collect();
    sessions.sort_by_key(|m| m.begin_at);
    sessions
        .into_iter()
        .map(|m| (m.team_a.label.clone(), m.motor_result_ref.clone().unwrap()))
        .collect()
}

/// Every result section for a motorsport event page, by full edition name: the
/// classifications [`motor_result_plan`] selects, each fetched on demand (and
/// cached indefinitely once non-empty) and concurrently. Empty sections — and
/// F1 / non-motorsport editions — drop out. The snapshot lock is released before
/// the awaits so the future stays `Send`.
pub async fn motor_results(event: &str) -> Vec<crate::types::MotorResult> {
    let plan = {
        let snap = SNAPSHOT.read().unwrap_or_else(PoisonError::into_inner);
        let rows: Vec<NormalizedMatch> = snap
            .matches
            .iter()
            .filter(|m| m.sport == Sport::Motorsport)
            .filter(|m| event_name_eq(&m.league, &m.series_name, event))
            .cloned()
            .collect();
        motor_result_plan(&rows)
    };
    if plan.is_empty() {
        return Vec::new();
    }
    let fetched = futures::future::join_all(plan.iter().map(|(_, r)| motor_result(r))).await;
    plan.into_iter()
        .zip(fetched)
        .filter(|(_, rows)| !rows.is_empty())
        .map(|((title, _), rows)| crate::types::MotorResult { title, rows })
        .collect()
}

/// The F1 championship standings as of a Grand Prix's round, fetched on demand
/// and TTL-cached (keyed by the GP's round so each page caches its own).
pub async fn f1_standings(season: i64, round: i64) -> crate::types::F1Standings {
    let key = format!("{season}:{round}");
    {
        let cache = F1_STANDINGS.read().unwrap_or_else(PoisonError::into_inner);
        if let Some(c) = cache.get(&(season, round)) {
            if Utc::now() - c.fetched_at < Duration::minutes(F1_STANDINGS_TTL_MIN) {
                return c.standings.clone();
            }
        }
    }
    if let Some((standings, fetched_at)) =
        db_cache_get::<crate::types::F1Standings>("f1_standings", &key)
    {
        if Utc::now() - fetched_at < Duration::minutes(F1_STANDINGS_TTL_MIN) {
            F1_STANDINGS
                .write()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(
                    (season, round),
                    CachedF1Standings {
                        standings: standings.clone(),
                        fetched_at,
                    },
                );
            return standings;
        }
    }
    let now = Utc::now();
    let standings = crate::f1::fetch_standings(&HTTP, season, round).await;
    F1_STANDINGS
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(
            (season, round),
            CachedF1Standings {
                standings: standings.clone(),
                fetched_at: now,
            },
        );
    if !standings.drivers.is_empty() || !standings.constructors.is_empty() {
        db_cache_put("f1_standings", &key, &standings, now);
    }
    standings
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

/// Shared HTTP client for on-demand standings/bracket/result fetches. A request
/// timeout caps a hung/slow upstream so a single bad endpoint can't stall a page
/// indefinitely (Orange Cat Blacktop returns slow ~11 s 500s for some sessions);
/// set generously so it only catches true hangs, not slow-but-valid responses.
static HTTP: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .user_agent(crate::http::USER_AGENT)
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .unwrap_or_default()
});

/// Dedicated connection for the persistent result cache (`store::result_cache`),
/// used by both the on-demand caches and the poller's standings persistence.
/// `None` in DEMO mode → every `db_cache_*` helper no-ops and the caches stay
/// memory-only (today's behaviour). WAL + busy_timeout (set by `store::open`) let
/// it coexist with the poller's and push sender's connections.
static CACHE_DB: Lazy<Option<Mutex<rusqlite::Connection>>> = Lazy::new(|| {
    let cfg = config();
    if cfg.demo {
        return None;
    }
    crate::store::open(&cfg.db_path).ok().map(Mutex::new)
});

/// Read a typed value from `result_cache`. `None` when there's no DB, the row is
/// absent, or it doesn't decode.
fn db_cache_get<T: serde::de::DeserializeOwned>(ns: &str, key: &str) -> Option<(T, DateTime<Utc>)> {
    let conn = CACHE_DB
        .as_ref()?
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let entry = crate::store::cache_get(&conn, ns, key).ok().flatten()?;
    let value = serde_json::from_str::<T>(&entry.value).ok()?;
    let fetched_at = DateTime::from_timestamp_millis(entry.fetched_at_ms)?;
    Some((value, fetched_at))
}

/// Write a typed value through to `result_cache` (best-effort; logs on error,
/// never fails the caller).
fn db_cache_put<T: serde::Serialize>(ns: &str, key: &str, value: &T, fetched_at: DateTime<Utc>) {
    let Some(db) = CACHE_DB.as_ref() else { return };
    let Ok(json) = serde_json::to_string(value) else {
        return;
    };
    let conn = db.lock().unwrap_or_else(PoisonError::into_inner);
    if let Err(e) = crate::store::cache_put(&conn, ns, key, &json, fetched_at.timestamp_millis()) {
        leptos::logging::log!("result_cache put failed ({ns}/{key}): {e}");
    }
}

/// Every row of a namespace, decoded (for Pattern B startup load).
fn db_cache_get_ns<T: serde::de::DeserializeOwned>(ns: &str) -> Vec<(String, T, DateTime<Utc>)> {
    let Some(db) = CACHE_DB.as_ref() else {
        return Vec::new();
    };
    let conn = db.lock().unwrap_or_else(PoisonError::into_inner);
    crate::store::cache_get_ns(&conn, ns)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(k, e)| {
            let v = serde_json::from_str::<T>(&e.value).ok()?;
            let f = DateTime::from_timestamp_millis(e.fetched_at_ms)?;
            Some((k, v, f))
        })
        .collect()
}

/// Populate the poller-refreshed standings caches from the persisted result_cache
/// on boot, so a restart serves the last-known standings before the first poll.
fn load_persisted_standings() {
    let fill_vec = |ns: &str, store: &RwLock<Vec<EventInfo>>| {
        if let Some((tables, _)) = db_cache_get::<Vec<EventInfo>>(ns, "") {
            if !tables.is_empty() {
                *store.write().unwrap_or_else(PoisonError::into_inner) = tables;
            }
        }
    };
    fill_vec("mlb_standings", &MLB_STANDINGS);
    fill_vec("nhl_standings", &NHL_STANDINGS);
    fill_vec("nba_standings", &NBA_STANDINGS);
    fill_vec("nfl_standings", &NFL_STANDINGS);
    {
        let mut soccer = SOCCER_STANDINGS
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        for (league, tables, _) in db_cache_get_ns::<Vec<EventInfo>>("soccer_standings") {
            if !tables.is_empty() {
                soccer.insert(league, tables);
            }
        }
    }
    let fill_motor = |ns: &str, store: &RwLock<crate::types::MotorStandings>| {
        if let Some((s, _)) = db_cache_get::<crate::types::MotorStandings>(ns, "") {
            if !s.tables.is_empty() {
                *store.write().unwrap_or_else(PoisonError::into_inner) = s;
            }
        }
    };
    fill_motor("wrc_standings", &WRC_STANDINGS);
    fill_motor("wec_standings", &WEC_STANDINGS);
    fill_motor("motogp_standings", &MOTOGP_STANDINGS);
    {
        let mut brackets = PLAYOFF_BRACKETS
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        for (event, stage, _) in db_cache_get_ns::<EventInfo>("playoff_bracket") {
            if !stage.rounds.is_empty() {
                brackets.insert(event, stage);
            }
        }
    }
}

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
    // Load persisted standings so a restart serves the last-known tables
    // before the first poll completes (uses CACHE_DB, not the boot conn).
    load_persisted_standings();

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
            .user_agent(crate::http::USER_AGENT)
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
        // Orange Cat Blacktop (WRC/WEC/MotoGP): the last fetched calendars, merged
        // into the Motorsport feed every cycle but refreshed only on each series'
        // own proximity-driven gate, under a hard per-UTC-day request cap (free
        // tier is 250/day). These in-memory caches start empty on a restart; the
        // persisted snapshot rows drive the gates until they refill.
        let mut last_wrc: Vec<NormalizedMatch> = Vec::new();
        let mut last_wec: Vec<NormalizedMatch> = Vec::new();
        let mut last_motogp: Vec<NormalizedMatch> = Vec::new();
        // Resume the day's budget and the per-series/standings gates from the meta
        // table so a restart doesn't re-fire a full refresh or reset the cap.
        let mut ocb_day: Option<NaiveDate> = store
            .as_ref()
            .and_then(|c| crate::store::get_meta(c, OCB_META_DAY))
            .and_then(|s| s.parse::<NaiveDate>().ok());
        let mut ocb_used: u64 = store
            .as_ref()
            .and_then(|c| crate::store::get_meta(c, OCB_META_USED))
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        let mut ocb_wrc_at = ocb_load_at(store.as_ref(), OCB_META_WRC_AT);
        let mut ocb_wec_at = ocb_load_at(store.as_ref(), OCB_META_WEC_AT);
        let mut ocb_motogp_at = ocb_load_at(store.as_ref(), OCB_META_MOTOGP_AT);
        let mut ocb_standings_at = ocb_load_at(store.as_ref(), OCB_META_STANDINGS_AT);
        // Force one calendar refresh on the first poll after a restart, regardless
        // of the persisted per-series cadence: the in-memory result refs are gone
        // (and a just-migrated/restarted DB may carry none), so without this a
        // finished WRC stage's results stay blank until the next cadence-due fetch —
        // up to a full idle day off. The persisted daily cap still bounds it, so even
        // a crash loop can't exceed the budget.
        let mut ocb_initial = true;
        loop {
            let now = Utc::now();
            let active = {
                let snap = SNAPSHOT.read().unwrap_or_else(PoisonError::into_inner);
                is_active_window(&snap.matches, now, LIVE_GRACE, Duration::minutes(15))
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
            let mlb_window = (
                (now - Duration::days(8)).date_naive(),
                window_end.date_naive(),
            );
            // NHL, NBA and NFL share MLB's window; all keyless team-sport feeds.
            let nhl_window = mlb_window;
            let espn_window = mlb_window;
            // F1 comes from its own keyless API (Jolpica); the whole season's
            // sessions are fetched and the snapshot windowing decides what shows.
            use crate::espn::{european_season, season_year, EPL, NBA, NFL, WORLD_CUP};
            let (
                cs_res,
                lol_res,
                mlb_raw,
                mlb_standings_raw,
                nhl_raw,
                nhl_standings_raw,
                nba_raw,
                nba_standings_raw,
                nfl_raw,
                nfl_standings_raw,
                epl_raw,
                epl_standings_raw,
                wc_raw,
                wc_standings_raw,
                f1_raw,
            ) = tokio::join!(
                fetch_game(&client, &token, Sport::Cs2, window_end, deep),
                fetch_game(&client, &token, Sport::Lol, window_end, deep),
                crate::mlb::fetch_schedule(&client, mlb_window.0, mlb_window.1),
                crate::mlb::fetch_standings(&client, now.year()),
                crate::nhl::fetch_schedule(&client, nhl_window.0, nhl_window.1),
                crate::nhl::fetch_standings(&client),
                crate::espn::fetch_schedule(&client, &NBA, espn_window.0, espn_window.1),
                crate::espn::fetch_standings(&client, &NBA, Some(season_year(Sport::Nba, now))),
                crate::espn::fetch_schedule(&client, &NFL, espn_window.0, espn_window.1),
                crate::espn::fetch_standings(&client, &NFL, Some(season_year(Sport::Nfl, now))),
                crate::espn::fetch_schedule(&client, &EPL, espn_window.0, espn_window.1),
                crate::espn::fetch_standings(&client, &EPL, Some(european_season(now))),
                crate::espn::fetch_schedule(&client, &WORLD_CUP, espn_window.0, espn_window.1),
                crate::espn::fetch_standings(&client, &WORLD_CUP, None),
                crate::f1::fetch_schedule(&client, now.year()),
            );
            // Both soccer competitions are one Sport, so their schedules merge into
            // a single Soccer feed (best effort if only one side fetched).
            let soccer_res: FetchResult = match (epl_raw, wc_raw) {
                (Ok(mut a), Ok(b)) => {
                    a.extend(b);
                    Ok(a)
                }
                (Ok(a), Err(_)) | (Err(_), Ok(a)) => Ok(a),
                (Err(e), Err(_)) => Err(e.into()),
            };
            // Orange Cat Blacktop (WRC + WEC + MotoGP) — polled per series on a
            // proximity-driven cadence: fast only while a session of THAT series is
            // live or imminent, medium within ~2 weeks of an event, slow otherwise.
            // The day's budget and the per-series/standings gates are persisted
            // (meta table) so restarts resume the day rather than re-firing.
            // Cadence reads each series' rows from the snapshot (persisted), so it's
            // correct on a restart before the in-memory calendars refill. Results
            // merge into the Motorsport feed below; only the network fetch is
            // throttled. (Per-result classifications are fetched on demand by the
            // detail page and cached separately — see `motor_result`.)
            if let Some(key) = cfg.ocblacktop_token.as_deref() {
                if ocb_day != Some(now.date_naive()) {
                    ocb_day = Some(now.date_naive());
                    ocb_used = 0;
                    ocb_save_budget(store.as_ref(), now.date_naive(), ocb_used);
                }
                let cap = cfg.ocblacktop_daily_cap;
                let live = Duration::seconds(cfg.ocblacktop_live_poll.as_secs() as i64);
                let near = Duration::seconds(cfg.ocblacktop_near_poll.as_secs() as i64);
                let idle = Duration::seconds(cfg.ocblacktop_idle_poll.as_secs() as i64);
                // The three series' currently-known rows, from the snapshot. Cadence
                // is derived from these, so it survives a restart (the in-memory
                // last_* caches are empty until refetched).
                let ocb_ms: Vec<NormalizedMatch> = {
                    let snap = SNAPSHOT.read().unwrap_or_else(PoisonError::into_inner);
                    snap.matches
                        .iter()
                        .filter(|m| {
                            m.sport == Sport::Motorsport
                                && matches!(m.league.as_str(), "WRC" | "WEC" | "MotoGP")
                        })
                        .cloned()
                        .collect()
                };
                let rows_of = |lg: &str| {
                    ocb_ms
                        .iter()
                        .filter(|m| m.league == lg)
                        .cloned()
                        .collect::<Vec<_>>()
                };
                let wrc_iv = series_cadence(&rows_of("WRC"), now, live, near, idle);
                let wec_iv = series_cadence(&rows_of("WEC"), now, live, near, idle);
                let motogp_iv = series_cadence(&rows_of("MotoGP"), now, live, near, idle);

                // WRC: rallies calendar (1) + the active rally's stage timetable (1
                // more, near/live only) — reserve 2, bill the exact count it reports.
                if (ocb_initial || ocb_wrc_at.is_none_or(|t| now - t >= wrc_iv))
                    && ocb_used + 2 <= cap
                {
                    match crate::ocblacktop::fetch_wrc(&client, key, now.year(), now).await {
                        Ok((v, used)) => {
                            last_wrc = v;
                            ocb_used += used;
                        }
                        // A failure means the calendar request itself failed (detail
                        // is best-effort and never errors out) — bill the one.
                        Err(e) => {
                            ocb_used += 1;
                            leptos::logging::log!("WRC fetch failed: {e}");
                        }
                    }
                    ocb_wrc_at = Some(now);
                    ocb_save_at(store.as_ref(), OCB_META_WRC_AT, now);
                    ocb_save_budget(store.as_ref(), now.date_naive(), ocb_used);
                }
                // WEC: one events request.
                if (ocb_initial || ocb_wec_at.is_none_or(|t| now - t >= wec_iv)) && ocb_used < cap {
                    match crate::ocblacktop::fetch_wec(&client, key, now.year()).await {
                        Ok(v) => last_wec = v,
                        Err(e) => leptos::logging::log!("WEC fetch failed: {e}"),
                    }
                    ocb_used += 1;
                    ocb_wec_at = Some(now);
                    ocb_save_at(store.as_ref(), OCB_META_WEC_AT, now);
                    ocb_save_budget(store.as_ref(), now.date_naive(), ocb_used);
                }
                // MotoGP: one events request.
                if (ocb_initial || ocb_motogp_at.is_none_or(|t| now - t >= motogp_iv))
                    && ocb_used < cap
                {
                    match crate::ocblacktop::fetch_motogp(&client, key, now.year()).await {
                        Ok(v) => last_motogp = v,
                        Err(e) => leptos::logging::log!("MotoGP fetch failed: {e}"),
                    }
                    ocb_used += 1;
                    ocb_motogp_at = Some(now);
                    ocb_save_at(store.as_ref(), OCB_META_MOTOGP_AT, now);
                    ocb_save_budget(store.as_ref(), now.date_naive(), ocb_used);
                }
                // The one-shot initial refresh is spent; revert to the cadence gates.
                ocb_initial = false;
                // Standings: 6 requests (WRC drivers/co-drivers/manufacturers + WEC
                // + MotoGP riders/teams). They only change post-round, so refresh on
                // a daily floor plus an edge-trigger once any series finishes a round
                // (no oftener than the `near` cadence).
                let standings_floor =
                    Duration::seconds(cfg.ocblacktop_standings_poll.as_secs() as i64);
                if standings_due(&ocb_ms, now, ocb_standings_at, standings_floor, near)
                    && ocb_used + 6 <= cap
                {
                    let (wrc_s, wec_s, motogp_s) = tokio::join!(
                        crate::ocblacktop::fetch_wrc_standings(&client, key),
                        crate::ocblacktop::fetch_wec_standings(&client, key),
                        crate::ocblacktop::fetch_motogp_standings(&client, key),
                    );
                    ocb_used += 6;
                    ocb_standings_at = Some(now);
                    ocb_save_at(store.as_ref(), OCB_META_STANDINGS_AT, now);
                    ocb_save_budget(store.as_ref(), now.date_naive(), ocb_used);
                    if !wrc_s.tables.is_empty() {
                        db_cache_put("wrc_standings", "", &wrc_s, now);
                        *WRC_STANDINGS
                            .write()
                            .unwrap_or_else(PoisonError::into_inner) = wrc_s;
                    }
                    if !wec_s.tables.is_empty() {
                        db_cache_put("wec_standings", "", &wec_s, now);
                        *WEC_STANDINGS
                            .write()
                            .unwrap_or_else(PoisonError::into_inner) = wec_s;
                    }
                    if !motogp_s.tables.is_empty() {
                        db_cache_put("motogp_standings", "", &motogp_s, now);
                        *MOTOGP_STANDINGS
                            .write()
                            .unwrap_or_else(PoisonError::into_inner) = motogp_s;
                    }
                }
            }
            // Merge F1 (keyless, every cycle) with the cached WRC/WEC/MotoGP
            // calendars into one Motorsport feed — the same trick soccer uses for
            // its two competitions. The cached series stay visible even if this
            // cycle's F1 fetch failed.
            let motorsport_res: FetchResult = match f1_raw {
                Ok(mut v) => {
                    v.extend(last_wrc.iter().cloned());
                    v.extend(last_wec.iter().cloned());
                    v.extend(last_motogp.iter().cloned());
                    Ok(v)
                }
                Err(e) if last_wrc.is_empty() && last_wec.is_empty() && last_motogp.is_empty() => {
                    Err(e.into())
                }
                Err(_) => {
                    let mut v = last_wrc.clone();
                    v.extend(last_wec.iter().cloned());
                    v.extend(last_motogp.iter().cloned());
                    Ok(v)
                }
            };
            // The keyless feeds return a reqwest error; lift each to the shared
            // FetchResult (the first tuple pins the type, so `into` is inferred).
            apply_poll(
                vec![
                    (Sport::Cs2, cs_res),
                    (Sport::Lol, lol_res),
                    (Sport::Mlb, mlb_raw.map_err(Into::into)),
                    (Sport::Nhl, nhl_raw.map_err(Into::into)),
                    (Sport::Nba, nba_raw.map_err(Into::into)),
                    (Sport::Nfl, nfl_raw.map_err(Into::into)),
                    (Sport::Soccer, soccer_res),
                    (Sport::Motorsport, motorsport_res),
                ],
                store.as_mut(),
                cfg.archive_cutoff(now).timestamp_millis(),
            );
            // Refresh each standings cache. Keep the old tables on a fetch error
            // or an empty off-season response, rather than blanking the page.
            update_standings(&MLB_STANDINGS, mlb_standings_raw, "MLB", "mlb_standings");
            update_standings(&NHL_STANDINGS, nhl_standings_raw, "NHL", "nhl_standings");
            update_standings(&NBA_STANDINGS, nba_standings_raw, "NBA", "nba_standings");
            update_standings(&NFL_STANDINGS, nfl_standings_raw, "NFL", "nfl_standings");
            // Soccer is keyed by competition: the PL's single table, the World
            // Cup's group tables.
            update_soccer_standings("Premier League", epl_standings_raw);
            update_soccer_standings("World Cup", wc_standings_raw);

            // Postseason brackets (soccer World Cup + NFL/NBA/NHL/MLB playoffs) —
            // same cadence as standings; each builder is a no-op off-season.
            refresh_brackets(&client, now).await;

            // Resolve exact Liquipedia links for any new events. The async
            // lookups hold no DB connection; persistence happens synchronously
            // after, so the future stays Send.
            if cfg.resolve_links {
                let candidates = collect_resolution_candidates();
                if !candidates.is_empty() {
                    let mut results: Vec<(String, Option<String>)> = Vec::new();
                    for c in candidates.into_iter().take(MAX_RESOLVE_PER_CYCLE) {
                        match crate::liquipedia::resolve_event(&client, c.sport, &c.league, c.year)
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
            // Likewise keep the current F1 championship standings hot: the home
            // (and the current-GP page) request `f1_standings(season, 0)` on
            // demand from Jolpica (~0.5s cold), so without this the first viewer
            // after each 10-min TTL eats that fetch. `f1_standings` self-gates on
            // its own TTL and Jolpica is keyless (separate from the PandaScore
            // budget), so this adds at most one keyless request per cycle and
            // isn't headroom-gated.
            let _ = f1_standings(i64::from(now.year()), 0).await;

            // Low-priority past-day work: only when nothing is live/imminent and
            // the API budget has headroom, so current/future polling comes first.
            if !active && headroom_ok(rate_remaining, cfg.rate_limit_floor) {
                let cutoff_ms = cfg.archive_cutoff(now).timestamp_millis();

                // 1. Refresh one overdue recent-past day (≤1 request).
                if cfg.past_refresh {
                    let bucket = store.as_ref().and_then(|c| next_due_refresh_bucket(c, now));
                    if let Some((sport, day)) = bucket {
                        let (from, to) = utc_day_bounds(day);
                        match crate::pandascore::fetch_past_range_all(
                            &client, &token, sport, from, to,
                        )
                        .await
                        {
                            Ok((matches, rate)) => {
                                rate_remaining = rate.or(rate_remaining);
                                apply_past(matches, store.as_mut(), cutoff_ms);
                                if let Some(conn) = store.as_ref() {
                                    let _ = crate::store::set_meta(
                                        conn,
                                        &refresh_key(sport, day),
                                        &now.timestamp_millis().to_string(),
                                    );
                                }
                            }
                            Err(e) => {
                                leptos::logging::log!(
                                    "past-refresh failed ({} {day}): {e}",
                                    sport.slug()
                                );
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
                                crate::pandascore::fetch_past_range_all(
                                    &client,
                                    &token,
                                    Sport::Cs2,
                                    from,
                                    to
                                ),
                                crate::pandascore::fetch_past_range_all(
                                    &client,
                                    &token,
                                    Sport::Lol,
                                    from,
                                    to
                                ),
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
                            leptos::logging::log!(
                                "backfilled {} … {}",
                                from.date_naive(),
                                to.date_naive()
                            );
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

            let delay = if active {
                cfg.active_poll
            } else {
                cfg.idle_poll
            };
            leptos::logging::log!(
                "next poll in {}s (active={active}, deep={deep})",
                delay.as_secs()
            );
            tokio::time::sleep(delay).await;
        }
    });
}

/// How long after its start a match the API never marks "finished" is still
/// assumed to be live. The free tier has no realtime feed, so a started match
/// with no terminal status is shown as live up to this long — past it, we assume
/// it's over rather than leaving a permanent fake "LIVE". Bounds both the poll
/// cadence ([`is_active_window`]) and the display heuristic ([`effective_status`])
/// so the two agree. Sized to cover a long Bo5.
const LIVE_GRACE: Duration = Duration::hours(5);

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

/// Switch an Orange Cat Blacktop series to its fast (`live`) tier this far ahead
/// of a session's start, so a stage/race is already being polled when it begins.
const OCB_LIVE_LEAD: Duration = Duration::minutes(60);
/// How far ahead an event counts as "near" (the medium tier) — roughly when WRC
/// publishes a rally's stage timetable, so we ramp up in time to catch it.
const OCB_NEAR_WITHIN: Duration = Duration::days(14);
/// A finished session stays "recent" — worth a standings refresh — for this long
/// after its start, covering a race weekend's worth of just-completed sessions.
const OCB_STANDINGS_SETTLE: Duration = Duration::hours(48);

/// Per-series poll cadence derived from a series' currently-known matches: the
/// fast `live` tier when a session is running or starts within the hour, the
/// medium `near` tier when its next event is within ~2 weeks, else the slow
/// `idle` tier (off-season / between rounds). Pure, so it's unit-tested; the
/// poller feeds it the persisted snapshot rows for one series, so it reads right
/// across a restart (the in-memory calendar caches are empty until a refetch).
fn series_cadence(
    matches: &[NormalizedMatch],
    now: DateTime<Utc>,
    live: Duration,
    near: Duration,
    idle: Duration,
) -> Duration {
    let mut soonest: Option<Duration> = None;
    for m in matches {
        match m.status {
            MatchStatus::Live => return live,
            MatchStatus::Upcoming => {
                let until = m.begin_at - now;
                if until >= Duration::zero() && soonest.is_none_or(|s| until < s) {
                    soonest = Some(until);
                }
            }
            MatchStatus::Finished | MatchStatus::Canceled => {}
        }
    }
    match soonest {
        Some(until) if until <= OCB_LIVE_LEAD => live,
        Some(until) if until <= OCB_NEAR_WITHIN => near,
        _ => idle,
    }
}

/// Whether the championship standings are due a refresh. They only move after a
/// round, so this is edge-triggered: refresh once a session has finished within
/// the settle window (no more often than `post_round`), plus a `daily_floor`
/// backstop that also covers the off-season and the first-ever run. Pure, for
/// unit testing.
fn standings_due(
    matches: &[NormalizedMatch],
    now: DateTime<Utc>,
    last_fetch: Option<DateTime<Utc>>,
    daily_floor: Duration,
    post_round: Duration,
) -> bool {
    let since = last_fetch.map(|t| now - t);
    // Daily floor (and the first-ever run, when `since` is None).
    if since.is_none_or(|d| d >= daily_floor) {
        return true;
    }
    // A round just finished: refresh, but no more often than `post_round`.
    let recent_finish = matches.iter().any(|m| {
        matches!(m.status, MatchStatus::Finished) && {
            let age = now - m.begin_at;
            age >= Duration::zero() && age <= OCB_STANDINGS_SETTLE
        }
    });
    recent_finish && since.is_none_or(|d| d >= post_round)
}

// Meta keys for the persisted Orange Cat Blacktop poll state. Persisting the
// day's request budget and the per-series/standings gates means a restart
// resumes the day instead of re-firing a full refresh — across many restarts
// (e.g. iterating in dev) that re-firing used to burn through the daily quota.
const OCB_META_DAY: &str = "ocb_budget_day";
const OCB_META_USED: &str = "ocb_budget_used";
const OCB_META_WRC_AT: &str = "ocb_wrc_at";
const OCB_META_WEC_AT: &str = "ocb_wec_at";
const OCB_META_MOTOGP_AT: &str = "ocb_motogp_at";
const OCB_META_STANDINGS_AT: &str = "ocb_standings_at";

/// Load a persisted millisecond timestamp from the meta table.
fn ocb_load_at(store: Option<&rusqlite::Connection>, key: &str) -> Option<DateTime<Utc>> {
    store
        .and_then(|c| crate::store::get_meta(c, key))
        .and_then(|s| s.parse::<i64>().ok())
        .and_then(DateTime::from_timestamp_millis)
}

/// Persist a millisecond timestamp to the meta table (best effort).
fn ocb_save_at(store: Option<&rusqlite::Connection>, key: &str, at: DateTime<Utc>) {
    if let Some(c) = store {
        let _ = crate::store::set_meta(c, key, &at.timestamp_millis().to_string());
    }
}

/// Persist the day's request budget (UTC date + count) so a restart can't reset
/// it and let repeated restarts exceed the daily cap.
fn ocb_save_budget(store: Option<&rusqlite::Connection>, day: NaiveDate, used: u64) {
    if let Some(c) = store {
        let _ = crate::store::set_meta(c, OCB_META_DAY, &day.to_string());
        let _ = crate::store::set_meta(c, OCB_META_USED, &used.to_string());
    }
}

/// Build the WRC keep-set for the league-scoped DB replace, and prune superseded
/// placeholders from `fresh` in place.
///
/// WRC's feed is fully enumerated each poll, but only the *active* rally is
/// expanded into per-stage rows; every other rally is a date-only placeholder. A
/// naive "keep just this poll's WRC ids" drops a *finished* rally's stages (no
/// longer re-fetched) and lets its placeholder reappear beside any it still has.
/// So: gather the rallies that have stage rows (this poll's `fresh` plus the
/// previously-cached `prev_wrc_stages`), drop from `fresh` any placeholder for
/// such a rally (the detailed rows stand in for the generic line), and keep both
/// this poll's WRC rows and the carried-forward stages. A stage carries a
/// non-empty `session_id`; a placeholder an empty one (both now DB-persisted).
fn reconcile_wrc(
    fresh: &mut Vec<NormalizedMatch>,
    prev_wrc_stages: &[NormalizedMatch],
) -> Vec<i64> {
    let expanded: std::collections::HashSet<String> = fresh
        .iter()
        .chain(prev_wrc_stages.iter())
        .filter_map(|m| m.motor_result_ref.as_ref())
        .filter(|r| !r.session_id.is_empty())
        .map(|r| r.event_id.clone())
        .collect();
    fresh.retain(|m| {
        m.league != "WRC"
            || m.motor_result_ref
                .as_ref()
                .is_none_or(|r| !r.session_id.is_empty() || !expanded.contains(&r.event_id))
    });
    let mut keep: Vec<i64> = fresh
        .iter()
        .filter(|m| m.league == "WRC")
        .map(|m| m.id)
        .collect();
    keep.extend(prev_wrc_stages.iter().map(|m| m.id));
    keep
}

/// Persist freshly polled matches (if a store is present) and refresh the
/// in-memory snapshot. Synchronous — no `.await` while touching the DB.
fn apply_poll(
    results: Vec<(Sport, FetchResult)>,
    store: Option<&mut rusqlite::Connection>,
    cutoff_ms: i64,
) {
    let now = Utc::now();
    let mut fresh: Vec<NormalizedMatch> = Vec::new();
    let mut errored: Vec<Sport> = Vec::new();
    let mut any_ok = false;

    for (sport, res) in results {
        match res {
            Ok(mut v) => {
                any_ok = true;
                fresh.append(&mut v);
            }
            Err(e) => {
                errored.push(sport);
                leptos::logging::log!("poll failed for {}: {e}", sport.slug());
            }
        }
    }
    let any_err = !errored.is_empty();

    if let Some(conn) = store {
        // Upsert successes; matches for an errored sport stay in the DB (we only
        // prune by age), so the reload below preserves them. WRC is the exception:
        // its season feed is complete every poll, so it's league-scoped replaced —
        // a rally placeholder superseded by its stages is dropped rather than
        // retained. `reconcile_wrc` builds the keep-set so a *finished* rally's
        // already-fetched stages (this poll only re-fetches the active rally) are
        // carried forward instead of pruned. Empty keep = no WRC this poll (skipped
        // in the store), so a failed fetch never clears the league.
        if any_ok {
            let prev_wrc_stages: Vec<NormalizedMatch> = {
                let snap = SNAPSHOT.read().unwrap_or_else(PoisonError::into_inner);
                snap.matches
                    .iter()
                    .filter(|m| {
                        m.league == "WRC"
                            && m.motor_result_ref
                                .as_ref()
                                .is_some_and(|r| !r.session_id.is_empty())
                    })
                    .cloned()
                    .collect()
            };
            let wrc_keep = reconcile_wrc(&mut fresh, &prev_wrc_stages);
            if let Err(e) = crate::store::upsert_and_prune(
                conn,
                &fresh,
                now.timestamp_millis(),
                cutoff_ms,
                &[("WRC", wrc_keep)],
            ) {
                leptos::logging::log!("cache db write failed: {e}");
            }
        }
        match crate::store::load_all(conn) {
            Ok(mut all) => {
                all.sort_by_key(|m| m.begin_at);
                // Streams/broadcasts and the MLB series ref aren't persisted to the
                // DB, so the reload drops them; re-attach them from this poll's fresh
                // fetch (keyed by match id) so the detail page still has them. (The
                // motorsport result ref IS persisted now — see store.rs — so it
                // survives the reload directly, which is why a finished WRC stage
                // still renders its results after a restart.)
                type LiveExtras = (Vec<StreamView>, Option<crate::types::MlbSeriesRef>);
                let mut live: HashMap<i64, LiveExtras> = fresh
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
/// the previous snapshot's matches for any sport that failed this round.
/// Merge freshly fetched matches with the previous set, keeping the previous
/// matches for any sport that failed this round. Sorted by start time.
fn merge_matches(
    prev: &[NormalizedMatch],
    mut fresh: Vec<NormalizedMatch>,
    errored: &[Sport],
    cutoff_ms: i64,
) -> Vec<NormalizedMatch> {
    for &sport in errored {
        fresh.extend(prev.iter().filter(|m| m.sport == sport).cloned());
    }
    // Mirror the DB prune so memory-only mode doesn't accumulate stale matches.
    fresh.retain(|m| m.begin_at.timestamp_millis() >= cutoff_ms);
    fresh.sort_by_key(|m| m.begin_at);
    fresh
}

fn merge_in_memory(
    matches: Vec<NormalizedMatch>,
    errored: &[Sport],
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
fn apply_past(
    matches: Vec<NormalizedMatch>,
    store: Option<&mut rusqlite::Connection>,
    cutoff_ms: i64,
) {
    if matches.is_empty() {
        return;
    }
    // Persist (best effort), then merge the small batch into the in-memory
    // snapshot in place — no full table reload (the snapshot is already in sync
    // from the main poll's load_all this same cycle).
    if let Some(conn) = store {
        let now = Utc::now();
        if let Err(e) =
            crate::store::upsert_and_prune(conn, &matches, now.timestamp_millis(), cutoff_ms, &[])
        {
            leptos::logging::log!("past write failed: {e}");
            return;
        }
    }
    let replaced: std::collections::HashSet<(i64, Sport)> =
        matches.iter().map(|m| (m.id, m.sport)).collect();
    let mut snap = SNAPSHOT.write().unwrap_or_else(PoisonError::into_inner);
    snap.matches.retain(|m| {
        !replaced.contains(&(m.id, m.sport)) && m.begin_at.timestamp_millis() >= cutoff_ms
    });
    snap.matches.extend(
        matches
            .into_iter()
            .filter(|m| m.begin_at.timestamp_millis() >= cutoff_ms),
    );
    snap.matches.sort_by_key(|m| m.begin_at);
}

/// How often to re-fetch a past day's scores, by age in whole days vs today
/// (UTC). Today (0) gets a short interval: the main poll can miss a match that
/// finished and dropped out of /upcoming (the free tier's /running and top-of
/// /past feeds don't reliably surface it), so the date-range re-fetch closes the
/// gap the same day instead of waiting for the next. `None` = never: matches over
/// a week old are locked (scores no longer change, frees budget for backfill).
fn refresh_interval(age_days: i64) -> Option<Duration> {
    match age_days {
        0 => Some(Duration::minutes(30)),
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

fn refresh_key(sport: Sport, day: NaiveDate) -> String {
    format!("past_refresh:{}:{}", sport.slug(), day.format("%Y-%m-%d"))
}

/// The single most-overdue past `(sport, day)` bucket due for a score refresh,
/// or `None`. Only buckets that actually have matches in the snapshot and that
/// [`refresh_interval`] still tracks (today through a week old) are considered.
fn next_due_refresh_bucket(
    conn: &rusqlite::Connection,
    now: DateTime<Utc>,
) -> Option<(Sport, NaiveDate)> {
    let today = now.date_naive();
    let buckets: Vec<(Sport, NaiveDate)> = {
        let snap = SNAPSHOT.read().unwrap_or_else(PoisonError::into_inner);
        let mut set = std::collections::HashSet::new();
        for m in &snap.matches {
            // Past-refresh re-fetches a day from PandaScore; traditional sports
            // (MLB) come from their own feed and have no PandaScore endpoint.
            if m.sport.traditional() {
                continue;
            }
            let day = m.begin_at.date_naive();
            if refresh_interval((today - day).num_days()).is_some() {
                set.insert((m.sport, day));
            }
        }
        set.into_iter().collect()
    };
    let mut best: Option<(Sport, NaiveDate, i64)> = None;
    for (sport, day) in buckets {
        let Some(interval) = refresh_interval((today - day).num_days()) else {
            continue;
        };
        let overdue = match crate::store::get_meta(conn, &refresh_key(sport, day))
            .and_then(|s| s.parse::<i64>().ok())
        {
            None => i64::MAX, // never refreshed → top priority
            Some(t) => (now.timestamp_millis() - t) - interval.num_milliseconds(),
        };
        if overdue >= 0 && best.as_ref().is_none_or(|b| overdue > b.2) {
            best = Some((sport, day, overdue));
        }
    }
    best.map(|(g, d, _)| (g, d))
}

struct Candidate {
    key: String,
    sport: Sport,
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
        if m.sport.traditional() {
            continue;
        }
        let year = m.begin_at.year();
        let key = event_key(m.sport, &m.league, year);
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
                sport: m.sport,
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
    tz.from_local_datetime(&naive).earliest().map_or_else(
        || Utc.from_utc_datetime(&naive),
        |dt| dt.with_timezone(&Utc),
    )
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
    let min = local.minute();
    // Left-pad single-hour times with a figure space (U+2007 — a digit-width space
    // that HTML won't collapse) so e.g. " 4:10 PM" lines up under "12:00 PM".
    if h12 < 10 {
        format!("\u{2007}{h12}:{min:02} {ampm}")
    } else {
        format!("{h12}:{min:02} {ampm}")
    }
}

// Weekday/month names looked up directly rather than via chrono's `format()`,
// which allocates several temporaries per call. `day_label` is the hottest
// allocator on a schedule render — `to_view` calls it once per row for
// `date_label` — so this is pure allocation savings; the output is byte-identical
// to the old `%A`/`%a`/`%B`/`%b` formatting.
const WEEKDAYS_FULL: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];
const WEEKDAYS_SHORT: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const MONTHS_FULL: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const MONTHS_SHORT: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

fn day_label(local: DateTime<Tz>) -> String {
    let wd = WEEKDAYS_FULL[local.weekday().num_days_from_monday() as usize];
    let mo = MONTHS_FULL[local.month0() as usize];
    format!("{wd}, {mo} {}", local.day())
}

/// Compact day label, e.g. "Mon, Jun 23" — used by the series rows, where the
/// date sits inline with the time rather than as a day heading.
fn short_day_label(local: DateTime<Tz>) -> String {
    let wd = WEEKDAYS_SHORT[local.weekday().num_days_from_monday() as usize];
    let mo = MONTHS_SHORT[local.month0() as usize];
    format!("{wd}, {mo} {}", local.day())
}

/// Resolve the effective view status, applying the "started but not marked
/// finished" live heuristic (the free tier has no real-time feed).
fn effective_status(m: &NormalizedMatch, now: DateTime<Utc>) -> MatchStatus {
    match m.status {
        // Started but never marked finished. The free tier has no realtime feed,
        // so treat it as live only within the grace window; past that, assume it
        // ended rather than leaving a permanent fake "LIVE" — the real result is
        // backfilled by the past-day refresh.
        MatchStatus::Upcoming if m.begin_at <= now => {
            if now < m.begin_at + LIVE_GRACE {
                MatchStatus::Live
            } else {
                MatchStatus::Finished
            }
        }
        other => other,
    }
}

#[doc(hidden)]
pub fn to_view(m: &NormalizedMatch, tz: &Tz, now: DateTime<Utc>, hour24: bool) -> MatchView {
    let local = m.begin_at.with_timezone(tz);
    let status = effective_status(m, now);

    // Motorsport rows that carry only a calendar date (no session time), anchored
    // at noon UTC so the viewer's day comes out right: a WRC rally or WEC round
    // whose timetable isn't published yet — a placeholder with no venue tz (every
    // real WRC stage / WEC session carries one). There's no real clock to show, so
    // the time cell stays blank and there's no venue time to toggle to; the day
    // heading still carries the date.
    let date_only = m.sport == Sport::Motorsport
        && (m.league == "WRC" || m.league == "WEC")
        && m.venue_tz.is_none();

    // PandaScore returns placeholder 0-0 results on unplayed matches, and a match
    // we only *assume* is over (started, never marked finished) may still hold
    // that placeholder — so for esports map counts and assumed-over rows, trust a
    // score only once it's nonzero (a real Bo-X final always has a winner > 0, and
    // a live map count is real only past 0-0). A traditional sport the *source*
    // itself marked finished can legitimately end 0-0 (a soccer draw), so trust
    // that as the real result rather than blanking it.
    let nonzero = matches!((m.team_a.score, m.team_b.score), (Some(a), Some(b)) if a > 0 || b > 0);
    let real_traditional_final = m.status == MatchStatus::Finished && m.sport.traditional();
    let trust_scores = matches!(status, MatchStatus::Finished | MatchStatus::Live)
        && (nonzero || real_traditional_final);
    let mut team_a = TeamView {
        label: m.team_a.label.clone(),
        name: m.team_a.name.clone(),
        score: trust_scores.then_some(m.team_a.score).flatten(),
        winner: false,
        logo: m.team_a_logo.clone(),
        abbrev: m.team_a.abbrev.clone(),
    };
    let mut team_b = TeamView {
        label: m.team_b.label.clone(),
        name: m.team_b.name.clone(),
        score: trust_scores.then_some(m.team_b.score).flatten(),
        winner: false,
        logo: m.team_b_logo.clone(),
        abbrev: m.team_b.abbrev.clone(),
    };
    // Only a finished match has a winner; a leader mid-match isn't one yet.
    if status == MatchStatus::Finished {
        if let (Some(a), Some(b)) = (team_a.score, team_b.score) {
            team_a.winner = a > b;
            team_b.winner = b > a;
        }
    }

    // When the venue's timezone is known (MLB/F1), the same instant in that zone,
    // labelled with its date and abbreviation (e.g. "Wed, Jun 24 · 6:00 PM EDT") —
    // the local date+time at the venue. The date is included because the venue can
    // sit on a different calendar day than the viewer, which showing only the time
    // would silently hide. Empty otherwise (esports has no venue tz).
    let venue_label = if date_only {
        String::new()
    } else {
        m.venue_tz
            .as_deref()
            .and_then(|id| id.parse::<Tz>().ok())
            .map(|vtz| {
                let vlocal = m.begin_at.with_timezone(&vtz);
                format!(
                    "{} · {} {}",
                    vlocal.format("%a, %b %-d"),
                    time_label(vlocal, hour24),
                    vlocal.format("%Z")
                )
            })
            .unwrap_or_default()
    };

    MatchView {
        id: m.id,
        sport: m.sport,
        league: m.league.clone(),
        series_name: m.series_name.clone(),
        status,
        clock_label: if date_only {
            String::new()
        } else {
            time_label(local, hour24)
        },
        date_label: day_label(local),
        venue_label,
        venue_name: m.venue_name.clone(),
        venue_location: m.venue_location.clone(),
        best_of: m.best_of.map(|n| format!("Bo{n}")).unwrap_or_default(),
        team_a,
        team_b,
        league_url: m.league_url.clone().unwrap_or_default(),
        begin_at_ms: m.begin_at.timestamp_millis(),
        row_href: None,
    }
}

/// The event link shown in the UI: a resolved exact Liquipedia page when one is
/// cached, else the generic fallback (official site or Liquipedia search).
fn resolved_event_url(
    sport: Sport,
    league: &str,
    begin_at: DateTime<Utc>,
    official: Option<&str>,
) -> String {
    let key = event_key(sport, league, begin_at.year());
    if let Some(EventLink { url: Some(u), .. }) = EVENT_LINKS
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
    {
        return u.clone();
    }
    event_link(sport, league, official)
}

/// Generic fallback link: the official site when the API provides one, else —
/// for esports — a sport-specific Liquipedia search, or for a traditional sport
/// its league's official site ([`traditional_event_url`]). Empty if none fits.
fn event_link(sport: Sport, league: &str, official: Option<&str>) -> String {
    if let Some(url) = official.filter(|u| !u.trim().is_empty()) {
        return url.to_string();
    }
    match sport {
        // Esports: a Liquipedia search for the event.
        Sport::Cs2 | Sport::Lol => {
            let wiki = if sport == Sport::Lol {
                "leagueoflegends"
            } else {
                "counterstrike"
            };
            format!(
                "https://liquipedia.net/{wiki}/index.php?search={}",
                pct_encode(league)
            )
        }
        // Traditional sports: the official league site — Liquipedia is esports-only.
        _ => traditional_event_url(sport, league),
    }
}

/// The official site for a traditional sport's competition (empty if we have no
/// good page for it). Shown as the event page's external link instead of a
/// Liquipedia search.
fn traditional_event_url(sport: Sport, league: &str) -> String {
    match sport {
        Sport::Mlb => "https://www.mlb.com/scores",
        Sport::Nhl => "https://www.nhl.com/scores",
        Sport::Nba => "https://www.nba.com/games",
        Sport::Nfl => "https://www.nfl.com/scores",
        // The series' official site, by league (F1 / WRC / WEC / MotoGP).
        Sport::Motorsport => match league {
            "WRC" => "https://www.wrc.com/",
            "WEC" => "https://www.fiawec.com/",
            "MotoGP" => "https://www.motogp.com/",
            _ => "https://www.formula1.com/en/racing.html",
        },
        Sport::Soccer => match league {
            "Premier League" => "https://www.premierleague.com/fixtures",
            "World Cup" => "https://www.fifa.com/en/tournaments/mens/worldcup/canadamexicousa2026",
            _ => "",
        },
        Sport::Cs2 | Sport::Lol => "",
    }
    .to_string()
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

/// ESPN gamecast URL slug for the leagues we pull from ESPN. `None` for sports
/// we don't fetch from ESPN (so they fall through to the event-page fallback).
fn espn_slug(sport: Sport) -> Option<&'static str> {
    match sport {
        Sport::Nhl => Some("nhl"),
        Sport::Nba => Some("nba"),
        Sport::Nfl => Some("nfl"),
        _ => None,
    }
}

/// Human host label for a fallback event/series URL — the anchor text's source
/// name. `None` for an empty or unrecognized host, so the page shows no link
/// rather than an anchor with no sensible label.
fn source_label(url: &str) -> Option<&'static str> {
    if url.is_empty() {
        return None;
    }
    Some(if url.contains("liquipedia.net") {
        "Liquipedia"
    } else if url.contains("formula1.com") {
        "Formula 1"
    } else if url.contains("wrc.com") {
        "WRC"
    } else if url.contains("fiawec.com") {
        "WEC"
    } else if url.contains("motogp.com") {
        "MotoGP"
    } else if url.contains("mlb.com") {
        "MLB"
    } else if url.contains("nhl.com") {
        "NHL"
    } else if url.contains("nba.com") {
        "NBA"
    } else if url.contains("nfl.com") {
        "NFL"
    } else if url.contains("premierleague.com") {
        "Premier League"
    } else if url.contains("fifa.com") {
        "FIFA"
    } else if url.contains("wikipedia.org") {
        "Wikipedia"
    } else {
        return None;
    })
}

/// The best external "view this game" link for a match: a precise per-game page
/// (ESPN gamecast, MLB Gameday) built from the match id when we have one, else the
/// event/series page from [`resolved_event_url`]. `None` when no labelled link
/// fits. `id == 0` (demo/fixtures) never builds a per-game URL.
pub(crate) fn match_source_link(
    sport: Sport,
    id: i64,
    league: &str,
    begin_at_ms: i64,
    official: Option<&str>,
) -> Option<SourceLink> {
    if id > 0 {
        if let Some(slug) = espn_slug(sport) {
            return Some(SourceLink {
                url: format!("https://www.espn.com/{slug}/game/_/gameId/{id}"),
                label: "ESPN".to_string(),
            });
        }
        if sport == Sport::Mlb {
            return Some(SourceLink {
                url: format!("https://www.mlb.com/gameday/{id}"),
                label: "MLB".to_string(),
            });
        }
    }
    let begin_at = DateTime::from_timestamp_millis(begin_at_ms).unwrap_or_else(Utc::now);
    let url = resolved_event_url(sport, league, begin_at, official);
    let label = source_label(&url)?;
    Some(SourceLink {
        url,
        label: label.to_string(),
    })
}

/// Group already-time-sorted views into per-day buckets, and within each day
/// into per-league groups (leagues ordered by their earliest match). A league
/// group's `bo` is set only when every match in it shares the same best-of.
#[doc(hidden)]
pub fn group_days(views: Vec<MatchView>, tz: &Tz) -> Vec<DayGroup> {
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
        // Group by edition (league + series), so two editions of the same circuit
        // form distinct groups with their own header and event link.
        if let Some(lg) = day
            .leagues
            .iter_mut()
            .find(|l| l.league == v.league && l.series_name == v.series_name)
        {
            lg.matches.push(v);
        } else {
            day.leagues.push(LeagueGroup {
                league: v.league.clone(),
                series_name: v.series_name.clone(),
                event_url: String::new(),
                bo: None,
                matches: vec![v],
            });
        }
    }
    for day in &mut days {
        for lg in &mut day.leagues {
            let first = lg
                .matches
                .first()
                .map(|m| m.best_of.clone())
                .unwrap_or_default();
            let uniform = !first.is_empty() && lg.matches.iter().all(|m| m.best_of == first);
            lg.bo = uniform.then_some(first);
            // Resolve the event link once per league-group from its first row,
            // rather than for every row in `to_view` (only this value is ever
            // used — see `MatchView::league_url`).
            lg.event_url = lg
                .matches
                .first()
                .map(|m| {
                    let begin =
                        DateTime::from_timestamp_millis(m.begin_at_ms).unwrap_or_else(Utc::now);
                    let official = (!m.league_url.is_empty()).then_some(m.league_url.as_str());
                    resolved_event_url(m.sport, &m.league, begin, official)
                })
                .unwrap_or_default();
        }
        // Keep each sport's events together within the day (CS2, then LoL).
        // Stable sort preserves the time order within a sport.
        day.leagues
            .sort_by_key(|lg| match lg.matches.first().map(|m| m.sport) {
                Some(Sport::Cs2) => 0u8,
                Some(Sport::Lol) => 1,
                Some(Sport::Mlb) => 2,
                Some(Sport::Nhl) => 3,
                Some(Sport::Nba) => 4,
                Some(Sport::Nfl) => 5,
                Some(Sport::Soccer) => 6,
                Some(Sport::Motorsport) => 7,
                None => 8,
            });
    }
    days
}

/// Fold a rally's per-stage rows into one landmark row per local day for the
/// schedule views — "Day 1", "Day 2", … with the day carrying the power stage
/// flagged. The event page keeps the full per-stage breakdown; only the main
/// schedule condenses, so a 16-stage rally reads as a handful of day rows beside
/// the other sports. Each day row anchors at that day's first stage (its real
/// start time and venue-local clock). Non-WRC rows — placeholders, F1/WEC
/// sessions, every other sport — pass straight through. Output is re-sorted by
/// start time, which also reorders any events `next_motorsport_events` appended
/// out of window. Idempotent on input with no timed WRC stages.
#[doc(hidden)]
pub fn collapse_wrc_days(views: Vec<MatchView>, tz: &Tz, snap: &Snapshot) -> Vec<MatchView> {
    use std::collections::{BTreeSet, HashMap, HashSet};

    let is_stage = |m: &MatchView| {
        m.sport == Sport::Motorsport && m.league == "WRC" && !m.clock_label.is_empty()
    };

    // Only WRC stage rows get condensed. With none in view — every esports page,
    // and any traditional page with no WRC stage in window (the common case) —
    // skip the full O(n) cross-sport snapshot pre-scan and the regroup entirely;
    // just keep the input time-sorted (the homepage appends out-of-window
    // motorsport rows, so the sort still matters).
    if !views.iter().any(is_stage) {
        let mut views = views;
        views.sort_by_key(|m| m.begin_at_ms);
        return views;
    }

    // Leg ordinal + power-stage flag per (series, local day), taken from the whole
    // rally in the snapshot — not just the windowed subset — so a single-day view
    // still numbers its leg from the rally's first day.
    let mut days_by_series: HashMap<&str, BTreeSet<String>> = HashMap::new();
    let mut power_days: HashSet<(String, String)> = HashSet::new();
    for m in &snap.matches {
        if m.sport != Sport::Motorsport || m.league != "WRC" || m.venue_tz.is_none() {
            continue;
        }
        let day = m.begin_at.with_timezone(tz).format("%Y-%m-%d").to_string();
        if m.team_a.label.contains("Power Stage") {
            power_days.insert((m.series_name.clone(), day.clone()));
        }
        days_by_series
            .entry(m.series_name.as_str())
            .or_default()
            .insert(day);
    }

    let mut out: Vec<MatchView> = Vec::new();
    let mut groups: HashMap<(String, String), Vec<MatchView>> = HashMap::new();
    for v in views {
        if is_stage(&v) {
            let day = DateTime::from_timestamp_millis(v.begin_at_ms)
                .unwrap_or_else(Utc::now)
                .with_timezone(tz)
                .format("%Y-%m-%d")
                .to_string();
            groups
                .entry((v.series_name.clone(), day))
                .or_default()
                .push(v);
        } else {
            out.push(v);
        }
    }

    for ((series, day), mut stages) in groups {
        stages.sort_by_key(|m| m.begin_at_ms);
        // The group came from a non-empty push, so a first element exists.
        let mut rep = stages[0].clone();
        let ordinal = days_by_series
            .get(series.as_str())
            .and_then(|days| days.iter().position(|d| *d == day))
            .map_or(1, |i| i + 1);
        // Clicking the collapsed leg jumps to that day on the rally's event page,
        // where its stages are listed individually — the schedule can't show them
        // itself without undoing the per-day condensing the day grouping relies on.
        let event = full_event_name(&rep.league, &series);
        // The event route is sport-scoped (`/event/{slug}/{name}`); WRC is Motorsport.
        rep.row_href = Some(format!(
            "/event/{}/{}#day-{}",
            rep.sport.slug(),
            pct_encode(&event),
            day
        ));
        rep.team_a.label = if power_days.contains(&(series, day)) {
            format!("Day {ordinal} · Power Stage")
        } else {
            format!("Day {ordinal}")
        };
        rep.team_a.name = String::new();
        rep.status = day_status(&stages);
        out.push(rep);
    }

    out.sort_by_key(|m| m.begin_at_ms);
    out
}

/// Aggregate status of a rally day from its stages: live if any stage is, finished
/// only once every stage is, otherwise still upcoming.
fn day_status(stages: &[MatchView]) -> MatchStatus {
    if stages.iter().any(|m| m.status == MatchStatus::Live) {
        MatchStatus::Live
    } else if stages.iter().all(|m| m.status == MatchStatus::Finished) {
        MatchStatus::Finished
    } else {
        MatchStatus::Upcoming
    }
}

#[doc(hidden)]
#[allow(clippy::too_many_arguments)]
pub fn matches_in_window(
    snap: &Snapshot,
    traditional: bool,
    sport: Option<Sport>,
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
        .filter(|m| m.sport.traditional() == traditional)
        .filter(|m| sport.is_none_or(|g| m.sport == g))
        .filter(|m| m.begin_at >= start && m.begin_at < end)
        .map(|m| to_view(m, tz, now, hour24))
        .collect()
}

/// The mode (traditional vs esports) and optional within-mode sport for a filter
/// slug: "trad" ⇒ all traditional sports; "mlb"/"f1" ⇒ that one traditional
/// sport; "cs2"/"lol" ⇒ that esports title; anything else (incl. "all") ⇒ all
/// esports. The client filters traditional sports further (the sport tabs), so
/// the homepage uses "trad" and narrows client-side.
fn mode_filter(game_filter: &str) -> (bool, Option<Sport>) {
    if game_filter == "trad" {
        return (true, None);
    }
    let sport = Sport::from_filter(game_filter);
    (sport.is_some_and(Sport::traditional), sport)
}

/// Parse an IANA tz name, falling back to the configured default.
fn resolve_tz(name: &str, default: Tz) -> Tz {
    name.parse::<Tz>().unwrap_or(default)
}

/// Freshness metadata copied out of the snapshot before the read lock is
/// released, so the post-lock assembly doesn't borrow the snapshot.
struct FreshMeta {
    fetched_at: DateTime<Utc>,
    stale: bool,
    using_fixture: bool,
}

/// Build the final `ScheduleView` from grouped days + freshness meta. Shared by
/// every schedule builder so the wire shape stays identical across them; only the
/// date-navigation trio differs per view.
#[allow(clippy::too_many_arguments)]
fn assemble_schedule_view(
    days: Vec<DayGroup>,
    tz: &Tz,
    now: DateTime<Utc>,
    meta: FreshMeta,
    demo: bool,
    hour24: bool,
    date_label: Option<String>,
    prev_date: Option<String>,
    next_date: Option<String>,
) -> ScheduleView {
    ScheduleView {
        days,
        today_key: now
            .with_timezone(tz)
            .date_naive()
            .format("%Y-%m-%d")
            .to_string(),
        fetched_at_ms: meta.fetched_at.timestamp_millis(),
        fetched_label: time_label(meta.fetched_at.with_timezone(tz), hour24),
        stale: meta.stale,
        using_fixture: meta.using_fixture,
        demo_forced: demo,
        date_label,
        prev_date,
        next_date,
    }
}

/// The snapshot-dependent phase of the homepage view: filter the window, surface
/// each motorsport series' current/next event, and collapse rally stages — every
/// step that borrows `snap`. Returns owned rows + freshness meta so the caller can
/// drop the read lock before the sort/clone-heavy `group_days`.
fn homepage_rows(
    snap: &Snapshot,
    cfg: &Config,
    now: DateTime<Utc>,
    game_filter: &str,
    tz: &Tz,
    hour24: bool,
) -> (Vec<MatchView>, FreshMeta) {
    let (traditional, sport) = mode_filter(game_filter);
    let today = now.with_timezone(tz).date_naive();
    let start = local_day_start(tz, today);
    // Traditional sports play daily, so cap the default forward window (the
    // client's "show later days" extends it up to TRAD_FORWARD_MAX). Esports
    // show the full upcoming horizon.
    let end = if traditional {
        local_day_start(
            tz,
            today + Duration::days(crate::types::TRAD_FORWARD_DAYS + 1),
        )
    } else {
        now + Duration::days(cfg.upcoming_days)
    };

    let mut all = matches_in_window(snap, traditional, sport, start, end, tz, now, hour24);
    // Motorsport is episodic — events are days/weeks apart and span several days,
    // so the short daily window misses them. Always surface each series' (F1/WRC/
    // WEC) current-or-next event in full (MLB etc. stay compact). The client
    // sport-tabs/chips still filter it down if that series isn't selected.
    if traditional {
        let start_ms = start.timestamp_millis();
        let have: std::collections::HashSet<i64> = all.iter().map(|m| m.id).collect();
        for m in next_motorsport_events(snap) {
            // Surface the event's today-or-later rows only. Filter/dedup before
            // to_view so the dropped past rows never pay for the conversion.
            if m.begin_at.timestamp_millis() >= start_ms && !have.contains(&m.id) {
                all.push(to_view(m, tz, now, hour24));
            }
        }
    }
    // Condense each rally's stages into per-day rows and re-sort by start.
    let all = collapse_wrc_days(all, tz, snap);
    let meta = FreshMeta {
        fetched_at: snap.fetched_at,
        stale: snap.stale,
        using_fixture: snap.using_fixture,
    };
    (all, meta)
}

/// Homepage: live matches pinned, then the next `upcoming_days` grouped by day.
#[must_use]
pub fn homepage_view(game_filter: &str, tz_name: &str, hour24: bool) -> ScheduleView {
    let cfg = config();
    let tz = resolve_tz(tz_name, cfg.tz);
    let now = Utc::now();

    let snap = SNAPSHOT.read().unwrap_or_else(PoisonError::into_inner);
    let (rows, meta) = homepage_rows(&snap, cfg, now, game_filter, &tz, hour24);
    // Done with the snapshot — release the read lock before group_days (sort +
    // clones) so it doesn't block the poller's periodic write.
    drop(snap);

    assemble_schedule_view(
        group_days(rows, &tz),
        &tz,
        now,
        meta,
        cfg.demo,
        hour24,
        None,
        None,
        None,
    )
}

/// End-to-end homepage render against an explicit snapshot — the prod pipeline
/// minus the global lock. Used by benches/examples (the lock is uncontended
/// there, so this measures the same CPU cost a request pays).
#[doc(hidden)]
pub fn homepage_render(
    snap: &Snapshot,
    cfg: &Config,
    now: DateTime<Utc>,
    game_filter: &str,
    tz: &Tz,
    hour24: bool,
) -> ScheduleView {
    let (rows, meta) = homepage_rows(snap, cfg, now, game_filter, tz, hour24);
    assemble_schedule_view(
        group_days(rows, tz),
        tz,
        now,
        meta,
        cfg.demo,
        hour24,
        None,
        None,
        None,
    )
}

/// Every row of each motorsport series' soonest not-yet-finished event (an F1
/// weekend, a rally, an endurance round), borrowed from the snapshot.
///
/// Motorsport series are episodic — often more than a week between events, and a
/// single event spans several days — so the short traditional forward window
/// misses them; this surfaces, per series (by league), the whole of its current
/// or next event. Returns borrowed `NormalizedMatch` rows so the caller can
/// window and de-duplicate *before* paying for the per-row `to_view` conversion
/// (a rally has up to ~16 stages, most of them already-finished past days the
/// homepage then drops). Empty off-season.
#[doc(hidden)]
pub fn next_motorsport_events(snap: &Snapshot) -> Vec<&NormalizedMatch> {
    use std::collections::{HashMap, HashSet};
    // One pass: per league, the series_name of its soonest live/upcoming row.
    let mut soonest: HashMap<&str, (DateTime<Utc>, &str)> = HashMap::new();
    for m in &snap.matches {
        if m.sport != Sport::Motorsport
            || !matches!(m.status, MatchStatus::Live | MatchStatus::Upcoming)
        {
            continue;
        }
        soonest
            .entry(m.league.as_str())
            .and_modify(|cur| {
                if m.begin_at < cur.0 {
                    *cur = (m.begin_at, m.series_name.as_str());
                }
            })
            .or_insert((m.begin_at, m.series_name.as_str()));
    }
    let series: HashSet<&str> = soonest.values().map(|(_, s)| *s).collect();
    snap.matches
        .iter()
        .filter(|m| m.sport == Sport::Motorsport && series.contains(m.series_name.as_str()))
        .collect()
}

/// Single-day view with prev/next navigation.
#[must_use]
pub fn day_view(date: &str, game_filter: &str, tz_name: &str, hour24: bool) -> ScheduleView {
    let cfg = config();
    let tz = resolve_tz(tz_name, cfg.tz);
    let (traditional, sport) = mode_filter(game_filter);
    let now = Utc::now();

    let day = NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .unwrap_or_else(|_| now.with_timezone(&tz).date_naive());
    let start = local_day_start(&tz, day);
    let end = local_day_start(&tz, day + Duration::days(1));

    let snap = SNAPSHOT.read().unwrap_or_else(PoisonError::into_inner);
    let all = matches_in_window(&snap, traditional, sport, start, end, &tz, now, hour24);
    let all = collapse_wrc_days(all, &tz, &snap);
    let (fetched_at, stale, using_fixture) = (snap.fetched_at, snap.stale, snap.using_fixture);
    drop(snap);

    let local_noon = tz
        .from_local_datetime(&day.and_hms_opt(12, 0, 0).unwrap())
        .earliest()
        .unwrap_or_else(|| tz.from_utc_datetime(&day.and_hms_opt(12, 0, 0).unwrap()));

    assemble_schedule_view(
        group_days(all, &tz),
        &tz,
        now,
        FreshMeta {
            fetched_at,
            stale,
            using_fixture,
        },
        cfg.demo,
        hour24,
        Some(day_label(local_noon)),
        Some((day - Duration::days(1)).format("%Y-%m-%d").to_string()),
        Some((day + Duration::days(1)).format("%Y-%m-%d").to_string()),
    )
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
    let (traditional, sport) = mode_filter(game_filter);
    let now = Utc::now();
    let today = now.with_timezone(&tz).date_naive();

    let parse = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d");
    let start_day = parse(start_date).unwrap_or(today);
    let end_day = parse(end_date).unwrap_or(today).max(start_day);
    let start = local_day_start(&tz, start_day);
    // Inclusive end day → window runs to the start of the following day.
    let end = local_day_start(&tz, end_day + Duration::days(1));

    let snap = SNAPSHOT.read().unwrap_or_else(PoisonError::into_inner);
    let all = matches_in_window(&snap, traditional, sport, start, end, &tz, now, hour24);
    let all = collapse_wrc_days(all, &tz, &snap);
    let (fetched_at, stale, using_fixture) = (snap.fetched_at, snap.stale, snap.using_fixture);
    drop(snap);

    assemble_schedule_view(
        group_days(all, &tz),
        &tz,
        now,
        FreshMeta {
            fetched_at,
            stale,
            using_fixture,
        },
        cfg.demo,
        hour24,
        None,
        None,
        None,
    )
}

/// All cached matches for one event/league (past + upcoming), grouped by day.
/// Backs the internal event page.
#[must_use]
pub fn event_view(event: &str, tz_name: &str, hour24: bool) -> ScheduleView {
    let cfg = config();
    let tz = resolve_tz(tz_name, cfg.tz);
    let now = Utc::now();

    // `event` is the full edition name (league + series), so two editions of one
    // circuit resolve to distinct pages.
    let snap = SNAPSHOT.read().unwrap_or_else(PoisonError::into_inner);
    let all: Vec<MatchView> = snap
        .matches
        .iter()
        .filter(|m| m.status != MatchStatus::Canceled)
        .filter(|m| event_name_eq(&m.league, &m.series_name, event))
        .map(|m| to_view(m, &tz, now, hour24))
        .collect();

    ScheduleView {
        days: group_days(all, &tz),
        today_key: now
            .with_timezone(&tz)
            .date_naive()
            .format("%Y-%m-%d")
            .to_string(),
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
        today_key: now
            .with_timezone(&tz)
            .date_naive()
            .format("%Y-%m-%d")
            .to_string(),
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

/// The starred match uids resolved for the notifications page: the still-upcoming
/// ones as views (sorted by start), plus the uids the snapshot knows are no longer
/// upcoming (started/finished/canceled) so the client can silently drop them. A
/// uid not in the snapshot is left alone (it may be outside the loaded window).
#[must_use]
pub fn notifications_view(
    uids: &[String],
    tz_name: &str,
    hour24: bool,
) -> crate::types::NotificationsView {
    let cfg = config();
    let tz = resolve_tz(tz_name, cfg.tz);
    let now = Utc::now();
    let wanted: std::collections::HashSet<&str> = uids.iter().map(String::as_str).collect();
    let snap = SNAPSHOT.read().unwrap_or_else(PoisonError::into_inner);
    let mut upcoming = Vec::new();
    let mut stale = Vec::new();
    for m in snap.matches.iter() {
        // Starred matches are keyed by their uid (id isn't unique across games).
        let uid = crate::types::match_uid(m.sport, m.id);
        if !wanted.contains(uid.as_str()) {
            continue;
        }
        if m.status == MatchStatus::Upcoming && m.begin_at > now {
            upcoming.push(to_view(m, &tz, now, hour24));
        } else {
            stale.push(uid);
        }
    }
    upcoming.sort_by_key(|v| v.begin_at_ms);
    crate::types::NotificationsView { upcoming, stale }
}

// ----- Sport/event subscription expansion -----------------------------------

/// A reminder to create for one upcoming match (from a sport/event subscription).
pub struct ReminderSeed {
    pub match_id: i64,
    pub sport: String,
    pub league: String,
    /// Both teams' full names, so a team subscription's reminders can be cleaned
    /// up by team on unsubscribe (mirrors the sport/league cleanup columns).
    pub team_a: String,
    pub team_b: String,
    /// Full event name (league + edition), so an event subscription's reminders
    /// can be cleaned up by event on unsubscribe.
    pub event: String,
    /// Lead time (ms before start) this reminder fires at; part of its key so a
    /// match can carry several timers.
    pub lead_ms: i64,
    pub notify_at_ms: i64,
    pub title: String,
    pub body: String,
    pub url: String,
}

impl ReminderSeed {
    /// Whether this timer should be armed at all: only when its notify instant is
    /// still ahead. A timer whose lead window already opened must NOT be armed —
    /// otherwise it's immediately "due" and fires retroactively. That happens in
    /// bulk on the first server start (subscription expansion arms every upcoming
    /// match, and any with a lead longer than the time-to-start is already past),
    /// right after subscribing, or when a timer is longer than the time remaining.
    /// Such timers are simply skipped; you only get a heads-up you're still ahead of.
    #[must_use]
    pub fn is_armable(&self, now_ms: i64) -> bool {
        self.notify_at_ms > now_ms
    }

    /// Combine this server-derived seed with a client's push subscription into a
    /// persistable reminder. The seed owns the trusted notification fields; the
    /// subscription only supplies where to deliver it.
    #[must_use]
    pub fn into_reminder(self, sub: crate::types::PushSub) -> crate::store::Reminder {
        crate::store::Reminder {
            endpoint: sub.endpoint,
            p256dh: sub.p256dh,
            auth: sub.auth,
            match_id: self.match_id,
            lead_ms: self.lead_ms,
            notify_at_ms: self.notify_at_ms,
            title: self.title,
            body: self.body,
            url: self.url,
            sport: self.sport,
            league: self.league,
            team_a: self.team_a,
            team_b: self.team_b,
            event: self.event,
        }
    }
}

/// Build a reminder seed for one upcoming match. Centralizes the time/title/
/// body/url derivation so every reminder (single-match ★ and sport/event
/// subscription) is produced server-side from the same snapshot data.
fn reminder_seed(m: &NormalizedMatch, lead_ms: i64, tz: &Tz, hour24: bool) -> ReminderSeed {
    let local = m.begin_at.with_timezone(tz);
    ReminderSeed {
        match_id: m.id,
        sport: m.sport.slug().to_string(),
        league: m.league.clone(),
        team_a: m.team_a.name.clone(),
        team_b: m.team_b.name.clone(),
        event: full_event_name(&m.league, &m.series_name),
        lead_ms,
        notify_at_ms: m.begin_at.timestamp_millis() - lead_ms,
        // The American team sports read "away at home"; soccer (neutral-venue
        // World Cup games) and esports read "vs".
        title: format!(
            "{} {} {}",
            m.team_a.label,
            match m.sport {
                Sport::Mlb | Sport::Nhl | Sport::Nba | Sport::Nfl => "at",
                _ => "vs",
            },
            m.team_b.label
        ),
        // Tag the time with the viewer's zone abbreviation so it's unambiguous
        // (and obviously the viewer's local time, not the server's), in their
        // chosen 12h/24h format.
        body: format!(
            "{} · {} {}",
            m.league,
            time_label(local, hour24),
            local.format("%Z")
        ),
        url: resolved_event_url(m.sport, &m.league, m.begin_at, m.league_url.as_deref()),
    }
}

/// Upcoming matches matching a subscription scope, as reminder seeds. `kind` is
/// "sport" (value = "cs2"/"lol"), "league" (value = league name), "team" (value =
/// full team name, matched against either side), or "event" (value = the full
/// event name, e.g. an F1 Grand Prix). `tz_name` is the subscriber's IANA zone
/// (empty ⇒ the server default) and `hour24` their clock format, used to format
/// each body's start time.
#[must_use]
pub fn scope_reminder_seeds(
    kind: &str,
    value: &str,
    lead_ms: i64,
    tz_name: &str,
    hour24: bool,
) -> Vec<ReminderSeed> {
    let cfg = config();
    let tz = resolve_tz(tz_name, cfg.tz);
    let now = Utc::now();
    let snap = SNAPSHOT.read().unwrap_or_else(PoisonError::into_inner);
    snap.matches
        .iter()
        .filter(|m| m.status != MatchStatus::Canceled && m.begin_at > now)
        .filter(|m| match kind {
            "sport" => m.sport.slug() == value,
            "league" => m.league == value,
            "team" => m.team_a.name == value || m.team_b.name == value,
            "event" => event_name_eq(&m.league, &m.series_name, value),
            _ => false,
        })
        .map(|m| reminder_seed(m, lead_ms, &tz, hour24))
        .collect()
}

/// Reminder seeds for a single upcoming starred match by id — one per lead
/// offset, or empty if the match is unknown, already started, or canceled. Lets
/// the server (not the client) decide each notification's time/title/body/url.
/// `tz_name` is the viewer's IANA zone (empty ⇒ the server default) and `hour24`
/// their clock format, used to format the body's start time.
#[must_use]
pub fn reminder_seeds_for_match(
    match_id: i64,
    sport: &str,
    leads: &[i64],
    tz_name: &str,
    hour24: bool,
) -> Vec<ReminderSeed> {
    let cfg = config();
    let tz = resolve_tz(tz_name, cfg.tz);
    let now = Utc::now();
    let snap = SNAPSHOT.read().unwrap_or_else(PoisonError::into_inner);
    // Match on (id, sport); an empty sport (older client) falls back to id-only.
    let Some(m) = snap.matches.iter().find(|m| {
        m.id == match_id
            && (sport.is_empty() || m.sport.slug() == sport)
            && m.status != MatchStatus::Canceled
            && m.begin_at > now
    }) else {
        return Vec::new();
    };
    leads
        .iter()
        .map(|&lead| reminder_seed(m, lead, &tz, hour24))
        .collect()
}

// ----- Match detail + event standings/bracket ------------------------------

/// Basics for the match detail page: the formatted view, its streams, the
/// tournament id, and league name. `None` when the match isn't in the window.
#[must_use]
#[allow(clippy::type_complexity)]
pub fn match_basics(
    match_id: i64,
    sport: Sport,
    tz_name: &str,
    hour24: bool,
) -> Option<(
    MatchView,
    Vec<StreamView>,
    Option<i64>,
    String,
    Option<crate::types::MotorResultRef>,
)> {
    let cfg = config();
    let tz = resolve_tz(tz_name, cfg.tz);
    let now = Utc::now();
    let snap = SNAPSHOT.read().unwrap_or_else(PoisonError::into_inner);
    // (id, sport) is the match identity — ids aren't unique across games.
    let m = snap
        .matches
        .iter()
        .find(|m| m.id == match_id && m.sport == sport)?;
    Some((
        to_view(m, &tz, now, hour24),
        m.streams.clone(),
        m.tournament_id,
        m.league.clone(),
        m.motor_result_ref.clone(),
    ))
}

/// The tournament (stage) ids of an event's cached matches, ordered by when each
/// stage started — so a multi-stage event (e.g. Swiss stages → playoffs) renders
/// its stages in order. `event` is the full edition name (league + series).
#[must_use]
pub fn event_stages(event: &str) -> Vec<(i64, Sport)> {
    stages_matching(|m| event_name_eq(&m.league, &m.series_name, event))
}

/// The stage tournament ids of a league filter's cached matches. Like
/// [`event_stages`] but keyed on the (short) league name, which is what the
/// front-page event filter selects.
#[must_use]
pub fn league_stages(league: &str) -> Vec<(i64, Sport)> {
    stages_matching(|m| m.league == league)
}

/// Distinct `(tournament id, sport)` of the cached matches passing `pred`, ordered
/// by when each stage started (its earliest match). Capturing the sport here saves
/// [`event_info`] a per-stage snapshot scan to look it up.
fn stages_matching(pred: impl Fn(&NormalizedMatch) -> bool) -> Vec<(i64, Sport)> {
    let snap = SNAPSHOT.read().unwrap_or_else(PoisonError::into_inner);
    let mut earliest: std::collections::HashMap<i64, (i64, Sport)> =
        std::collections::HashMap::new();
    for m in &snap.matches {
        if pred(m) {
            if let Some(tid) = m.tournament_id {
                let ms = m.begin_at.timestamp_millis();
                earliest
                    .entry(tid)
                    .and_modify(|e| e.0 = e.0.min(ms))
                    .or_insert((ms, m.sport));
            }
        }
    }
    let mut stages: Vec<(i64, (i64, Sport))> = earliest.into_iter().collect();
    stages.sort_by_key(|&(_, (ms, _))| ms);
    stages
        .into_iter()
        .map(|(tid, (_, sport))| (tid, sport))
        .collect()
}

/// Resolve a list of `(stage tournament id, sport)` to their [`EventInfo`]s
/// (standings + brackets), dropping empties and tagging each with the event
/// name. Order is preserved so stages render Swiss → playoffs.
pub async fn stages_info(stages: Vec<(i64, Sport)>, event: &str) -> Vec<EventInfo> {
    // Fetch every stage concurrently (each is an independent set of API calls),
    // preserving order, so a multi-stage event isn't N round-trips deep.
    futures::future::join_all(
        stages
            .into_iter()
            .map(|(tid, sport)| event_info(tid, sport)),
    )
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
pub async fn event_info(tournament_id: i64, sport: Sport) -> EventInfo {
    {
        let cache = EVENTS.read().unwrap_or_else(PoisonError::into_inner);
        if let Some(c) = cache.get(&tournament_id) {
            if Utc::now() - c.fetched_at < Duration::minutes(EVENT_TTL_MIN) {
                return c.info.clone();
            }
        }
    }
    let key = tournament_id.to_string();
    if let Some((info, fetched_at)) = db_cache_get::<EventInfo>("event", &key) {
        if Utc::now() - fetched_at < Duration::minutes(EVENT_TTL_MIN) {
            EVENTS
                .write()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(
                    tournament_id,
                    CachedEvent {
                        info: info.clone(),
                        fetched_at,
                    },
                );
            return info;
        }
    }
    fetch_event(tournament_id, sport).await
}

/// Fetch a tournament's standings + bracket from the API unconditionally and
/// store it in the EVENTS cache. The poller's pre-warm calls this directly (with
/// its own freshness check) to keep active events hot below the serve TTL.
async fn fetch_event(tournament_id: i64, sport: Sport) -> EventInfo {
    let now = Utc::now();
    // `sport` (for sport-appropriate standings labels) is passed in by the caller,
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
        sport,
        standings,
        rounds,
        swiss,
    };
    EVENTS
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(
            tournament_id,
            CachedEvent {
                info: info.clone(),
                fetched_at: now,
            },
        );
    if !info.is_empty() {
        db_cache_put("event", &tournament_id.to_string(), &info, now);
    }
    info
}

/// Every stage `(tournament id, sport)` of an edition that has a recent or
/// upcoming match — the events a visitor is likely to open. All stages, so a
/// recently-finished event's older group stages are warmed too (the event page
/// loads them all).
fn active_tournaments(now: DateTime<Utc>) -> Vec<(i64, Sport)> {
    let lo = (now - Duration::days(4)).timestamp_millis();
    let hi = (now + Duration::days(config().upcoming_days)).timestamp_millis();
    let snap = SNAPSHOT.read().unwrap_or_else(PoisonError::into_inner);
    // Editions with a recent/upcoming match.
    let mut active: std::collections::HashSet<String> = std::collections::HashSet::new();
    for m in &snap.matches {
        if (lo..=hi).contains(&m.begin_at.timestamp_millis()) {
            active.insert(full_event_name(&m.league, &m.series_name));
        }
    }
    // Every stage tournament of those editions.
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for m in &snap.matches {
        if let Some(tid) = m.tournament_id {
            if !seen.contains(&tid) && active.contains(&full_event_name(&m.league, &m.series_name))
            {
                seen.insert(tid);
                out.push((tid, m.sport));
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
    let stale: Vec<(i64, Sport)> = active_tournaments(now)
        .into_iter()
        .filter(|(tid, _)| {
            let cache = EVENTS.read().unwrap_or_else(PoisonError::into_inner);
            cache.get(tid).is_none_or(|c| now - c.fetched_at >= prewarm)
        })
        .take(MAX_PREWARM_PER_CYCLE)
        .collect();
    futures::future::join_all(
        stale
            .into_iter()
            .map(|(tid, sport)| fetch_event(tid, sport)),
    )
    .await;
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
                ev.insert(
                    tid,
                    CachedEvent {
                        info,
                        fetched_at: now,
                    },
                );
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
            br(
                "Final",
                "",
                vec![bm("TBD", "TBD", None, None, "", vec![(1, 0), (1, 1)])],
            ),
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
            br(
                "Final",
                "upper",
                vec![bm(t[0], t[2], Some(1), Some(2), "b", vec![(0, 0), (0, 1)])],
            ),
            // Losers of the upper semis drop here.
            br(
                "Round 1",
                "lower",
                vec![bm(t[1], t[3], Some(2), Some(0), "a", vec![(0, 0), (0, 1)])],
            ),
            // Lower-round winner vs the upper-final loser.
            br(
                "Final",
                "lower",
                vec![bm(t[1], t[0], Some(1), Some(2), "b", vec![(2, 0), (1, 0)])],
            ),
            br(
                "Final",
                "final",
                vec![bm(t[2], t[0], Some(3), Some(1), "a", vec![(1, 0), (3, 0)])],
            ),
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
        sport: if lol { Sport::Lol } else { Sport::Cs2 },
        standings,
        rounds,
        swiss: Vec::new(),
    }
}

// Shared builders for the larger demo brackets.
fn dbm(
    a: &str,
    b: &str,
    sa: Option<i64>,
    sb: Option<i64>,
    w: &str,
    feeders: &[(usize, usize)],
) -> BracketMatch {
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
        dbr(
            "Final",
            "upper",
            vec![dbm(
                "FaZe",
                "SPIRIT",
                Some(2),
                Some(1),
                "a",
                &[(1, 0), (1, 1)],
            )],
        ),
        // Loser's bracket (the quarterfinal losers drop in).
        dbr(
            "Round 1",
            "lower",
            vec![
                dbm("VIT", "MOUZ", Some(2), Some(0), "a", &[(0, 0), (0, 1)]),
                dbm("G2", "NIP", Some(2), Some(1), "a", &[(0, 2), (0, 3)]),
            ],
        ),
        dbr(
            "Final",
            "lower",
            vec![dbm("VIT", "G2", Some(2), Some(1), "a", &[(3, 0), (3, 1)])],
        ),
        // Grand final.
        dbr(
            "Final",
            "final",
            vec![dbm("FaZe", "VIT", Some(3), Some(1), "a", &[(2, 0), (4, 0)])],
        ),
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
        sport: Sport::Cs2,
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
        dbr(
            "Final",
            "",
            vec![dbm("FaZe", "SPIRIT", None, None, "", &[(2, 0), (2, 1)])],
        ),
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
        sport: Sport::Cs2,
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
        dbr(
            "Final",
            "",
            vec![dbm("Alpha", "Foxtrot", None, None, "", &[(1, 0), (1, 1)])],
        ),
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
        dbr(
            "Final",
            "upper",
            vec![dbm("Sun", "Star", Some(2), Some(1), "a", &[(0, 0), (0, 1)])],
        ),
        dbr(
            "Round 1",
            "lower",
            vec![dbm(
                "Moon",
                "Comet",
                Some(2),
                Some(0),
                "a",
                &[(0, 0), (0, 1)],
            )],
        ),
        dbr(
            "Final",
            "lower",
            vec![dbm(
                "Moon",
                "Star",
                Some(2),
                Some(1),
                "a",
                &[(2, 0), (1, 0)],
            )],
        ),
        // Grand final (lower-bracket team wins) → reset.
        dbr(
            "Grand Final",
            "final",
            vec![dbm("Sun", "Moon", Some(2), Some(3), "b", &[(1, 0), (3, 0)])],
        ),
        dbr(
            "Reset",
            "final",
            vec![dbm("Sun", "Moon", Some(3), Some(2), "a", &[(4, 0)])],
        ),
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
        dbr(
            "Final",
            "upper",
            vec![dbm("A", "E", Some(2), Some(1), "a", &[(1, 0), (1, 1)])],
        ),
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
        dbr(
            "Round 3",
            "lower",
            vec![dbm("B", "F", Some(2), Some(1), "a", &[(4, 0), (4, 1)])],
        ),
        // LB final: LB-R3 winner vs the WB-final dropout.
        dbr(
            "Final",
            "lower",
            vec![dbm("B", "E", Some(2), Some(0), "a", &[(5, 0), (2, 0)])],
        ),
        dbr(
            "Grand Final",
            "final",
            vec![dbm("A", "B", Some(3), Some(2), "a", &[(2, 0), (6, 0)])],
        ),
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
        sport: Sport::Cs2,
        standings: Vec::new(),
        rounds,
        swiss: Vec::new(),
    }
}

// ----- Demo fixture (no token) ---------------------------------------------

fn demo_team(label: &str, score: Option<i64>) -> NormalizedTeam {
    NormalizedTeam {
        label: label.to_string(),
        name: label.to_string(),
        abbrev: String::new(),
        score,
    }
}

#[allow(clippy::too_many_arguments)]
fn demo_match(
    id: i64,
    sport: Sport,
    league: &str,
    tier: &str,
    begin_at: DateTime<Utc>,
    status: MatchStatus,
    best_of: i64,
    a: NormalizedTeam,
    b: NormalizedTeam,
) -> NormalizedMatch {
    NormalizedMatch {
        id,
        sport,
        league: league.to_string(),
        league_url: None,
        // Give the demo IEM a dated edition so the event/match pages can show
        // the combined "IEM Katowice 2026" name; league seasons stay bare.
        series_name: if league == "IEM" {
            "Katowice 2026".to_string()
        } else {
            String::new()
        },
        tier: tier.to_string(),
        begin_at,
        status,
        best_of: Some(best_of),
        team_a: a,
        team_b: b,
        stream_url: Some("https://www.twitch.tv/".to_string()),
        tournament_id: Some(demo_tournament_id(league)),
        venue_tz: None,
        venue_name: String::new(),
        venue_location: String::new(),
        team_a_logo: String::new(),
        team_b_logo: String::new(),
        streams: demo_streams(),
        mlb_series: None,
        motor_result_ref: None,
    }
}

/// Stable per-league tournament id for the demo, so all of an event's matches
/// share one tournament (and thus one standings/bracket).
fn demo_tournament_id(league: &str) -> i64 {
    9000 + i64::from(league.bytes().map(u32::from).sum::<u32>() % 900)
}

/// The Twitch game-category substring for an esports sport (case-insensitive
/// match), or "" for sports with no co-stream enrichment. `contains` tolerates
/// "Counter-Strike" vs "Counter-Strike 2".
fn game_needle(sport: Sport) -> &'static str {
    match sport {
        Sport::Cs2 => "counter-strike",
        Sport::Lol => "league of legends",
        _ => "",
    }
}

/// The curated co-streamer logins that apply to a match, merging the entry keyed
/// by its game (the sport slug, e.g. "lol" — applies to every league of that
/// game) with the entry keyed by its specific league (e.g. "LCK" — league-only
/// extras). Game seeds lead; league seeds follow; duplicates are dropped so a
/// login listed under both is queried once.
fn costreamer_seeds(
    costreamers: &HashMap<String, Vec<String>>,
    league: &str,
    sport: Sport,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |logins: Option<&Vec<String>>| {
        for l in logins.into_iter().flatten() {
            if !out.iter().any(|e| e.eq_ignore_ascii_case(l)) {
                out.push(l.clone());
            }
        }
    };
    push(costreamers.get(sport.slug()));
    push(costreamers.get(league));
    out
}

/// Merge Twitch live status into a match's base streams and append live seeded
/// co-streamers. `live` maps lowercased login → info; `seeds` are the league's
/// curated co-streamer logins; `game_needle` filters co-streamers to the match's
/// game (empty ⇒ append none). Base streams keep their order; co-streamers append.
fn enrich_streams(
    mut base: Vec<StreamView>,
    live: &HashMap<String, LiveInfo>,
    seeds: &[String],
    game_needle: &str,
) -> Vec<StreamView> {
    let mut present: std::collections::HashSet<String> = std::collections::HashSet::new();
    for s in &mut base {
        if let Some(login) = login_of(&s.url) {
            present.insert(login.clone());
            if let Some(info) = live.get(&login) {
                s.live = true;
                s.viewers = Some(info.viewers);
            }
        }
    }
    if !game_needle.is_empty() {
        for seed in seeds {
            let login = seed.to_ascii_lowercase();
            if present.contains(&login) {
                continue;
            }
            if let Some(info) = live.get(&login) {
                if info.game.to_ascii_lowercase().contains(game_needle) {
                    present.insert(login.clone());
                    base.push(StreamView {
                        url: format!("https://www.twitch.tv/{login}"),
                        language: info.language.clone(),
                        live: true,
                        viewers: Some(info.viewers),
                        official: false,
                        group: "costream".to_string(),
                        ..Default::default()
                    });
                }
            }
        }
    }
    base
}

/// Merge SOOP live status into a match's base streams and append live seeded
/// co-streamers. `live` maps lowercased broadcaster id → info; `seeds` are the
/// league's curated SOOP ids. Base streams keep their order; appended seeds are
/// tagged Korean (SOOP is Korea-centric and its station endpoint carries no ISO
/// language). SOOP seeds aren't game-filtered — like YouTube's, they're trusted
/// curation (the station endpoint's category is available if that ever changes).
fn enrich_soop(
    mut base: Vec<StreamView>,
    live: &HashMap<String, crate::soop::LiveInfo>,
    seeds: &[String],
) -> Vec<StreamView> {
    let mut present: std::collections::HashSet<String> = std::collections::HashSet::new();
    for s in &mut base {
        if let Some(bid) = crate::soop::bid_of(&s.url) {
            present.insert(bid.clone());
            if let Some(info) = live.get(&bid) {
                s.live = true;
                s.viewers = Some(info.viewers);
            }
        }
    }
    for seed in seeds {
        let bid = seed.to_ascii_lowercase();
        if present.contains(&bid) {
            continue;
        }
        if let Some(info) = live.get(&bid) {
            present.insert(bid.clone());
            base.push(StreamView {
                url: format!("https://www.sooplive.com/{bid}"),
                language: "ko".to_string(),
                live: true,
                viewers: Some(info.viewers),
                official: false,
                group: "costream".to_string(),
                ..Default::default()
            });
        }
    }
    base
}

/// Twitch title keywords for a league: its configured aliases (lowercased) plus
/// the raw league token as a fallback. Empty league / no alias behaves sensibly
/// (raw token only, or nothing).
fn league_keywords(
    league: &str,
    aliases: &std::collections::HashMap<String, Vec<String>>,
) -> Vec<String> {
    let mut out: Vec<String> = aliases
        .get(league)
        .into_iter()
        .flatten()
        .map(|s| s.to_ascii_lowercase())
        .collect();
    let raw = league.trim().to_ascii_lowercase();
    if !raw.is_empty() && !out.iter().any(|k| k == &raw) {
        out.push(raw);
    }
    out
}

/// Discovery keyword set for a live match, widened to its whole day: the league
/// keywords, plus the team names and acronyms of every same-event match scheduled
/// within `window` of it. Esports co-streamers frequently keep a stale stream
/// title, so matching *any* of the day's matchups (not only this one) keeps them
/// from being filtered out. `matches` is the snapshot's match list.
fn event_day_keywords(
    sport: Sport,
    league: &str,
    series_name: &str,
    begin_at: DateTime<Utc>,
    matches: &[NormalizedMatch],
    aliases: &std::collections::HashMap<String, Vec<String>>,
    window: Duration,
) -> Vec<String> {
    let mut out = league_keywords(league, aliases);
    let window_min = window.num_minutes();
    for m in matches {
        // Same sport + event (league + edition), within the day window.
        if m.sport != sport
            || !m.league.eq_ignore_ascii_case(league)
            || !m.series_name.eq_ignore_ascii_case(series_name)
            || (m.begin_at - begin_at).num_minutes().abs() > window_min
        {
            continue;
        }
        for team in [&m.team_a, &m.team_b] {
            for tok in [&team.name, &team.abbrev, &team.label] {
                let tok = tok.trim().to_ascii_lowercase();
                // Skip empties, single chars, and "TBD" placeholders.
                if tok.len() >= 2 && tok != "tbd" && !out.contains(&tok) {
                    out.push(tok);
                }
            }
        }
    }
    out
}

/// True when `kw` appears in `hay` as a whole token (not merely a substring), so
/// short abbreviations like "lec" don't match inside "collector". Both args must
/// already be lowercased.
fn contains_token(hay: &str, kw: &str) -> bool {
    if kw.is_empty() {
        return false;
    }
    hay.match_indices(kw).any(|(i, _)| {
        let before = hay[..i].chars().next_back();
        let after = hay[i + kw.len()..].chars().next();
        before.is_none_or(|c| !c.is_alphanumeric()) && after.is_none_or(|c| !c.is_alphanumeric())
    })
}

/// Attribute discovered streams to a league's match: keep those whose title or a
/// tag contains a league keyword (as a token), that clear `min_viewers`, and
/// whose login isn't already present; sort by viewers desc, take `max`, map to
/// `costream` StreamViews.
fn attribute_costreams(
    scan: &[crate::twitch_discover::DiscoveredStream],
    keywords: &[String],
    present: &std::collections::HashSet<String>,
    min_viewers: u64,
    max: usize,
) -> Vec<StreamView> {
    let mut hits: Vec<&crate::twitch_discover::DiscoveredStream> = scan
        .iter()
        .filter(|d| d.viewers >= min_viewers)
        .filter(|d| !present.contains(&d.login.to_ascii_lowercase()))
        .filter(|d| {
            let title = d.title.to_ascii_lowercase();
            keywords.iter().any(|k| {
                contains_token(&title, k)
                    || d.tags
                        .iter()
                        .any(|t| contains_token(&t.to_ascii_lowercase(), k))
            })
        })
        .collect();
    hits.sort_by(|a, b| b.viewers.cmp(&a.viewers));
    hits.into_iter()
        .take(max)
        .map(|d| StreamView {
            url: format!("https://www.twitch.tv/{}", d.login.to_ascii_lowercase()),
            language: d.language.clone(),
            live: true,
            viewers: Some(d.viewers),
            official: false,
            group: "costream".to_string(),
            ..Default::default()
        })
        .collect()
}

/// A watchable YouTube URL for a channel ident (`@handle` or `UC…`).
fn youtube_url_for(ident: &str) -> String {
    if ident.starts_with("UC") {
        format!("https://www.youtube.com/channel/{ident}")
    } else {
        format!("https://www.youtube.com/{ident}")
    }
}

/// Collapse base YouTube streams that resolve to the same channel to a single
/// entry (`channel_of`: stream url → channel id). PandaScore lists the same
/// broadcast under two URL forms — a channel permalink (`…/live`, `/@handle`) and
/// a per-video `/watch?v=` link — which are the same live stream. Among duplicates
/// keep a channel permalink over a per-video URL, and carry a language from any
/// duplicate that has one. Streams absent from the map (non-YouTube or unresolved)
/// are kept untouched, and input order is preserved.
fn dedupe_youtube_streams(
    streams: Vec<StreamView>,
    channel_of: &HashMap<String, String>,
) -> Vec<StreamView> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<StreamView> = Vec::new();
    for s in streams {
        let Some(chan) = channel_of.get(&s.url) else {
            out.push(s);
            continue;
        };
        if let Some(&i) = seen.get(chan) {
            if out[i].language.is_empty() && !s.language.is_empty() {
                out[i].language = s.language.clone();
            }
            // Prefer a stable channel permalink over the per-video URL.
            if crate::youtube::is_channel_url(&s.url)
                && !crate::youtube::is_channel_url(&out[i].url)
            {
                out[i].url = s.url.clone();
            }
            out[i].official |= s.official;
            out[i].main |= s.main;
            out[i].curated |= s.curated;
        } else {
            seen.insert(chan.clone(), out.len());
            out.push(s);
        }
    }
    out
}

/// Mark surviving base YouTube streams live (by resolved channel id) and append
/// each live seed co-streamer whose channel isn't already present. `channel_of`
/// maps a stream url → channel id; `yt_live` is keyed by channel id; `seeds` are
/// (channel id, watch url) pairs.
fn mark_and_append_youtube(
    mut streams: Vec<StreamView>,
    channel_of: &HashMap<String, String>,
    yt_live: &HashMap<String, crate::youtube::LiveInfo>,
    seeds: &[(String, String)],
) -> Vec<StreamView> {
    let mut present: std::collections::HashSet<String> = streams
        .iter()
        .filter_map(|s| channel_of.get(&s.url).cloned())
        .collect();
    for s in &mut streams {
        if let Some(cid) = channel_of.get(&s.url) {
            if let Some(info) = yt_live.get(cid) {
                s.live = true;
                s.viewers = info.viewers;
            }
        }
    }
    for (cid, url) in seeds {
        if present.contains(cid) {
            continue;
        }
        if let Some(info) = yt_live.get(cid) {
            present.insert(cid.clone());
            streams.push(StreamView {
                url: url.clone(),
                live: true,
                viewers: info.viewers,
                official: false,
                group: "costream".to_string(),
                ..Default::default()
            });
        }
    }
    streams
}

/// YouTube enrichment phase: resolve every stream to its channel id (non-YouTube
/// urls resolve to `None` with no API call), collapse same-channel duplicates,
/// then mark the survivors live and append live seed co-streamers.
async fn enrich_youtube_channels(
    enriched: Vec<StreamView>,
    seed_idents: &[String],
) -> Vec<StreamView> {
    let mut channel_of: HashMap<String, String> = HashMap::new();
    for s in &enriched {
        if channel_of.contains_key(&s.url) {
            continue;
        }
        if let Some(cid) = crate::youtube::channel_id_of_url(&s.url).await {
            channel_of.insert(s.url.clone(), cid);
        }
    }
    let deduped = dedupe_youtube_streams(enriched, &channel_of);
    // Resolve curated seeds (@handle/UC) → (channel id, watch url).
    let mut seeds: Vec<(String, String)> = Vec::new();
    for ident in seed_idents {
        if let Some(cid) = crate::youtube::channel_id_of_url(ident).await {
            seeds.push((cid, youtube_url_for(ident)));
        }
    }
    // Channels to query for live status: surviving base channels + seeds.
    let mut idents: Vec<String> = channel_of.values().cloned().collect();
    idents.extend(seeds.iter().map(|(cid, _)| cid.clone()));
    idents.sort();
    idents.dedup();
    if idents.is_empty() {
        return deduped;
    }
    let yt_live = crate::youtube::live_status(&idents).await;
    mark_and_append_youtube(deduped, &channel_of, &yt_live, &seeds)
}

/// A handful of demo broadcasts (official + a language variant + a co-streamer).
/// A rich, deterministic mock of an enriched esports stream list — an official
/// broadcast plus co-streams across several languages with live viewer counts — so
/// demo mode exercises the full streams layout (official lead, curated-first,
/// language grouping, two columns) without hitting Twitch/YouTube. Returned
/// already ordered, exactly as `live_streams_for` would.
fn demo_streams() -> Vec<StreamView> {
    let s =
        |url: &str, lang: &str, official: bool, main: bool, curated: bool, viewers: Option<u64>| {
            StreamView {
                url: url.to_string(),
                language: lang.to_string(),
                official,
                main,
                curated,
                live: true,
                viewers,
                ..Default::default()
            }
        };
    let mut out = vec![
        // Official broadcast (leads).
        s(
            "https://www.twitch.tv/riotgames",
            "en",
            true,
            true,
            true,
            Some(82_400),
        ),
        // Curated non-official feed (ranks above discovered co-streams; hidden count).
        s(
            "https://www.youtube.com/lolesports/live",
            "en",
            false,
            false,
            true,
            None,
        ),
        // Discovered co-streams across languages.
        s(
            "https://www.twitch.tv/caedrel",
            "en",
            false,
            false,
            false,
            Some(131_400),
        ),
        s(
            "https://www.twitch.tv/ls",
            "en",
            false,
            false,
            false,
            Some(8_300),
        ),
        s(
            "https://www.twitch.tv/lck",
            "ko",
            false,
            false,
            false,
            Some(55_000),
        ),
        s(
            "https://www.twitch.tv/faker",
            "ko",
            false,
            false,
            false,
            Some(12_000),
        ),
        s(
            "https://www.twitch.tv/ibai",
            "es",
            false,
            false,
            false,
            Some(30_500),
        ),
        s(
            "https://www.twitch.tv/otplol_",
            "fr",
            false,
            false,
            false,
            Some(18_200),
        ),
        s(
            "https://www.twitch.tv/cblol",
            "pt",
            false,
            false,
            false,
            Some(1_100),
        ),
        s(
            "https://www.twitch.tv/entych",
            "ja",
            false,
            false,
            false,
            Some(649),
        ),
        s(
            "https://www.twitch.tv/mixwarz",
            "",
            false,
            false,
            false,
            Some(400),
        ),
    ];
    crate::types::order_streams(&mut out);
    out
}

/// Demo data anchored to `now` so the page is populated without a token.
fn demo_matches(now: DateTime<Utc>) -> Vec<NormalizedMatch> {
    use MatchStatus::{Finished, Live, Upcoming};
    let h = Duration::hours;
    let m = Duration::minutes;
    let d = Duration::days;
    let mut out = vec![
        // CS2 — IEM: a playoff day — three finished results, a live sport, and an
        // upcoming one (shows an event whose matches are mostly final). The
        // finals stay within ~2h of now so they fall in the homepage's window.
        demo_match(
            14,
            Sport::Cs2,
            "IEM",
            "S",
            now - m(105),
            Finished,
            3,
            demo_team("NAVI", Some(2)),
            demo_team("VIT", Some(0)),
        ),
        demo_match(
            15,
            Sport::Cs2,
            "IEM",
            "S",
            now - m(70),
            Finished,
            3,
            demo_team("FaZe", Some(2)),
            demo_team("MOUZ", Some(1)),
        ),
        demo_match(
            1,
            Sport::Cs2,
            "IEM",
            "S",
            now - m(35),
            Finished,
            3,
            demo_team("NAVI", Some(1)),
            demo_team("FaZe", Some(2)),
        ),
        demo_match(
            2,
            Sport::Cs2,
            "IEM",
            "S",
            now - m(10),
            Live,
            3,
            demo_team("G2", Some(1)),
            demo_team("MOUZ", Some(0)),
        ),
        demo_match(
            3,
            Sport::Cs2,
            "IEM",
            "S",
            now + h(2),
            Upcoming,
            3,
            demo_team("VIT", None),
            demo_team("SPIRIT", None),
        ),
        // CS2 — BLAST: a result + upcoming.
        demo_match(
            4,
            Sport::Cs2,
            "BLAST",
            "S",
            now - m(50),
            Finished,
            3,
            demo_team("SPIRIT", Some(2)),
            demo_team("G2", Some(0)),
        ),
        demo_match(
            5,
            Sport::Cs2,
            "BLAST",
            "S",
            now + h(4),
            Upcoming,
            1,
            demo_team("FaZe", None),
            demo_team("MOUZ", None),
        ),
        // CS2 — ESL Pro League: upcoming only.
        demo_match(
            6,
            Sport::Cs2,
            "ESL Pro League",
            "A",
            now + h(5),
            Upcoming,
            3,
            demo_team("NIP", None),
            demo_team("BIG", None),
        ),
        // CS2 — ESL One: a larger double-elimination event.
        demo_match(
            30,
            Sport::Cs2,
            "ESL One",
            "S",
            now - m(80),
            Finished,
            3,
            demo_team("FaZe", Some(2)),
            demo_team("SPIRIT", Some(1)),
        ),
        demo_match(
            31,
            Sport::Cs2,
            "ESL One",
            "S",
            now + h(3),
            Upcoming,
            5,
            demo_team("FaZe", None),
            demo_team("VIT", None),
        ),
        // CS2 — PGL Major: a larger single-elimination event (Round of 16+).
        demo_match(
            32,
            Sport::Cs2,
            "PGL Major",
            "S",
            now - m(95),
            Finished,
            3,
            demo_team("NAVI", Some(2)),
            demo_team("FaZe", Some(1)),
        ),
        demo_match(
            33,
            Sport::Cs2,
            "PGL Major",
            "S",
            now + h(6),
            Upcoming,
            3,
            demo_team("SPIRIT", None),
            demo_team("TL", None),
        ),
        // CS2 — contrived events to examine the bracket layout engine (byes, a
        // grand-final reset, a full double-elim, and a wide 32-team tree).
        demo_match(
            40,
            Sport::Cs2,
            "Bye Cup",
            "S",
            now + h(7),
            Upcoming,
            3,
            demo_team("Alpha", None),
            demo_team("Foxtrot", None),
        ),
        demo_match(
            41,
            Sport::Cs2,
            "Reset Cup",
            "S",
            now + h(8),
            Upcoming,
            5,
            demo_team("Sun", None),
            demo_team("Moon", None),
        ),
        demo_match(
            42,
            Sport::Cs2,
            "Gauntlet",
            "S",
            now + h(9),
            Upcoming,
            3,
            demo_team("A", None),
            demo_team("B", None),
        ),
        demo_match(
            43,
            Sport::Cs2,
            "Mega Bracket",
            "S",
            now + h(10),
            Upcoming,
            3,
            demo_team("T01", None),
            demo_team("T32", None),
        ),
        demo_match(
            44,
            Sport::Cs2,
            "Long Names Cup",
            "S",
            now + h(11),
            Upcoming,
            3,
            demo_team("kukkosoosi", None),
            demo_team("FcottoNd", None),
        ),
        // LoL — LCK: a result + upcoming.
        demo_match(
            7,
            Sport::Lol,
            "LCK",
            "A",
            now - m(35),
            Finished,
            3,
            demo_team("T1", Some(2)),
            demo_team("GEN", Some(1)),
        ),
        demo_match(
            8,
            Sport::Lol,
            "LCK",
            "A",
            now + h(3),
            Upcoming,
            3,
            demo_team("HLE", None),
            demo_team("DK", None),
        ),
        // LoL — LEC: a result (FNC win) + upcoming next day.
        demo_match(
            9,
            Sport::Lol,
            "LEC",
            "A",
            now - m(20),
            Finished,
            3,
            demo_team("G2", Some(1)),
            demo_team("FNC", Some(2)),
        ),
        demo_match(
            10,
            Sport::Lol,
            "LEC",
            "A",
            now + d(1),
            Upcoming,
            3,
            demo_team("MAD", None),
            demo_team("VIT", None),
        ),
        // LoL — LPL: upcoming.
        demo_match(
            11,
            Sport::Lol,
            "LPL",
            "A",
            now + h(6),
            Upcoming,
            3,
            demo_team("BLG", None),
            demo_team("TES", None),
        ),
        // LoL — MSI / Worlds: bigger upcoming events, further out.
        demo_match(
            12,
            Sport::Lol,
            "MSI",
            "S",
            now + d(1) + h(2),
            Upcoming,
            5,
            demo_team("T1", None),
            demo_team("BLG", None),
        ),
        demo_match(
            13,
            Sport::Lol,
            "Worlds",
            "S",
            now + d(3),
            Upcoming,
            5,
            demo_team("GEN", None),
            demo_team("JDG", None),
        ),
    ];

    // Finished results across the prior ~2 weeks so "show earlier days" and the
    // calendar have history to render in demo mode. Reuse the same leagues so
    // their seeded standings/brackets apply to the detail pages.
    let cs_events = ["IEM", "BLAST", "ESL Pro League"];
    let lol_events = ["LCK", "LEC", "LPL"];
    let cs_teams = [
        ("NAVI", "FaZe"),
        ("VIT", "MOUZ"),
        ("G2", "SPIRIT"),
        ("BIG", "NIP"),
        ("FaZe", "VIT"),
        ("MOUZ", "NAVI"),
    ];
    let lol_teams = [
        ("T1", "GEN"),
        ("HLE", "DK"),
        ("BLG", "TES"),
        ("G2", "FNC"),
        ("GEN", "T1"),
        ("DK", "BLG"),
    ];
    let mut id = 100;
    for day in 1..=12i64 {
        let i = (day - 1) as usize;
        let (a, b) = cs_teams[i % cs_teams.len()];
        out.push(demo_match(
            id,
            Sport::Cs2,
            cs_events[i % cs_events.len()],
            "S",
            now - d(day) - h(3),
            Finished,
            3,
            demo_team(a, Some(2)),
            demo_team(b, Some(1)),
        ));
        id += 1;
        let (a, b) = lol_teams[i % lol_teams.len()];
        out.push(demo_match(
            id,
            Sport::Lol,
            lol_events[i % lol_events.len()],
            "A",
            now - d(day) - h(2),
            Finished,
            3,
            demo_team(a, Some(2)),
            demo_team(b, Some(0)),
        ));
        id += 1;
    }
    // Match the live poll path (which loads DB-sorted), so day grouping — which
    // relies on time order — produces one group per day.
    out.sort_by_key(|m| m.begin_at);
    out
}

// ----- Bench/profile fixtures (server-only, doc-hidden) --------------------

/// Build a `Snapshot` from explicit rows (bench/example helper).
#[doc(hidden)]
#[must_use]
pub fn snapshot_from(matches: Vec<NormalizedMatch>, now: DateTime<Utc>) -> Snapshot {
    Snapshot {
        matches,
        fetched_at: now,
        stale: false,
        using_fixture: false,
    }
}

/// Deterministic synthetic match set of size `n` for benches/profiling: rows
/// spread across −5..+14 days around `now`, cycling sports/leagues/statuses, plus one
/// multi-stage WRC rally so `collapse_wrc_days` does real work. Index-seeded (no
/// RNG) so runs are comparable.
#[doc(hidden)]
#[must_use]
pub fn synthetic_matches(n: usize, now: DateTime<Utc>) -> Vec<NormalizedMatch> {
    use MatchStatus::{Finished, Live, Upcoming};
    let sports = [
        Sport::Cs2,
        Sport::Lol,
        Sport::Mlb,
        Sport::Nhl,
        Sport::Soccer,
    ];
    let leagues = ["LEC", "LCK", "IEM", "MLB", "NHL", "EPL"];
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let idx = i as i64;
        let sport = sports[i % sports.len()];
        let league = leagues[i % leagues.len()];
        let begin_at = now + Duration::days((idx % 20) - 5) + Duration::minutes((idx % 24) * 30);
        let status = match i % 7 {
            0 => Live,
            1 | 2 => Finished,
            _ => Upcoming,
        };
        let score = matches!(status, Finished | Live).then_some(idx % 5);
        let tier = if i % 3 == 0 { "s" } else { "a" };
        out.push(demo_match(
            idx,
            sport,
            league,
            tier,
            begin_at,
            status,
            3,
            demo_team(&format!("T{idx}A"), score),
            demo_team(&format!("T{idx}B"), score.map(|s| 5 - s)),
        ));
    }
    // One multi-stage rally (12 stages over 4 days) to exercise collapse_wrc_days.
    let series = "Synthetic Rally 2026";
    for stage in 0..12i64 {
        let day = stage / 3; // ~3 stages/day across 4 days
        let mut m = NormalizedMatch::team_sport(
            1_000_000 + stage,
            Sport::Motorsport,
            "WRC",
            now + Duration::days(day) + Duration::hours(2 + (stage % 3)),
            if stage < 3 { Finished } else { Upcoming },
            demo_team(&format!("SS{}", stage + 1), None),
            demo_team("", None),
        );
        m.series_name = series.to_string();
        m.venue_tz = Some("Europe/Athens".to_string());
        out.push(m);
    }
    out
}

/// `synthetic_matches` wrapped in a `Snapshot` anchored at `now`.
#[doc(hidden)]
#[must_use]
pub fn synthetic_snapshot(n: usize, now: DateTime<Utc>) -> Snapshot {
    snapshot_from(synthetic_matches(n, now), now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_score_cache_evicts_oldest_over_cap() {
        let mk = |ms: i64| CachedBoxScore {
            bs: crate::types::BoxScore::default(),
            fetched_at: DateTime::from_timestamp_millis(ms).unwrap(),
            errored: false,
        };
        let mut m: HashMap<String, CachedBoxScore> = HashMap::new();
        // Fill to cap (3) with increasing fetch times.
        for ms in 1..=3 {
            insert_capped(&mut m, format!("k{ms}"), mk(ms), 3);
        }
        assert_eq!(m.len(), 3);
        // A fourth distinct key over cap evicts the oldest (k1, fetched_at = 1).
        insert_capped(&mut m, "k4".into(), mk(4), 3);
        assert_eq!(m.len(), 3, "stays at cap");
        assert!(!m.contains_key("k1"), "oldest evicted");
        assert!(m.contains_key("k4"), "newest inserted");
        assert!(
            m.contains_key("k2") && m.contains_key("k3"),
            "others retained"
        );
        // Re-inserting an existing key at cap updates in place, no eviction.
        insert_capped(&mut m, "k4".into(), mk(9), 3);
        assert_eq!(m.len(), 3, "in-place update does not evict");
    }

    #[test]
    fn source_link_espn_sports_build_gamecast_urls() {
        for (sport, slug) in [
            (Sport::Nhl, "nhl"),
            (Sport::Nba, "nba"),
            (Sport::Nfl, "nfl"),
        ] {
            let s = match_source_link(sport, 401688573, "NHL", 0, None).unwrap();
            assert_eq!(
                s.url,
                format!("https://www.espn.com/{slug}/game/_/gameId/401688573")
            );
            assert_eq!(s.label, "ESPN");
        }
    }

    #[test]
    fn source_link_mlb_builds_gameday_url() {
        let s = match_source_link(Sport::Mlb, 745444, "MLB", 0, None).unwrap();
        assert_eq!(s.url, "https://www.mlb.com/gameday/745444");
        assert_eq!(s.label, "MLB");
    }

    #[test]
    fn source_link_zero_id_falls_back_to_event_page() {
        // id 0 (demo/fixtures) must NOT build a bogus per-game URL; NHL falls back to
        // the league page, labelled by host.
        let s = match_source_link(Sport::Nhl, 0, "NHL", 0, None).unwrap();
        assert!(s.url.contains("nhl.com"), "got {}", s.url);
        assert_eq!(s.label, "NHL");
    }

    #[test]
    fn source_link_esports_uses_official_then_liquipedia() {
        // An official URL from the source is used as-is, labelled by host.
        let s = match_source_link(
            Sport::Cs2,
            123,
            "IEM",
            0,
            Some("https://liquipedia.net/counterstrike/IEM_Cologne"),
        )
        .unwrap();
        assert_eq!(s.url, "https://liquipedia.net/counterstrike/IEM_Cologne");
        assert_eq!(s.label, "Liquipedia");
        // With no official URL, falls back to a Liquipedia search (still Liquipedia).
        let s2 = match_source_link(Sport::Cs2, 123, "IEM", 0, None).unwrap();
        assert!(s2.url.contains("liquipedia.net"), "got {}", s2.url);
        assert_eq!(s2.label, "Liquipedia");
    }

    #[test]
    fn source_link_motorsport_series_fallbacks() {
        let f1 = match_source_link(Sport::Motorsport, 0, "F1", 0, None).unwrap();
        assert!(f1.url.contains("formula1.com"), "got {}", f1.url);
        assert_eq!(f1.label, "Formula 1");
        let wrc = match_source_link(Sport::Motorsport, 0, "WRC", 0, None).unwrap();
        assert_eq!(wrc.label, "WRC");
        let wec = match_source_link(Sport::Motorsport, 0, "WEC", 0, None).unwrap();
        assert_eq!(wec.label, "WEC");
        let moto = match_source_link(Sport::Motorsport, 0, "MotoGP", 0, None).unwrap();
        assert!(moto.url.contains("motogp.com"), "got {}", moto.url);
        assert_eq!(moto.label, "MotoGP");
    }

    #[test]
    fn source_label_unknown_or_empty_is_none() {
        assert_eq!(source_label(""), None);
        assert_eq!(source_label("https://example.com/whatever"), None);
    }

    #[test]
    fn source_label_recognizes_wikipedia() {
        assert_eq!(
            source_label("https://en.wikipedia.org/wiki/2026_Madrid_Grand_Prix"),
            Some("Wikipedia")
        );
    }

    /// A WRC stage row (real start + venue tz) for the collapse test.
    fn wrc_stage(id: i64, label: &str, series: &str, begin: &str) -> NormalizedMatch {
        let mut m = NormalizedMatch::team_sport(
            id,
            Sport::Motorsport,
            "WRC",
            begin.parse::<DateTime<Utc>>().unwrap(),
            MatchStatus::Upcoming,
            NormalizedTeam {
                label: label.into(),
                name: String::new(),
                abbrev: String::new(),
                score: None,
            },
            NormalizedTeam {
                label: String::new(),
                name: String::new(),
                abbrev: String::new(),
                score: None,
            },
        );
        m.series_name = series.to_string();
        m.venue_tz = Some("Europe/Athens".to_string());
        m
    }

    #[test]
    fn collapse_folds_wrc_stages_into_per_day_rows() {
        let series = "EKO Acropolis Rally Greece 2026";
        let mut matches = vec![
            wrc_stage(101, "SS1 EKO SSS", series, "2026-06-25T16:02:00Z"),
            wrc_stage(102, "SS2 Bauxites", series, "2026-06-26T05:45:00Z"),
            wrc_stage(103, "SS3 Parnassos", series, "2026-06-26T06:48:00Z"),
            wrc_stage(104, "SS4 Stiri", series, "2026-06-27T04:51:00Z"),
            wrc_stage(105, "SS17 Wolf Power Stage", series, "2026-06-28T11:12:00Z"),
        ];
        // A different rally with no published timetable stays a date-only
        // placeholder (no venue tz) and must pass through untouched.
        let mut placeholder = NormalizedMatch::team_sport(
            200,
            Sport::Motorsport,
            "WRC",
            "2026-07-16T12:00:00Z".parse().unwrap(),
            MatchStatus::Upcoming,
            NormalizedTeam {
                label: "Rally Estonia".into(),
                name: String::new(),
                abbrev: String::new(),
                score: None,
            },
            NormalizedTeam {
                label: String::new(),
                name: String::new(),
                abbrev: String::new(),
                score: None,
            },
        );
        placeholder.series_name = "Rally Estonia 2026".to_string();
        matches.push(placeholder);
        // An F1 session shares the sport but must keep its own per-session row.
        let mut f1 = NormalizedMatch::team_sport(
            300,
            Sport::Motorsport,
            "F1",
            "2026-06-26T13:00:00Z".parse().unwrap(),
            MatchStatus::Upcoming,
            NormalizedTeam {
                label: "Race".into(),
                name: String::new(),
                abbrev: String::new(),
                score: None,
            },
            NormalizedTeam {
                label: String::new(),
                name: String::new(),
                abbrev: String::new(),
                score: None,
            },
        );
        f1.series_name = "Austrian Grand Prix 2026".to_string();
        f1.venue_tz = Some("Europe/Vienna".to_string());
        matches.push(f1);

        let tz: Tz = "Europe/Athens".parse().unwrap();
        let now = "2026-06-20T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let snap = Snapshot {
            matches,
            fetched_at: now,
            stale: false,
            using_fixture: false,
        };
        let views: Vec<MatchView> = snap
            .matches
            .iter()
            .map(|m| to_view(m, &tz, now, false))
            .collect();
        let out = collapse_wrc_days(views, &tz, &snap);

        // The five stages fold into four day rows (June 25–28), labelled Day 1..4
        // with the power-stage day flagged; the placeholder and F1 row survive — so
        // six rows in all, in start order.
        let labels: Vec<&str> = out.iter().map(|m| m.team_a.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Day 1",
                "Day 2",
                "Race",
                "Day 3",
                "Day 4 · Power Stage",
                "Rally Estonia"
            ]
        );
        // Each day row anchors at that day's first stage (Day 2 → SS2, not SS3).
        let day2 = out.iter().find(|m| m.team_a.label == "Day 2").unwrap();
        assert_eq!(
            day2.begin_at_ms,
            "2026-06-26T05:45:00Z"
                .parse::<DateTime<Utc>>()
                .unwrap()
                .timestamp_millis()
        );
        // The collapsed rows keep a real venue-local clock (not the blank date-only
        // cell), while the untouched placeholder stays date-only.
        assert!(!out
            .iter()
            .find(|m| m.team_a.label == "Day 1")
            .unwrap()
            .clock_label
            .is_empty());
        assert!(out
            .iter()
            .find(|m| m.team_a.label == "Rally Estonia")
            .unwrap()
            .clock_label
            .is_empty());
    }

    fn at(begin: DateTime<Utc>, status: MatchStatus) -> NormalizedMatch {
        NormalizedMatch {
            id: 1,
            sport: Sport::Cs2,
            league: "X".into(),
            league_url: None,
            series_name: String::new(),
            tier: "S".into(),
            begin_at: begin,
            status,
            best_of: Some(3),
            team_a: NormalizedTeam {
                label: "A".into(),
                name: "A".into(),
                score: None,
                abbrev: String::new(),
            },
            team_b: NormalizedTeam {
                label: "B".into(),
                name: "B".into(),
                score: None,
                abbrev: String::new(),
            },
            stream_url: None,
            tournament_id: None,
            venue_tz: None,
            venue_name: String::new(),
            venue_location: String::new(),
            team_a_logo: String::new(),
            team_b_logo: String::new(),
            streams: Vec::new(),
            mlb_series: None,
            motor_result_ref: None,
        }
    }

    #[test]
    fn reconcile_wrc_keeps_finished_stages_and_drops_superseded_placeholders() {
        use crate::types::MotorResultRef;
        let now = Utc::now();
        let wrc = |id: i64, event: &str, session: &str| {
            let mut m = at(now, MatchStatus::Finished);
            m.id = id;
            m.sport = Sport::Motorsport;
            m.league = "WRC".into();
            m.motor_result_ref = Some(MotorResultRef {
                series: "WRC".into(),
                event_id: event.into(),
                session_id: session.into(),
            });
            m
        };
        // Rally A finished a previous poll — its stages are cached, not re-fetched.
        let prev = vec![wrc(11, "A", "a1"), wrc(12, "A", "a2")];
        // This poll: A's date-only placeholder is back in the feed, rally B is the
        // active one (expanded into stages), rally C is an untouched upcoming
        // placeholder, plus a non-WRC row the replace must never touch.
        let mut fresh = vec![
            wrc(10, "A", ""),
            wrc(21, "B", "b1"),
            wrc(22, "B", "b2"),
            wrc(30, "C", ""),
        ];
        let mut other = at(now, MatchStatus::Upcoming);
        other.id = 99;
        other.league = "F1".into();
        fresh.push(other);

        let keep = reconcile_wrc(&mut fresh, &prev);

        // A's superseded placeholder (10) is pruned from the upsert set; B's
        // stages, C's placeholder, and the non-WRC row survive.
        let fresh_ids: std::collections::HashSet<i64> = fresh.iter().map(|m| m.id).collect();
        assert_eq!(fresh_ids, std::collections::HashSet::from([21, 22, 30, 99]));
        // The keep-set carries A's finished stages (11, 12) forward alongside this
        // poll's WRC rows (21, 22, 30) — not the dropped placeholder (10), and not
        // the F1 row (99), a league this replace doesn't scope.
        let keep_set: std::collections::HashSet<i64> = keep.into_iter().collect();
        assert_eq!(
            keep_set,
            std::collections::HashSet::from([11, 12, 21, 22, 30])
        );
    }

    #[test]
    fn active_window_detection() {
        let now = Utc::now();
        let grace = Duration::hours(5);
        let lead = Duration::minutes(15);
        let active = |m: NormalizedMatch| is_active_window(&[m], now, grace, lead);

        assert!(
            !is_active_window(&[], now, grace, lead),
            "nothing scheduled"
        );
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
        // Single-hour 12h times are left-padded with a figure space (U+2007).
        assert_eq!(time_label(t(18, 30), false), "\u{2007}6:30 PM");
        assert_eq!(time_label(t(0, 5), true), "00:05");
        assert_eq!(time_label(t(0, 5), false), "12:05 AM");
        assert_eq!(time_label(t(12, 0), false), "12:00 PM");
    }

    #[test]
    fn effective_status_live_heuristic() {
        let now = Utc::now();
        let mut m = at(now - Duration::hours(1), MatchStatus::Upcoming);
        // Started recently, still inside the live grace → treated as Live.
        assert_eq!(effective_status(&m, now), MatchStatus::Live);
        // Just inside the grace window → still Live.
        m.begin_at = now - LIVE_GRACE + Duration::minutes(1);
        assert_eq!(effective_status(&m, now), MatchStatus::Live);
        // Past the grace window with no finished signal → assume over, rather
        // than a permanent fake "LIVE" on a match that ended hours ago.
        m.begin_at = now - LIVE_GRACE - Duration::minutes(1);
        assert_eq!(effective_status(&m, now), MatchStatus::Finished);
        // Not started yet → unchanged.
        m.begin_at = now + Duration::hours(1);
        assert_eq!(effective_status(&m, now), MatchStatus::Upcoming);
        // A real finished status is always preserved.
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
    fn to_view_hides_placeholder_zero_zero_on_finished() {
        let now = Utc::now();
        // A match marked finished but still at PandaScore's 0-0 placeholder (the
        // assume-over heuristic, or a result we haven't fetched yet) must not
        // render a fake "0 - 0" final with no winner.
        let mut fin = at(now - Duration::hours(8), MatchStatus::Finished);
        fin.team_a.score = Some(0);
        fin.team_b.score = Some(0);
        let v = to_view(&fin, &Tz::UTC, now, true);
        assert_eq!(v.team_a.score, None);
        assert_eq!(v.team_b.score, None);
        assert!(!v.team_a.winner && !v.team_b.winner);
    }

    #[test]
    fn to_view_keeps_zero_zero_soccer_draw() {
        let now = Utc::now();
        // A traditional sport the source itself marked finished can legitimately
        // end 0-0 (a soccer draw) — unlike an esports Bo-X, that's the real
        // result and must render "0 - 0", not blank.
        let mut draw = at(now - Duration::hours(2), MatchStatus::Finished);
        draw.sport = Sport::Soccer;
        draw.team_a.score = Some(0);
        draw.team_b.score = Some(0);
        let v = to_view(&draw, &Tz::UTC, now, true);
        assert_eq!(v.team_a.score, Some(0));
        assert_eq!(v.team_b.score, Some(0));
        // A draw has no winner.
        assert!(!v.team_a.winner && !v.team_b.winner);
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
            sport: Sport::Lol,
            league: league.into(),
            series_name: String::new(),
            status: MatchStatus::Upcoming,
            clock_label: String::new(),
            date_label: String::new(),
            venue_label: String::new(),
            venue_name: String::new(),
            venue_location: String::new(),
            best_of: bo.into(),
            team_a: TeamView {
                label: "A".into(),
                name: "A".into(),
                score: None,
                winner: false,
                logo: String::new(),
                abbrev: String::new(),
            },
            team_b: TeamView {
                label: "B".into(),
                name: "B".into(),
                score: None,
                winner: false,
                logo: String::new(),
                abbrev: String::new(),
            },
            league_url: String::new(),
            begin_at_ms: at_ms,
            row_href: None,
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
        let mk = |at_ms: i64, sport: Sport, league: &str| MatchView {
            id: at_ms,
            sport,
            league: league.into(),
            series_name: String::new(),
            status: MatchStatus::Upcoming,
            clock_label: String::new(),
            date_label: String::new(),
            venue_label: String::new(),
            venue_name: String::new(),
            venue_location: String::new(),
            best_of: "Bo3".into(),
            team_a: TeamView {
                label: "A".into(),
                name: "A".into(),
                score: None,
                winner: false,
                logo: String::new(),
                abbrev: String::new(),
            },
            team_b: TeamView {
                label: "B".into(),
                name: "B".into(),
                score: None,
                winner: false,
                logo: String::new(),
                abbrev: String::new(),
            },
            league_url: String::new(),
            begin_at_ms: at_ms,
            row_href: None,
        };
        // Interleaved by time: LoL, CS2, LoL, CS2.
        let views = vec![
            mk(ms(1), Sport::Lol, "LCK"),
            mk(ms(2), Sport::Cs2, "IEM"),
            mk(ms(3), Sport::Lol, "LEC"),
            mk(ms(4), Sport::Cs2, "BLAST"),
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
        let mk = |at_ms: i64, series: &str| MatchView {
            id: at_ms,
            sport: Sport::Cs2,
            league: "IEM".into(),
            series_name: series.into(),
            status: MatchStatus::Upcoming,
            clock_label: String::new(),
            date_label: String::new(),
            venue_label: String::new(),
            venue_name: String::new(),
            venue_location: String::new(),
            best_of: "Bo3".into(),
            team_a: TeamView {
                label: "A".into(),
                name: "A".into(),
                score: None,
                winner: false,
                logo: String::new(),
                abbrev: String::new(),
            },
            team_b: TeamView {
                label: "B".into(),
                name: "B".into(),
                score: None,
                winner: false,
                logo: String::new(),
                abbrev: String::new(),
            },
            league_url: String::new(),
            begin_at_ms: at_ms,
            row_href: None,
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
            .find(|l| l.series_name == "Cologne Major")
            .expect("Cologne group");
        assert_eq!(cologne.matches.len(), 2, "both Cologne matches grouped");
        assert_eq!(
            crate::types::full_event_name(&cologne.league, &cologne.series_name),
            "IEM Cologne Major"
        );
    }

    #[test]
    fn resolve_tz_falls_back() {
        assert_eq!(
            resolve_tz("Europe/London", Tz::UTC),
            chrono_tz::Europe::London
        );
        assert_eq!(resolve_tz("", Tz::UTC), Tz::UTC);
        assert_eq!(resolve_tz("Not/AZone", Tz::UTC), Tz::UTC);
    }

    #[test]
    fn event_link_official_else_liquipedia() {
        assert_eq!(
            event_link(Sport::Lol, "MSI", Some("https://lolesports.com")),
            "https://lolesports.com"
        );
        let lol = event_link(Sport::Lol, "Mid-Season Invitational", None);
        assert!(lol.starts_with("https://liquipedia.net/leagueoflegends/index.php?search="));
        assert!(lol.contains("Mid-Season%20Invitational"));
        // Blank official falls back too, with the CS wiki.
        let cs = event_link(Sport::Cs2, "BLAST Premier", Some("  "));
        assert!(cs.starts_with("https://liquipedia.net/counterstrike/index.php?search="));
        assert!(cs.contains("BLAST%20Premier"));
    }

    #[test]
    fn merge_matches_replaces_ok_games_keeps_errored() {
        let now = Utc::now();
        let mk = |id, sport, h| {
            let mut m = at(now + Duration::hours(h), MatchStatus::Upcoming);
            m.id = id;
            m.sport = sport;
            m
        };
        let prev = vec![mk(1, Sport::Cs2, 1), mk(2, Sport::Lol, 2)];
        let fresh = vec![mk(3, Sport::Cs2, 3)]; // cs2 refetched; lol failed this round
        let old_cutoff = (now - Duration::days(365)).timestamp_millis();
        let merged = merge_matches(&prev, fresh, &[Sport::Lol], old_cutoff);
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
        // Today (0) is eligible too: the main poll can miss a match that
        // finished and dropped out of /upcoming, so re-fetch today's scores
        // cheaply via the date-range path. > 1 week is locked.
        assert_eq!(refresh_interval(0), Some(Duration::minutes(30)));
        assert_eq!(refresh_interval(8), None);
    }

    // ----- Orange Cat Blacktop per-series poll cadence -----------------------

    /// A motorsport row for one series with a given status/start, for cadence
    /// tests. Reuses `at` (league is irrelevant — the cadence functions receive an
    /// already-league-filtered slice).
    fn ms(begin: DateTime<Utc>, status: MatchStatus) -> NormalizedMatch {
        let mut m = at(begin, status);
        m.sport = Sport::Motorsport;
        m.league = "WRC".into();
        m
    }

    // Tier durations distinct enough that the chosen tier is unambiguous.
    const LIVE: Duration = Duration::minutes(5);
    const NEAR: Duration = Duration::hours(3);
    const IDLE: Duration = Duration::hours(24);

    #[test]
    fn cadence_live_when_a_session_is_running() {
        let now = Utc::now();
        let ms = [ms(now - Duration::minutes(20), MatchStatus::Live)];
        assert_eq!(series_cadence(&ms, now, LIVE, NEAR, IDLE), LIVE);
    }

    #[test]
    fn cadence_live_when_a_session_starts_within_the_hour() {
        let now = Utc::now();
        let ms = [ms(now + Duration::minutes(30), MatchStatus::Upcoming)];
        assert_eq!(series_cadence(&ms, now, LIVE, NEAR, IDLE), LIVE);
    }

    #[test]
    fn cadence_near_when_an_event_is_within_two_weeks() {
        let now = Utc::now();
        let ms = [ms(now + Duration::days(5), MatchStatus::Upcoming)];
        assert_eq!(series_cadence(&ms, now, LIVE, NEAR, IDLE), NEAR);
    }

    #[test]
    fn cadence_idle_when_the_next_event_is_far_off() {
        let now = Utc::now();
        let ms = [ms(now + Duration::days(20), MatchStatus::Upcoming)];
        assert_eq!(series_cadence(&ms, now, LIVE, NEAR, IDLE), IDLE);
    }

    #[test]
    fn cadence_idle_when_there_are_no_matches() {
        let now = Utc::now();
        assert_eq!(series_cadence(&[], now, LIVE, NEAR, IDLE), IDLE);
    }

    #[test]
    fn cadence_ignores_finished_and_past_rows() {
        let now = Utc::now();
        // A just-finished session and a long-past upcoming straggler must not
        // count as live; the only real future event is far off → idle.
        let ms = [
            ms(now - Duration::hours(2), MatchStatus::Finished),
            ms(now - Duration::hours(1), MatchStatus::Upcoming), // stale, begin in the past
            ms(now + Duration::days(20), MatchStatus::Upcoming),
        ];
        assert_eq!(series_cadence(&ms, now, LIVE, NEAR, IDLE), IDLE);
    }

    #[test]
    fn cadence_picks_the_soonest_upcoming() {
        let now = Utc::now();
        // A far event and a within-the-hour event in the same series → live wins.
        let ms = [
            ms(now + Duration::days(10), MatchStatus::Upcoming),
            ms(now + Duration::minutes(40), MatchStatus::Upcoming),
        ];
        assert_eq!(series_cadence(&ms, now, LIVE, NEAR, IDLE), LIVE);
    }

    // ----- Orange Cat Blacktop standings cadence -----------------------------

    const DAILY: Duration = Duration::hours(24);
    const POST_ROUND: Duration = Duration::hours(3);

    #[test]
    fn standings_due_on_first_run() {
        let now = Utc::now();
        assert!(standings_due(&[], now, None, DAILY, POST_ROUND));
    }

    #[test]
    fn standings_due_after_the_daily_floor_even_when_quiet() {
        let now = Utc::now();
        let last = Some(now - Duration::hours(25));
        assert!(standings_due(&[], now, last, DAILY, POST_ROUND));
    }

    #[test]
    fn standings_not_due_when_recently_fetched_and_nothing_finished() {
        let now = Utc::now();
        let last = Some(now - Duration::hours(2));
        assert!(!standings_due(&[], now, last, DAILY, POST_ROUND));
    }

    #[test]
    fn standings_due_after_a_round_finishes() {
        let now = Utc::now();
        let last = Some(now - Duration::hours(4)); // past the post-round gap
        let rows = [ms(now - Duration::hours(2), MatchStatus::Finished)];
        assert!(standings_due(&rows, now, last, DAILY, POST_ROUND));
    }

    #[test]
    fn standings_not_due_again_within_the_post_round_gap() {
        let now = Utc::now();
        let last = Some(now - Duration::minutes(30)); // inside the 3h gap
        let rows = [ms(now - Duration::hours(2), MatchStatus::Finished)];
        assert!(!standings_due(&rows, now, last, DAILY, POST_ROUND));
    }

    #[test]
    fn standings_ignores_finishes_older_than_the_settle_window() {
        let now = Utc::now();
        let last = Some(now - Duration::hours(4));
        // Finished three days ago — well outside the 48h settle window, and the
        // daily floor hasn't elapsed → not due.
        let rows = [ms(now - Duration::days(3), MatchStatus::Finished)];
        assert!(!standings_due(&rows, now, last, DAILY, POST_ROUND));
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

    fn table(stage: &str, teams: &[&str]) -> EventInfo {
        EventInfo {
            event: "League".into(),
            stage: stage.into(),
            sport: Sport::Nhl,
            standings: teams
                .iter()
                .map(|t| StandingRow {
                    team: (*t).to_string(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn tables_with_team_keeps_only_the_groups_holding_a_side() {
        let tables = vec![
            table("Atlantic", &["Bruins", "Maple Leafs"]),
            table("Metropolitan", &["Rangers", "Devils"]),
            table("Central", &["Avalanche", "Stars"]),
        ];
        // Two teams in different divisions → both their tables, in order.
        let got = tables_with_team(&tables, "Bruins", "Rangers");
        let stages: Vec<&str> = got.iter().map(|t| t.stage.as_str()).collect();
        assert_eq!(stages, ["Atlantic", "Metropolitan"]);
        // Both in the same division → that one table, once.
        let same = tables_with_team(&tables, "Avalanche", "Stars");
        assert_eq!(same.len(), 1);
        assert_eq!(same[0].stage, "Central");
        // Neither present → nothing.
        assert!(tables_with_team(&tables, "Kraken", "Jets").is_empty());
    }

    #[test]
    fn standings_for_event_name_recognizes_traditional_events_only() {
        // Known traditional events resolve (the caches are empty in tests, so the
        // tables are empty — but it's `Some`, i.e. "this is a traditional event").
        for name in ["MLB", "NHL", "NBA", "NFL", "Premier League", "World Cup"] {
            assert!(standings_for_event_name(name).is_some(), "{name}");
        }
        // An esports/unknown event falls through to its tournament standings.
        assert!(standings_for_event_name("LCK").is_none());
        assert!(standings_for_event_name("IEM Cologne").is_none());
    }

    #[test]
    fn team_standings_for_match_is_empty_for_esports_and_f1() {
        for g in [Sport::Cs2, Sport::Lol, Sport::Motorsport] {
            assert!(
                team_standings_for_match(g, "X", "a", "a", "b", "b").is_empty(),
                "{g:?}"
            );
        }
    }

    #[test]
    fn reminder_seed_into_reminder_carries_the_trusted_fields() {
        let seed = ReminderSeed {
            match_id: 42,
            sport: "nhl".into(),
            league: "NHL".into(),
            team_a: "Boston Bruins".into(),
            team_b: "Montreal Canadiens".into(),
            event: "NHL".into(),
            lead_ms: 900_000,
            notify_at_ms: 1000,
            title: "Bruins at Canadiens".into(),
            body: "NHL · 7:00 PM".into(),
            url: "https://x".into(),
        };
        let sub = crate::types::PushSub {
            endpoint: "https://push/1".into(),
            p256dh: "key".into(),
            auth: "auth".into(),
        };
        let r = seed.into_reminder(sub);
        // Delivery comes from the subscription...
        assert_eq!(r.endpoint, "https://push/1");
        assert_eq!(r.p256dh, "key");
        // ...everything else from the server-derived seed.
        assert_eq!(r.match_id, 42);
        assert_eq!(r.notify_at_ms, 1000);
        assert_eq!(r.title, "Bruins at Canadiens");
        assert_eq!(r.sport, "nhl");
        assert_eq!(r.team_a, "Boston Bruins");
        assert_eq!(r.event, "NHL");
    }

    #[test]
    fn reminder_seed_body_uses_the_given_timezone() {
        // A match at 02:00 UTC — 10:00 PM the previous day in New York (EDT, −4),
        // 7:00 PM in Los Angeles (PDT, −7). The body's start time (and zone tag)
        // must follow whichever tz the seed is built with, not a server default.
        let m = at(
            "2026-06-25T02:00:00Z".parse::<DateTime<Utc>>().unwrap(),
            MatchStatus::Upcoming,
        );
        let lead = 15 * 60_000;

        let ny = reminder_seed(&m, lead, &chrono_tz::America::New_York, false);
        assert!(ny.body.contains("10:00 PM"), "NY body: {}", ny.body);
        assert!(
            ny.body.contains("EDT"),
            "NY body tags the zone: {}",
            ny.body
        );

        let la = reminder_seed(&m, lead, &chrono_tz::America::Los_Angeles, false);
        assert!(la.body.contains("7:00 PM"), "LA body: {}", la.body);
        assert!(
            la.body.contains("PDT"),
            "LA body tags the zone: {}",
            la.body
        );

        // The 24h preference formats the same instant as "22:00", not "10:00 PM".
        let ny24 = reminder_seed(&m, lead, &chrono_tz::America::New_York, true);
        assert!(ny24.body.contains("22:00"), "NY 24h body: {}", ny24.body);
        assert!(
            !ny24.body.contains("PM"),
            "NY 24h body has no AM/PM: {}",
            ny24.body
        );

        // The fire time is the same UTC instant regardless of display tz/format.
        assert_eq!(ny.notify_at_ms, la.notify_at_ms);
        assert_eq!(ny.notify_at_ms, m.begin_at.timestamp_millis() - lead);
    }

    #[test]
    fn seed_is_armable_only_when_notify_is_still_ahead() {
        // A match an hour out, seeded for two timers: a 15-min lead (notify still
        // ahead) and a 1-day lead (notify long past). Only the future one arms —
        // otherwise the past one would fire retroactively (the first-start burst).
        let begin = Utc::now() + Duration::hours(1);
        let m = at(begin, MatchStatus::Upcoming);
        let now = Utc::now().timestamp_millis();
        let soon = reminder_seed(&m, 15 * 60_000, &chrono_tz::Etc::UTC, false);
        let far = reminder_seed(&m, 24 * 60 * 60_000, &chrono_tz::Etc::UTC, false);
        assert!(soon.is_armable(now), "the 15-min timer is still ahead");
        assert!(
            !far.is_armable(now),
            "the 1-day timer's window already opened"
        );
    }

    #[test]
    fn mode_filter_splits_esports_and_traditional() {
        // "trad" = every traditional sport, no specific sport (the homepage's
        // sports mode; the client narrows it with the sport tabs).
        assert_eq!(mode_filter("trad"), (true, None));
        // A specific traditional sport.
        assert_eq!(mode_filter("mlb"), (true, Some(Sport::Mlb)));
        assert_eq!(mode_filter("football"), (true, Some(Sport::Soccer)));
        // A specific esports title.
        assert_eq!(mode_filter("cs2"), (false, Some(Sport::Cs2)));
        // "all"/unknown = all esports (the homepage's default mode).
        assert_eq!(mode_filter("all"), (false, None));
        assert_eq!(mode_filter(""), (false, None));
        // Regression: the legacy "soccer" slug no longer resolves, so it's treated
        // as unknown (all esports) rather than slipping in as a traditional sport.
        assert_eq!(mode_filter("soccer"), (false, None));
    }

    #[test]
    fn to_view_formats_venue_label_with_date_and_tz() {
        let la = "America/Los_Angeles".parse::<Tz>().unwrap();
        let when = "2026-06-24T23:10:00Z".parse::<DateTime<Utc>>().unwrap();

        // A venue tz (MLB/NHL/F1) yields the same instant at the venue, with its
        // date + tz abbreviation, while the row clock stays in the viewer's tz.
        let mut m = at(when, MatchStatus::Upcoming);
        m.sport = Sport::Mlb;
        m.venue_tz = Some("America/New_York".into());
        let view = to_view(&m, &la, when, false);
        assert_eq!(view.clock_label, "\u{2007}4:10 PM"); // viewer tz (PDT), padded
        assert!(
            view.venue_label.ends_with("7:10 PM EDT"),
            "{}",
            view.venue_label
        );
        assert!(view.venue_label.contains("Jun 24"), "{}", view.venue_label);

        // No venue tz (esports) → no venue label, nothing to swap to.
        let mut e = at(when, MatchStatus::Upcoming);
        e.venue_tz = None;
        assert!(to_view(&e, &la, when, false).venue_label.is_empty());
    }

    #[test]
    fn to_view_blanks_the_clock_for_date_only_wrc() {
        // WRC rallies are date-only, anchored at noon UTC. Even for a viewer well
        // west of UTC the day must read as the rally's date (not slip a day back),
        // and the clock must be blank rather than a fabricated time.
        let la = "America/Los_Angeles".parse::<Tz>().unwrap();
        let noon = "2026-01-22T12:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let mut m = at(noon, MatchStatus::Upcoming);
        m.sport = Sport::Motorsport;
        m.league = "WRC".into();
        let view = to_view(&m, &la, noon, false);
        assert_eq!(view.clock_label, "");
        assert!(view.venue_label.is_empty());
        assert_eq!(view.date_label, "Thursday, January 22");
    }

    #[test]
    fn to_view_wec_placeholder_is_date_only_but_a_session_keeps_its_clock() {
        let la = "America/Los_Angeles".parse::<Tz>().unwrap();
        let now = "2026-07-12T12:00:00Z".parse::<DateTime<Utc>>().unwrap();

        // A WEC round with no published schedule: a noon-anchored placeholder with
        // no venue tz → blank clock, no toggle (like WRC).
        let mut ph = at(now, MatchStatus::Upcoming);
        ph.sport = Sport::Motorsport;
        ph.league = "WEC".into();
        ph.venue_tz = None;
        let v = to_view(&ph, &la, now, false);
        assert_eq!(v.clock_label, "");
        assert!(v.venue_label.is_empty());

        // A real session (has a venue tz) keeps its clock and venue-time toggle.
        let mut sess = at(
            "2026-07-12T18:30:00Z".parse().unwrap(),
            MatchStatus::Upcoming,
        );
        sess.sport = Sport::Motorsport;
        sess.league = "WEC".into();
        sess.venue_tz = Some("America/Sao_Paulo".into());
        let vs = to_view(&sess, &la, now, false);
        assert!(!vs.clock_label.is_empty());
        assert!(!vs.venue_label.is_empty());
    }

    #[test]
    fn motor_plan_wrc_overall_then_power_when_finished() {
        use crate::types::MotorResultRef;
        let now = Utc::now();
        let stage = |label: &str, sess: &str, status: MatchStatus, begin: DateTime<Utc>| {
            let mut m = at(begin, status);
            m.sport = Sport::Motorsport;
            m.league = "WRC".into();
            m.team_a.label = label.into();
            m.motor_result_ref = Some(MotorResultRef {
                series: "WRC".into(),
                event_id: "rally-1".into(),
                session_id: sess.into(),
            });
            m
        };
        let rows = vec![
            stage(
                "SS1 Kuragaike",
                "s1",
                MatchStatus::Finished,
                now - Duration::hours(3),
            ),
            stage(
                "SS19 Loutraki Power Stage",
                "ps",
                MatchStatus::Finished,
                now - Duration::hours(1),
            ),
        ];
        let plan = motor_result_plan(&rows);
        // Overall (synthesized, empty session) first, then the Power Stage row.
        assert_eq!(plan.len(), 2);
        assert_eq!(plan[0].0, "Overall classification");
        assert_eq!(plan[0].1.event_id, "rally-1");
        assert_eq!(plan[0].1.session_id, "");
        assert!(plan[1].0.contains("Power Stage"));
        assert_eq!(plan[1].1.session_id, "ps");
    }

    #[test]
    fn motor_plan_wrc_empty_until_rally_finishes() {
        use crate::types::MotorResultRef;
        let now = Utc::now();
        let mk = |label: &str, sess: &str, status: MatchStatus, begin: DateTime<Utc>| {
            let mut m = at(begin, status);
            m.sport = Sport::Motorsport;
            m.league = "WRC".into();
            m.team_a.label = label.into();
            m.motor_result_ref = Some(MotorResultRef {
                series: "WRC".into(),
                event_id: "r".into(),
                session_id: sess.into(),
            });
            m
        };
        // The latest-starting row (the power stage) is still upcoming → nothing yet.
        let rows = vec![
            mk("SS1", "s1", MatchStatus::Finished, now - Duration::hours(1)),
            mk(
                "SS19 Power Stage",
                "ps",
                MatchStatus::Upcoming,
                now + Duration::hours(2),
            ),
        ];
        assert!(motor_result_plan(&rows).is_empty());
    }

    #[test]
    fn motor_plan_sessions_in_start_order_finished_only() {
        use crate::types::MotorResultRef;
        let now = Utc::now();
        let sess = |label: &str, id: &str, status: MatchStatus, begin: DateTime<Utc>| {
            let mut m = at(begin, status);
            m.sport = Sport::Motorsport;
            m.league = "WEC".into();
            m.team_a.label = label.into();
            m.motor_result_ref = Some(MotorResultRef {
                series: "WEC".into(),
                event_id: "lm".into(),
                session_id: id.into(),
            });
            m
        };
        let rows = vec![
            sess("Race", "race", MatchStatus::Finished, now),
            sess(
                "Qualifying HYPERCAR",
                "q",
                MatchStatus::Finished,
                now - Duration::hours(5),
            ),
            sess(
                "Warm Up",
                "wu",
                MatchStatus::Upcoming,
                now + Duration::hours(1),
            ),
        ];
        let plan = motor_result_plan(&rows);
        // Finished only, sorted by start: quali (earlier) before race; warm-up dropped.
        assert_eq!(plan.len(), 2);
        assert_eq!(plan[0].0, "Qualifying HYPERCAR");
        assert_eq!(plan[1].0, "Race");
    }

    #[test]
    fn motor_plan_empty_for_f1() {
        let now = Utc::now();
        let mut m = at(now, MatchStatus::Finished);
        m.sport = Sport::Motorsport;
        m.league = "F1".into(); // F1 carries no motor_result_ref
        assert!(motor_result_plan(&[m]).is_empty());
    }

    #[test]
    fn motor_cache_negative_ttl_refetches_errors_sooner_than_empties() {
        use crate::types::MotorResultRow;
        let now = Utc::now();
        let cached = |rows: Vec<MotorResultRow>, errored: bool, age_min: i64| CachedMotorResult {
            rows,
            fetched_at: now - Duration::minutes(age_min),
            errored,
        };
        let row = MotorResultRow::default();
        // A non-empty (finished, immutable) result is fresh forever.
        assert!(motor_cache_fresh(&cached(vec![row], false, 10_000), now));
        // A failed fetch is negative-cached: fresh briefly, then re-checked once
        // the short error TTL lapses (so a slow-failing endpoint isn't re-hit on
        // every view, but recovers quickly).
        assert!(motor_cache_fresh(
            &cached(vec![], true, MOTOR_RESULT_ERR_TTL_MIN - 1),
            now
        ));
        assert!(!motor_cache_fresh(
            &cached(vec![], true, MOTOR_RESULT_ERR_TTL_MIN + 1),
            now
        ));
        // An empty *success* (results not posted yet) stays fresh longer than an
        // error — past the error TTL, up to the longer empty TTL.
        assert!(motor_cache_fresh(
            &cached(vec![], false, MOTOR_RESULT_ERR_TTL_MIN + 1),
            now
        ));
        assert!(!motor_cache_fresh(
            &cached(vec![], false, MOTOR_RESULT_TTL_MIN + 1),
            now
        ));
    }

    #[test]
    fn homepage_rows_windows_and_includes_live() {
        let now = "2026-06-30T18:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let tz: Tz = "America/New_York".parse().unwrap();
        let cfg = config();

        // A live match (in window), a far-future match (past the esports horizon),
        // and a canceled one (always dropped).
        let mut live = demo_match(
            1,
            Sport::Cs2,
            "IEM",
            "s",
            now,
            MatchStatus::Live,
            3,
            demo_team("A", Some(1)),
            demo_team("B", Some(0)),
        );
        live.series_name = "Katowice 2026".to_string();
        let far = demo_match(
            2,
            Sport::Cs2,
            "IEM",
            "s",
            now + Duration::days(400),
            MatchStatus::Upcoming,
            3,
            demo_team("C", None),
            demo_team("D", None),
        );
        let mut canceled = demo_match(
            3,
            Sport::Cs2,
            "IEM",
            "s",
            now,
            MatchStatus::Canceled,
            3,
            demo_team("E", None),
            demo_team("F", None),
        );
        canceled.series_name = "Katowice 2026".to_string();

        let snap = snapshot_from(vec![live, far, canceled], now);
        let (rows, meta) = homepage_rows(&snap, cfg, now, "all", &tz, false);
        let ids: Vec<i64> = rows.iter().map(|m| m.id).collect();
        assert!(ids.contains(&1), "live match should be in window");
        assert!(!ids.contains(&2), "match past horizon should be excluded");
        assert!(!ids.contains(&3), "canceled match should be dropped");
        assert!(!meta.stale);
    }

    #[test]
    fn homepage_rows_trad_surfaces_far_future_motorsport_via_next_event_append() {
        // An F1 session 30 days out is well past TRAD_FORWARD_DAYS (2) and so
        // `matches_in_window` drops it. The traditional branch must still surface
        // it via the `next_motorsport_events` append loop — this test proves that
        // branch runs and produces the expected row.
        let now = "2026-06-30T18:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let tz: Tz = "America/New_York".parse().unwrap();
        let cfg = config();

        // F1 session far beyond the short traditional window.
        let mut f1 = demo_match(
            100,
            Sport::Motorsport,
            "F1",
            "s",
            now + Duration::days(30),
            MatchStatus::Upcoming,
            1,
            demo_team("Race", None),
            demo_team("", None),
        );
        f1.series_name = "British Grand Prix 2026".to_string();

        // An in-window MLB game so the snapshot has traditional content and the
        // dedup path is exercised.
        let mlb = demo_match(
            101,
            Sport::Mlb,
            "MLB",
            "s",
            now + Duration::hours(2),
            MatchStatus::Upcoming,
            1,
            demo_team("NYY", None),
            demo_team("BOS", None),
        );

        let snap = snapshot_from(vec![f1, mlb], now);
        let (rows, _meta) = homepage_rows(&snap, cfg, now, "trad", &tz, false);
        let ids: Vec<i64> = rows.iter().map(|m| m.id).collect();

        assert!(ids.contains(&101), "in-window MLB game should be present");
        // The far-future F1 session must appear via the motorsport-append branch.
        assert!(
            ids.contains(&100),
            "far-future F1 session should be surfaced by next_motorsport_events append; ids={ids:?}"
        );
    }

    #[test]
    fn day_labels_match_strftime_over_a_year() {
        let tz: Tz = "America/New_York".parse().unwrap();
        // 2026-06-24 is a Wednesday (sanity anchor).
        let anchor = "2026-06-24T16:00:00Z"
            .parse::<DateTime<Utc>>()
            .unwrap()
            .with_timezone(&tz);
        assert_eq!(day_label(anchor), "Wednesday, June 24");
        assert_eq!(short_day_label(anchor), "Wed, Jun 24");
        // The static-table lookups must stay byte-identical to chrono's strftime
        // across every weekday and month.
        let base = "2026-01-01T12:00:00Z".parse::<DateTime<Utc>>().unwrap();
        for d in 0..400 {
            let local = (base + Duration::days(d)).with_timezone(&tz);
            assert_eq!(
                day_label(local),
                format!(
                    "{}, {} {}",
                    local.format("%A"),
                    local.format("%B"),
                    local.day()
                ),
            );
            assert_eq!(
                short_day_label(local),
                format!(
                    "{}, {} {}",
                    local.format("%a"),
                    local.format("%b"),
                    local.day()
                ),
            );
        }
    }

    #[test]
    fn homepage_render_groups_synthetic_snapshot_in_day_order() {
        let now = "2026-06-30T18:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let tz: Tz = "America/New_York".parse().unwrap();
        let cfg = config();
        let snap = synthetic_snapshot(300, now);

        let view = homepage_render(&snap, cfg, now, "all", &tz, false);
        assert!(!view.days.is_empty(), "expected grouped days");
        // Day groups must be in ascending day order.
        let keys: Vec<&str> = view.days.iter().map(|d| d.day_key.as_str()).collect();
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        assert_eq!(keys, sorted, "day groups should be time-sorted");
    }

    #[test]
    fn enrich_streams_marks_live_and_appends_costreamers() {
        use crate::twitch::LiveInfo;
        let base = vec![
            StreamView {
                url: "https://www.twitch.tv/esl_csgo".into(),
                official: true,
                group: "official".into(),
                ..Default::default()
            },
            StreamView {
                url: "https://www.twitch.tv/offline_chan".into(),
                group: "official".into(),
                ..Default::default()
            },
        ];
        let live: std::collections::HashMap<String, LiveInfo> = [
            (
                "esl_csgo".to_string(),
                LiveInfo {
                    viewers: 5000,
                    game: "Counter-Strike".into(),
                    language: "en".into(),
                },
            ),
            (
                "caedrel".to_string(),
                LiveInfo {
                    viewers: 20000,
                    game: "Counter-Strike 2".into(),
                    language: "en".into(),
                },
            ),
            (
                "gaules".to_string(),
                LiveInfo {
                    viewers: 375,
                    game: "Back 4 Blood".into(),
                    language: "pt".into(),
                },
            ),
            (
                "esl_csgo".to_string(),
                LiveInfo {
                    viewers: 5000,
                    game: "Counter-Strike".into(),
                    language: "en".into(),
                },
            ),
        ]
        .into_iter()
        .collect();
        let seeds = vec![
            "caedrel".to_string(),
            "gaules".to_string(),
            "esl_csgo".to_string(),
        ];
        let out = enrich_streams(base, &live, &seeds, "counter-strike");

        // Base official stream marked live with viewers.
        assert!(out[0].live && out[0].viewers == Some(5000));
        // Offline base stream untouched.
        assert!(!out[1].live && out[1].viewers.is_none());
        // caedrel (live, matching game) appended as a costream.
        let caedrel = out
            .iter()
            .find(|s| s.url.contains("caedrel"))
            .expect("caedrel appended");
        assert!(
            caedrel.live
                && caedrel.viewers == Some(20000)
                && caedrel.group == "costream"
                && !caedrel.official
        );
        assert_eq!(caedrel.language, "en"); // Helix stream language carried onto the co-stream
                                            // gaules live but on the wrong game → NOT appended.
        assert!(out.iter().all(|s| !s.url.contains("gaules")));
        // esl_csgo is already a base stream → not double-appended as a costream.
        assert_eq!(out.iter().filter(|s| s.url.contains("esl_csgo")).count(), 1);
    }

    #[test]
    fn enrich_streams_empty_needle_appends_no_costreamers() {
        use crate::twitch::LiveInfo;
        let live: std::collections::HashMap<String, LiveInfo> = [(
            "x".to_string(),
            LiveInfo {
                viewers: 1,
                game: "whatever".into(),
                language: "en".into(),
            },
        )]
        .into_iter()
        .collect();
        let out = enrich_streams(Vec::new(), &live, &["x".to_string()], "");
        assert!(out.is_empty());
    }

    #[test]
    fn enrich_soop_marks_live_and_appends_live_seeds() {
        let base = vec![
            StreamView {
                url: "https://www.sooplive.com/lck".into(),
                language: "ko".into(),
                official: true,
                main: true,
                curated: true,
                ..Default::default()
            },
            // A non-SOOP stream must be left untouched.
            StreamView {
                url: "https://www.twitch.tv/riotgames".into(),
                official: true,
                ..Default::default()
            },
        ];
        let live: std::collections::HashMap<String, crate::soop::LiveInfo> = [
            ("lck".to_string(), crate::soop::LiveInfo { viewers: 5000 }),
            (
                "faker".to_string(),
                crate::soop::LiveInfo { viewers: 12000 },
            ),
        ]
        .into_iter()
        .collect();
        // faker is live and appended; offline_bj is not in `live` so it's skipped.
        let seeds = vec!["faker".to_string(), "offline_bj".to_string()];
        let out = enrich_soop(base, &live, &seeds);

        // Base SOOP stream marked live with viewers.
        let lck = out
            .iter()
            .find(|s| s.url.ends_with("/lck"))
            .expect("lck present");
        assert!(lck.live && lck.viewers == Some(5000));
        // Non-SOOP stream untouched.
        let tw = out.iter().find(|s| s.url.contains("twitch")).unwrap();
        assert!(!tw.live && tw.viewers.is_none());
        // faker appended as a Korean costream.
        let faker = out
            .iter()
            .find(|s| s.url.ends_with("/faker"))
            .expect("faker appended");
        assert!(
            faker.live
                && faker.viewers == Some(12000)
                && faker.group == "costream"
                && faker.language == "ko"
                && !faker.official
        );
        // Offline seed not appended.
        assert!(out.iter().all(|s| !s.url.contains("offline_bj")));
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn enrich_soop_skips_seed_already_present_as_base() {
        let base = vec![StreamView {
            url: "https://play.sooplive.com/lck".into(),
            official: true,
            ..Default::default()
        }];
        let live: std::collections::HashMap<String, crate::soop::LiveInfo> =
            [("lck".to_string(), crate::soop::LiveInfo { viewers: 5000 })]
                .into_iter()
                .collect();
        // "lck" is already the base channel → must not be double-appended.
        let out = enrich_soop(base, &live, &["lck".to_string()]);
        assert_eq!(out.iter().filter(|s| s.url.contains("lck")).count(), 1);
        assert!(out[0].live && out[0].viewers == Some(5000));
    }

    #[test]
    fn league_keywords_alias_plus_raw_token() {
        let mut al = std::collections::HashMap::new();
        al.insert(
            "Mid-Season Invitational".to_string(),
            vec!["MSI".to_string()],
        );
        let k = league_keywords("Mid-Season Invitational", &al);
        assert!(k.contains(&"msi".to_string()));
        assert!(k.contains(&"mid-season invitational".to_string()));
        // No alias → raw league token only (lowercased).
        assert_eq!(
            league_keywords("LEC", &std::collections::HashMap::new()),
            vec!["lec".to_string()]
        );
        // Empty league → no keywords.
        assert!(league_keywords("", &std::collections::HashMap::new()).is_empty());
    }

    #[test]
    fn event_day_keywords_widens_to_the_same_event_slate() {
        let base = "2026-07-04T06:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let team = |label: &str, name: &str, abbrev: &str| NormalizedTeam {
            label: label.into(),
            name: name.into(),
            abbrev: abbrev.into(),
            score: None,
        };
        let mk = |id: i64,
                  series: &str,
                  sport: Sport,
                  hrs: i64,
                  a: NormalizedTeam,
                  b: NormalizedTeam| {
            let mut m = NormalizedMatch::team_sport(
                id,
                sport,
                "Mid-Season Invitational",
                base + Duration::hours(hrs),
                MatchStatus::Upcoming,
                a,
                b,
            );
            m.series_name = series.into();
            m
        };
        let matches = vec![
            // The match being viewed.
            mk(
                1,
                "2026",
                Sport::Lol,
                0,
                team("BLG", "Bilibili Gaming", "BLG"),
                team("T1", "T1", "T1"),
            ),
            // Same event, +3h → its teams are folded in.
            mk(
                2,
                "2026",
                Sport::Lol,
                3,
                team("G2", "G2 Esports", "G2"),
                team("TES", "Top Esports", "TES"),
            ),
            // Same event but 20h away → out of the day window.
            mk(
                3,
                "2026",
                Sport::Lol,
                20,
                team("HLE", "Hanwha Life", "HLE"),
                team("GEN", "Gen.G", "GEN"),
            ),
            // Different edition (series) → excluded.
            mk(
                4,
                "2025",
                Sport::Lol,
                1,
                team("FNC", "Fnatic", "FNC"),
                team("MAD", "Mad Lions", "MAD"),
            ),
            // Different sport → excluded.
            mk(
                5,
                "2026",
                Sport::Cs2,
                1,
                team("NAVI", "Natus Vincere", "NAVI"),
                team("FAZE", "FaZe", "FAZE"),
            ),
        ];
        let aliases: std::collections::HashMap<String, Vec<String>> = [(
            "Mid-Season Invitational".to_string(),
            vec!["MSI".to_string()],
        )]
        .into_iter()
        .collect();
        let kw = event_day_keywords(
            Sport::Lol,
            "Mid-Season Invitational",
            "2026",
            base,
            &matches,
            &aliases,
            Duration::hours(12),
        );
        // League keywords, plus the current + within-window teams (names + acronyms).
        for t in [
            "msi",
            "mid-season invitational",
            "bilibili gaming",
            "blg",
            "t1",
            "g2 esports",
            "g2",
            "top esports",
            "tes",
        ] {
            assert!(kw.contains(&t.to_string()), "missing keyword {t}");
        }
        // Out-of-window, other edition, and other sport are all excluded.
        for t in [
            "hanwha life",
            "gen.g",
            "fnatic",
            "mad lions",
            "natus vincere",
            "faze",
        ] {
            assert!(!kw.contains(&t.to_string()), "should not include {t}");
        }
    }

    #[test]
    fn attribute_costreams_filters_dedups_and_marks_costream() {
        use crate::twitch_discover::DiscoveredStream;
        let ds = |login: &str, v: u64, title: &str, tags: &[&str]| DiscoveredStream {
            login: login.to_string(),
            viewers: v,
            title: title.to_string(),
            language: "en".to_string(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
        };
        let scan = vec![
            ds("caedrel", 40000, "MSI Watchparty Day 3", &[]), // title hit
            ds("tagonly", 5000, "just chatting", &["MSI"]),    // tag hit
            ds("tiny", 100, "MSI co-stream", &[]),             // below floor
            ds("unrelated", 40000, "ranked grind", &[]),       // no keyword
            ds("official", 40000, "MSI main", &[]),            // already present
        ];
        let keywords = vec!["msi".to_string()];
        let mut present = std::collections::HashSet::new();
        present.insert("official".to_string());
        let out = attribute_costreams(&scan, &keywords, &present, 300, 6);
        let logins: Vec<String> = out.iter().filter_map(|s| login_of(&s.url)).collect();
        assert!(logins.contains(&"caedrel".to_string()));
        assert!(logins.contains(&"tagonly".to_string()));
        assert!(!logins.contains(&"tiny".to_string())); // viewer floor
        assert!(!logins.contains(&"unrelated".to_string())); // no keyword
        assert!(!logins.contains(&"official".to_string())); // dedup
        assert!(out
            .iter()
            .all(|s| s.group == "costream" && s.live && !s.official));
        let caedrel = out
            .iter()
            .find(|s| login_of(&s.url).as_deref() == Some("caedrel"))
            .unwrap();
        assert_eq!(caedrel.viewers, Some(40000));
        assert_eq!(caedrel.language, "en"); // discovered stream language carried onto the co-stream
    }

    #[test]
    fn attribute_costreams_token_boundary_and_cap_ordering() {
        use crate::twitch_discover::DiscoveredStream;
        let ds = |login: &str, v: u64, title: &str| DiscoveredStream {
            login: login.to_string(),
            viewers: v,
            title: title.to_string(),
            language: "en".to_string(),
            tags: vec![],
        };
        let scan = vec![
            ds("nope", 90000, "rare card collector stream"), // "lec" only inside "collector"
            ds("a", 100000, "LEC Summer finals"),
            ds("b", 200000, "LEC watch party"),
            ds("c", 50000, "LEC co-stream"),
        ];
        let out = attribute_costreams(
            &scan,
            &["lec".to_string()],
            &std::collections::HashSet::new(),
            300,
            2,
        );
        let logins: Vec<String> = out.iter().filter_map(|s| login_of(&s.url)).collect();
        assert!(!logins.iter().any(|l| l == "nope")); // token boundary, not substring
        assert_eq!(logins, vec!["b".to_string(), "a".to_string()]); // top-2 by viewers desc
    }

    #[test]
    fn dedupe_youtube_collapses_same_channel_and_keeps_permalink() {
        let streams = vec![
            // Same broadcast, two URL forms (per-video + channel permalink).
            StreamView {
                url: "https://www.youtube.com/watch?v=vid1".into(),
                language: "en".into(),
                group: "costream".into(),
                ..Default::default()
            },
            StreamView {
                url: "https://www.youtube.com/lolesports/live".into(),
                group: "costream".into(),
                ..Default::default()
            },
            // A different channel (Spanish feed) — must survive.
            StreamView {
                url: "https://www.youtube.com/@latam/live".into(),
                language: "es".into(),
                group: "costream".into(),
                ..Default::default()
            },
            // Non-YouTube — untouched.
            StreamView {
                url: "https://www.twitch.tv/riotgames".into(),
                official: true,
                group: "official".into(),
                ..Default::default()
            },
        ];
        let channel_of: HashMap<String, String> = [
            ("https://www.youtube.com/watch?v=vid1", "UCmain"),
            ("https://www.youtube.com/lolesports/live", "UCmain"),
            ("https://www.youtube.com/@latam/live", "UClatam"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let out = dedupe_youtube_streams(streams, &channel_of);
        let urls: Vec<&str> = out.iter().map(|s| s.url.as_str()).collect();
        assert_eq!(
            urls,
            vec![
                "https://www.youtube.com/lolesports/live", // permalink kept over /watch
                "https://www.youtube.com/@latam/live",     // distinct channel survives
                "https://www.twitch.tv/riotgames",         // non-youtube untouched
            ]
        );
        assert_eq!(out[0].language, "en"); // carried from the /watch duplicate
    }

    #[test]
    fn mark_and_append_youtube_marks_base_and_appends_live_seeds() {
        use crate::youtube::LiveInfo;
        let streams = vec![StreamView {
            url: "https://www.youtube.com/lolesports/live".into(),
            group: "costream".into(),
            ..Default::default()
        }];
        let channel_of: HashMap<String, String> =
            [("https://www.youtube.com/lolesports/live", "UCmain")]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
        let yt_live: HashMap<String, LiveInfo> = [
            (
                "UCmain".to_string(),
                LiveInfo {
                    viewers: Some(12000),
                },
            ),
            (
                "UCcaedrel".to_string(),
                LiveInfo {
                    viewers: Some(3700),
                },
            ),
            // UCoffline absent → its seed isn't appended.
        ]
        .into_iter()
        .collect();
        let seeds = vec![
            // Same channel as the base stream → must NOT be double-added.
            (
                "UCmain".to_string(),
                "https://www.youtube.com/@lolesports".to_string(),
            ),
            (
                "UCcaedrel".to_string(),
                "https://www.youtube.com/@caedrel".to_string(),
            ),
            (
                "UCoffline".to_string(),
                "https://www.youtube.com/@offline".to_string(),
            ),
        ];
        let out = mark_and_append_youtube(streams, &channel_of, &yt_live, &seeds);
        assert!(out[0].live && out[0].viewers == Some(12000)); // base marked live
        let urls: Vec<&str> = out.iter().map(|s| s.url.as_str()).collect();
        assert!(urls.contains(&"https://www.youtube.com/@caedrel")); // live seed appended
        assert!(!urls.iter().any(|u| u.contains("@offline"))); // offline seed skipped
        assert_eq!(out.len(), 2); // base + caedrel only (lolesports not double-added)
    }

    #[test]
    fn costreamer_seeds_merges_game_and_league_deduped() {
        let mut cs: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        cs.insert(
            "lol".to_string(),
            vec!["caedrel".to_string(), "sneaky".to_string()],
        );
        cs.insert(
            "LCK".to_string(),
            vec!["Sneaky".to_string(), "cvmax".to_string()],
        );
        // Game seeds ("lol") lead; league extras ("LCK") follow; "sneaky" (in both,
        // case-insensitively) is deduped so it's queried once.
        assert_eq!(
            costreamer_seeds(&cs, "LCK", Sport::Lol),
            vec![
                "caedrel".to_string(),
                "sneaky".to_string(),
                "cvmax".to_string()
            ]
        );
        // A LoL league with no league-specific entry still gets the game-wide list.
        assert_eq!(
            costreamer_seeds(&cs, "LEC", Sport::Lol),
            vec!["caedrel".to_string(), "sneaky".to_string()]
        );
        // A game/league with no entry → empty.
        assert!(costreamer_seeds(&cs, "IEM", Sport::Cs2).is_empty());
    }
}
