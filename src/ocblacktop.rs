//! WRC + WEC schedules and championship standings from the Orange Cat Blacktop
//! API (`api.ocblacktop.com`, `x-api-key` header). Both series live under
//! [`Sport::Motorsport`] (distinguished by `league` — "WRC"/"WEC", the series
//! chips), alongside F1. Single-entity like F1: a row's one label is the rally /
//! race meeting; there's no opponent.
//!
//! The free tier is 250 requests/day, so — unlike F1's keyless Jolpica — the
//! poller fetches these on a slow, gated cadence and serves everything from
//! cache; nothing here is fetched per page view. `?year=` returns a whole season
//! in one request, which carries both the calendar and (via status) results.
//!
//! Like F1, WEC weekends are broken out one row per session (practice /
//! qualifying / race), each at its real UTC start so the schedule can show the
//! venue-local time. A round whose schedule isn't published yet collapses to a
//! single date-only placeholder. WRC rallies stay one row per rally (the feed
//! gives only a date, no per-stage times).

use crate::pandascore::{NormTeam, NormalizedMatch};
use crate::types::{Sport, MatchStatus, MotorStandingRow, MotorStandingTable, MotorStandings};
use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};
use serde::Deserialize;

const BASE: &str = "https://api.ocblacktop.com/v1";
const USER_AGENT: &str =
    "plaintextesports/0.1 (https://github.com/ralphpotato/plaintextesports; ralphpotato@gmail.com)";
const API_KEY_HEADER: &str = "x-api-key";

// Distinct high id ranges per series so WRC/WEC/F1 ids never collide (they now
// share Sport::Motorsport, and match identity is (sport, id)). F1's are season-
// derived and stay below ~1e9; these sit far above.
const WRC_ID_BASE: i64 = 10_000_000_000;
const WEC_ID_BASE: i64 = 20_000_000_000;

/// A stable 0..1e9 hash of the API's UUID, so a rally/event keeps the same id
/// across fetches (FNV-1a — deterministic, unlike `DefaultHasher`'s seeded form).
fn stable_hash(s: &str) -> i64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    (h % 1_000_000_000) as i64
}

/// A national flag (flagcdn SVG) from the API's country code; empty when absent.
/// The feed is mostly ISO alpha-2, but occasionally alpha-3 (e.g. Japan as
/// "JPN"); flagcdn keys on alpha-2, so fold the known odd one.
fn flag(two_code: &str) -> String {
    let lower = two_code.to_ascii_lowercase();
    let code = if lower == "jpn" { "jp" } else { lower.as_str() };
    if code.is_empty() {
        String::new()
    } else {
        format!("https://flagcdn.com/{code}.svg")
    }
}

/// Map the API's status string to ours; for a vague "scheduled"/unknown, infer
/// from the event window so an in-progress or just-finished event reads right.
fn status_of(api: &str, begin: DateTime<Utc>, end: DateTime<Utc>, now: DateTime<Utc>) -> MatchStatus {
    match api.to_ascii_lowercase().as_str() {
        "completed" | "finished" | "final" => MatchStatus::Finished,
        "cancelled" | "canceled" | "postponed" => MatchStatus::Canceled,
        "live" | "running" | "ongoing" | "in_progress" | "in-progress" => MatchStatus::Live,
        _ if now > end => MatchStatus::Finished,
        _ if now >= begin => MatchStatus::Live,
        _ => MatchStatus::Upcoming,
    }
}

fn parse_date(d: &str) -> Option<DateTime<Utc>> {
    NaiveDate::parse_from_str(d.get(..10)?, "%Y-%m-%d")
        .ok()?
        .and_hms_opt(0, 0, 0)
        .map(|dt| dt.and_utc())
}

fn parse_dt(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Utc))
}

// ----- Shared location shape ------------------------------------------------

#[derive(Deserialize, Default)]
struct Country {
    #[serde(default)]
    name: String,
    #[serde(rename = "twoCode", default)]
    two_code: String,
}

#[derive(Deserialize, Default)]
struct Location {
    #[serde(default)]
    name: String,
    #[serde(default)]
    city: String,
    #[serde(default)]
    country: Country,
}

/// IANA timezone for a WEC round's venue. ocblacktop exposes no tz, so we map the
/// host country: the WEC calendar runs one venue per country, so the 2-letter ISO
/// code is enough — including the US (Austin / Central) and Brazil (São Paulo),
/// whose sole WEC venues fix the zone even though the country spans several.
/// `None` (no venue-time toggle) for an unmapped country, exactly like F1's
/// `circuit_tz`. Update alongside the calendar.
fn wec_venue_tz(country_code: &str) -> Option<&'static str> {
    Some(match country_code.to_ascii_uppercase().as_str() {
        "QA" => "Asia/Qatar",
        "IT" => "Europe/Rome",
        "BE" => "Europe/Brussels",
        "FR" => "Europe/Paris",
        "BR" => "America/Sao_Paulo",
        "US" => "America/Chicago",
        // The feed reports Japan as the alpha-3 "JPN", not "JP" — accept both.
        "JP" | "JPN" => "Asia/Tokyo",
        "BH" => "Asia/Bahrain",
        _ => return None,
    })
}

impl Location {
    /// "City, Country" (or whichever is present).
    fn label(&self) -> String {
        match (self.city.is_empty(), self.country.name.is_empty()) {
            (false, false) => format!("{}, {}", self.city, self.country.name),
            (false, true) => self.city.clone(),
            (true, false) => self.country.name.clone(),
            _ => String::new(),
        }
    }
}

/// Build the single-entity match shared by WRC rallies and WEC sessions. `label`
/// is the row's one label (the rally name, or a WEC session like "Qualifying
/// HYPERCAR"); `event_name` is the meeting, qualified by season into `series_name`
/// so each year's edition groups on its own and stays distinct — exactly like
/// F1's "{GP} {year}". There's no opponent.
#[allow(clippy::too_many_arguments)]
fn motorsport_match(
    id: i64,
    league: &str,
    label: &str,
    event_name: &str,
    season: i32,
    begin_at: DateTime<Utc>,
    status: MatchStatus,
    loc: &Location,
) -> NormalizedMatch {
    let mut m = NormalizedMatch::team_sport(
        id,
        Sport::Motorsport,
        league,
        begin_at,
        status,
        NormTeam { label: label.to_string(), name: String::new(), abbrev: String::new(), score: None },
        NormTeam { label: String::new(), name: String::new(), abbrev: String::new(), score: None },
    );
    m.series_name = format!("{event_name} {season}");
    m.venue_name = loc.name.clone();
    m.venue_location = loc.label();
    m.team_a_logo = flag(&loc.country.two_code);
    m
}

// ----- WRC: /v1/wrc/rallies?year= -------------------------------------------

#[derive(Deserialize)]
struct RalliesResp {
    #[serde(default)]
    data: Vec<Rally>,
}

#[derive(Deserialize)]
struct Rally {
    id: String,
    name: String,
    #[serde(rename = "dateStart", default)]
    date_start: String,
    #[serde(rename = "dateEnd", default)]
    date_end: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    location: Location,
}

fn rally_to_match(r: &Rally, now: DateTime<Utc>) -> Option<NormalizedMatch> {
    // A rally carries only a calendar date (no session time). Anchor it at noon
    // UTC of the start day so converting to the viewer's zone lands on the right
    // date — midnight UTC would slip onto the previous day for anyone west of UTC.
    // The view layer shows no clock for it (there's no real start time).
    let begin = parse_date(&r.date_start)? + Duration::hours(12);
    // A rally runs to the end of dateEnd; default the window to the start day.
    let end = parse_date(&r.date_end).unwrap_or(begin) + Duration::days(1);
    let status = status_of(&r.status, begin, end, now);
    Some(motorsport_match(
        WRC_ID_BASE + stable_hash(&r.id),
        "WRC",
        &r.name,
        &r.name,
        begin.year(),
        begin,
        status,
        &r.location,
    ))
}

/// The WRC season calendar as single-entity rows (one per rally). One request.
pub async fn fetch_wrc(
    client: &reqwest::Client,
    key: &str,
    year: i32,
) -> Result<Vec<NormalizedMatch>, reqwest::Error> {
    let url = format!("{BASE}/wrc/rallies?year={year}&limit=100");
    let resp: RalliesResp = get(client, key, &url).await?;
    let now = Utc::now();
    Ok(resp.data.iter().filter_map(|r| rally_to_match(r, now)).collect())
}

// ----- WEC: /v1/wec/events?year= --------------------------------------------

#[derive(Deserialize)]
struct EventsResp {
    #[serde(default)]
    data: Vec<Event>,
}

#[derive(Deserialize)]
struct Event {
    id: String,
    name: String,
    #[serde(rename = "dateStart", default)]
    date_start: String,
    #[serde(rename = "dateEnd", default)]
    date_end: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    location: Location,
    #[serde(default)]
    schedule: Vec<Session>,
}

#[derive(Deserialize)]
struct Session {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    name: String,
    #[serde(rename = "startTime", default)]
    start_time: Option<String>,
    #[serde(default)]
    status: String,
}

/// The row label for a WEC session: its published name ("Free Practice 1",
/// "Hyperpole 2 HYPERCAR", "Race"), falling back to the title-cased type.
fn session_label(s: &Session) -> String {
    let name = s.name.trim();
    if !name.is_empty() {
        return name.to_string();
    }
    match s.kind.to_ascii_lowercase().as_str() {
        "practice" => "Practice",
        "qualifying" => "Qualifying",
        "race" => "Race",
        _ => "Session",
    }
    .to_string()
}

/// Stable, collision-resistant id for a WEC row. The race — and the date-only
/// placeholder that stands in for it before a schedule is published — take the
/// event's base id, so the placeholder upserts straight into the race row once
/// times appear (and old single-row caches migrate in place). Support sessions
/// are keyed by name, stable across fetches.
fn wec_match_id(event_id: &str, session: Option<&Session>) -> i64 {
    match session {
        Some(s) if !s.kind.eq_ignore_ascii_case("race") => {
            WEC_ID_BASE + stable_hash(&format!("{event_id}|{}", s.name))
        }
        _ => WEC_ID_BASE + stable_hash(event_id),
    }
}

/// How long after its start a session reads as live before flipping to finished,
/// used only when the feed's own status is vague. Races run long (6–24 h);
/// everything else is short.
fn session_window(kind: &str) -> Duration {
    if kind.eq_ignore_ascii_case("race") {
        Duration::hours(8)
    } else {
        Duration::hours(2)
    }
}

/// Every WEC session of an event as its own row (like an F1 weekend), each at its
/// real UTC start with the venue-local time available. An event whose schedule
/// isn't published yet has no timed sessions, so it yields a single date-only
/// placeholder (anchored at noon UTC, no clock), keyed to the eventual race row.
fn event_to_matches(e: &Event, now: DateTime<Utc>) -> Vec<NormalizedMatch> {
    let venue_tz = wec_venue_tz(&e.location.country.two_code);
    let mut out: Vec<NormalizedMatch> = e
        .schedule
        .iter()
        .filter_map(|s| {
            let begin = s.start_time.as_deref().and_then(parse_dt)?;
            let end = begin + session_window(&s.kind);
            let status = status_of(&s.status, begin, end, now);
            let mut m = motorsport_match(
                wec_match_id(&e.id, Some(s)),
                "WEC",
                &session_label(s),
                &e.name,
                begin.year(),
                begin,
                status,
                &e.location,
            );
            // A real session start → offer the venue-local time (like F1/MLB).
            m.venue_tz = venue_tz.map(str::to_string);
            Some(m)
        })
        .collect();
    if out.is_empty() {
        // No schedule yet: a date-only placeholder anchored at noon UTC (so the
        // viewer's day is right) for the race to come. No venue tz, so the view
        // shows no clock and no venue toggle until the real times land.
        if let Some(day) = parse_date(&e.date_start) {
            let begin = day + Duration::hours(12);
            let end = parse_date(&e.date_end)
                .map(|d| d + Duration::days(1))
                .unwrap_or(begin + Duration::hours(30));
            let status = status_of(&e.status, begin, end, now);
            out.push(motorsport_match(
                wec_match_id(&e.id, None),
                "WEC",
                "Race",
                &e.name,
                begin.year(),
                begin,
                status,
                &e.location,
            ));
        }
    }
    out
}

/// The WEC season as one row per session (placeholders for unpublished rounds).
/// One request.
pub async fn fetch_wec(
    client: &reqwest::Client,
    key: &str,
    year: i32,
) -> Result<Vec<NormalizedMatch>, reqwest::Error> {
    let url = format!("{BASE}/wec/events?year={year}&limit=100");
    let resp: EventsResp = get(client, key, &url).await?;
    let now = Utc::now();
    Ok(resp.data.iter().flat_map(|e| event_to_matches(e, now)).collect())
}

// ----- Standings ------------------------------------------------------------

/// A flat standings row, shared by WRC's tables and WEC's class tables.
#[derive(Deserialize)]
struct RawStandRow {
    #[serde(default)]
    position: i64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    country: Option<String>,
    #[serde(default)]
    points: f64,
}

fn fmt_points(p: f64) -> String {
    if (p.fract()).abs() < f64::EPSILON {
        format!("{}", p as i64)
    } else {
        format!("{p}")
    }
}

fn stand_rows(rows: &[RawStandRow]) -> Vec<MotorStandingRow> {
    rows.iter()
        .map(|r| MotorStandingRow {
            pos: r.position.to_string(),
            name: r.name.clone(),
            points: fmt_points(r.points),
            flag: flag(r.country.as_deref().unwrap_or_default()),
        })
        .collect()
}

/// WRC standings: three independent tables (drivers / co-drivers / manufacturers),
/// each a bare array. Three requests; any that fail are simply omitted.
pub async fn fetch_wrc_standings(client: &reqwest::Client, key: &str) -> MotorStandings {
    let mut tables = Vec::new();
    for (path, title) in [
        ("drivers", "Drivers"),
        ("co-drivers", "Co-drivers"),
        ("manufacturers", "Manufacturers"),
    ] {
        let url = format!("{BASE}/wrc/standings/{path}");
        if let Ok(rows) = get::<Vec<RawStandRow>>(client, key, &url).await {
            if !rows.is_empty() {
                tables.push(MotorStandingTable {
                    group: String::new(),
                    title: title.to_string(),
                    rows: stand_rows(&rows),
                });
            }
        }
    }
    MotorStandings { tables }
}

#[derive(Deserialize)]
struct WecStandTable {
    #[serde(rename = "classLabel", default)]
    class_label: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    rows: Vec<RawStandRow>,
}

fn kind_label(kind: &str) -> &str {
    match kind {
        "drivers" => "Drivers",
        "teams" => "Teams",
        "manufacturers" => "Manufacturers",
        other => other,
    }
}

/// WEC standings: one request returns every parallel-championship table (per
/// class × drivers/teams/manufacturers). The class is carried in `group` so each
/// class renders on its own row.
pub async fn fetch_wec_standings(client: &reqwest::Client, key: &str) -> MotorStandings {
    let url = format!("{BASE}/wec/standings");
    let tables = match get::<Vec<WecStandTable>>(client, key, &url).await {
        Ok(ts) => ts
            .into_iter()
            .filter(|t| !t.rows.is_empty())
            .map(|t| MotorStandingTable {
                group: t.class_label.clone(),
                title: kind_label(&t.kind).to_string(),
                rows: stand_rows(&t.rows),
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    MotorStandings { tables }
}

// ----- HTTP helper ----------------------------------------------------------

async fn get<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    key: &str,
    url: &str,
) -> Result<T, reqwest::Error> {
    client
        .get(url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .header(API_KEY_HEADER, key)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_wrc_rallies() {
        let json = r#"{"data":[
          {"id":"uuid-mc","name":"Rallye Monte-Carlo","round":1,"surface":"Tarmac",
           "dateStart":"2026-01-22","dateEnd":"2026-01-25","status":"scheduled",
           "location":{"name":"Monte Carlo","city":"Monaco","country":{"name":"Monaco","twoCode":"MC"}}},
          {"id":"uuid-py","name":"Rally del Paraguay","round":10,"surface":"Gravel",
           "dateStart":"2025-08-29","dateEnd":"2025-08-31","status":"completed",
           "location":{"name":"Encarnación","city":"Encarnación","country":{"name":"Paraguay","twoCode":"PY"}}}
        ]}"#;
        let resp: RalliesResp = serde_json::from_str(json).unwrap();
        // Before the 2026 season: Monte-Carlo (Jan) upcoming, Paraguay (Aug 2025) done.
        let now = "2026-01-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let ms: Vec<NormalizedMatch> = resp.data.iter().filter_map(|r| rally_to_match(r, now)).collect();
        assert_eq!(ms.len(), 2);
        let mc = &ms[0];
        assert_eq!(mc.sport, Sport::Motorsport);
        assert_eq!(mc.league, "WRC");
        assert_eq!(mc.team_a.label, "Rallye Monte-Carlo");
        assert_eq!(mc.series_name, "Rallye Monte-Carlo 2026");
        // Date-only: anchored at noon UTC so the viewer's day stays Jan 22, and
        // no venue tz (there's no session time to localise).
        assert_eq!(mc.begin_at.to_rfc3339(), "2026-01-22T12:00:00+00:00");
        assert_eq!(mc.venue_tz, None);
        assert_eq!(mc.status, MatchStatus::Upcoming);
        assert_eq!(mc.team_a_logo, "https://flagcdn.com/mc.svg");
        // The completed Paraguay rally is in the past → Finished, distinct id.
        assert_eq!(ms[1].status, MatchStatus::Finished);
        assert_ne!(mc.id, ms[1].id);
        assert!(mc.id >= WRC_ID_BASE && mc.id < WEC_ID_BASE);
    }

    #[test]
    fn wec_event_expands_each_session_into_its_own_row() {
        let json = r#"{"data":[
          {"id":"uuid-lm","name":"24 Hours of Le Mans","dateStart":"2025-06-08","dateEnd":"2025-06-14",
           "status":"completed",
           "location":{"name":"Circuit de la Sarthe","city":"Le Mans","country":{"name":"France","twoCode":"FR"}},
           "schedule":[
             {"type":"practice","name":"Free Practice 1","startTime":"2025-06-11T12:00:00.000Z","status":"completed"},
             {"type":"qualifying","name":"Qualifying HYPERCAR","startTime":"2025-06-11T17:30:00.000Z","status":"completed"},
             {"type":"race","name":"Race","startTime":"2025-06-14T14:00:00.000Z","status":"completed"}
           ]}
        ]}"#;
        let resp: EventsResp = serde_json::from_str(json).unwrap();
        let now = "2026-06-26T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let ms: Vec<NormalizedMatch> = resp.data.iter().flat_map(|e| event_to_matches(e, now)).collect();
        // One row per session, all under the same season-qualified series.
        assert_eq!(ms.len(), 3);
        assert!(ms.iter().all(|m| m.league == "WEC" && m.series_name == "24 Hours of Le Mans 2025"));
        // Each session is labelled by its published name and carries the venue
        // tz, so every row gets the venue-local toggle (FR → Paris).
        assert!(ms.iter().all(|m| m.venue_tz.as_deref() == Some("Europe/Paris")));
        let race = ms.iter().find(|m| m.team_a.label == "Race").unwrap();
        assert_eq!(race.begin_at.to_rfc3339(), "2025-06-14T14:00:00+00:00");
        assert_eq!(race.status, MatchStatus::Finished);
        // The race keeps the event's base id (so a placeholder upserts into it);
        // a support session is labelled by name and gets a distinct id.
        assert_eq!(race.id, WEC_ID_BASE + stable_hash("uuid-lm"));
        let quali = ms.iter().find(|m| m.team_a.label == "Qualifying HYPERCAR").unwrap();
        assert_ne!(quali.id, race.id);
        assert!(quali.id >= WEC_ID_BASE);
    }

    #[test]
    fn wec_event_without_schedule_yields_a_date_only_placeholder() {
        let json = r#"{"data":[
          {"id":"u","name":"8 Hours of Bahrain","dateStart":"2026-11-07","dateEnd":"2026-11-07",
           "status":"scheduled","location":{"name":"Bahrain","city":"Sakhir","country":{"name":"Bahrain","twoCode":"BH"}},
           "schedule":[]}
        ]}"#;
        let resp: EventsResp = serde_json::from_str(json).unwrap();
        let now = "2026-06-26T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let ms: Vec<NormalizedMatch> = resp.data.iter().flat_map(|e| event_to_matches(e, now)).collect();
        // A single placeholder, anchored at noon UTC (right day for any viewer),
        // labelled as the race-to-come, with no venue tz → the view shows no clock.
        assert_eq!(ms.len(), 1);
        let m = &ms[0];
        assert_eq!(m.begin_at.to_rfc3339(), "2026-11-07T12:00:00+00:00");
        assert_eq!(m.venue_tz, None);
        assert_eq!(m.team_a.label, "Race");
        // Same id the real race will take, so it upserts in place once published.
        assert_eq!(m.id, WEC_ID_BASE + stable_hash("u"));
        assert_eq!(m.status, MatchStatus::Upcoming);
    }

    #[test]
    fn wec_venue_tz_maps_hosts_including_multizone_countries() {
        // Single-zone hosts come straight from the country code.
        assert_eq!(wec_venue_tz("FR"), Some("Europe/Paris"));
        assert_eq!(wec_venue_tz("jp"), Some("Asia/Tokyo")); // case-insensitive
        // Multi-zone countries: the sole WEC venue pins the zone (COTA → Central,
        // Interlagos → São Paulo), not the country's "first" zone.
        assert_eq!(wec_venue_tz("US"), Some("America/Chicago"));
        assert_eq!(wec_venue_tz("BR"), Some("America/Sao_Paulo"));
        // The feed reports Japan as alpha-3 "JPN", not "JP" — both resolve.
        assert_eq!(wec_venue_tz("JPN"), Some("Asia/Tokyo"));
        assert_eq!(wec_venue_tz("JP"), Some("Asia/Tokyo"));
        // Anything off-calendar simply has no toggle.
        assert_eq!(wec_venue_tz("DE"), None);
        assert_eq!(wec_venue_tz(""), None);
    }

    #[test]
    fn flag_folds_alpha3_japan_to_alpha2() {
        // flagcdn keys on alpha-2, but the feed sends "JPN" for Japan.
        assert_eq!(flag("JPN"), "https://flagcdn.com/jp.svg");
        assert_eq!(flag("FR"), "https://flagcdn.com/fr.svg");
        assert_eq!(flag(""), "");
    }

    #[test]
    fn parses_wrc_drivers_standings_array() {
        let json = r#"[
          {"position":1,"name":"Elfyn EVANS","country":"GB","points":151},
          {"position":2,"name":"Sébastien OGIER","country":"FR","points":140}
        ]"#;
        let rows: Vec<RawStandRow> = serde_json::from_str(json).unwrap();
        let out = stand_rows(&rows);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].pos, "1");
        assert_eq!(out[0].name, "Elfyn EVANS");
        assert_eq!(out[0].points, "151");
        assert_eq!(out[0].flag, "https://flagcdn.com/gb.svg");
    }

    #[test]
    fn wec_standings_carry_class_in_group() {
        let json = r#"[
          {"code":"hypercar-drivers","name":"...","classLabel":"Hypercar","kind":"drivers",
           "rows":[{"position":1,"name":"Kamui Kobayashi · Mike Conway","country":null,"points":75}]},
          {"code":"lmgt3-teams","name":"...","classLabel":"LMGT3","kind":"teams","rows":[]}
        ]"#;
        let ts: Vec<WecStandTable> = serde_json::from_str(json).unwrap();
        // The class goes to `group` and the kind to `title`, so the view can keep
        // each class on its own row.
        assert_eq!(ts[0].class_label, "Hypercar");
        assert_eq!(kind_label(&ts[0].kind), "Drivers");
        assert_eq!(kind_label(&ts[1].kind), "Teams");
        // The drivers crew has a null country → no flag, no panic.
        let rows = stand_rows(&ts[0].rows);
        assert_eq!(rows[0].flag, "");
        assert_eq!(rows[0].points, "75");
    }
}
