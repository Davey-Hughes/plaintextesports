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

use crate::feed::{NormalizedMatch, NormalizedTeam};
use crate::http::USER_AGENT;
use crate::types::{MatchStatus, MotorStandingRow, MotorStandingTable, MotorStandings, Sport};
use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};
use serde::Deserialize;

const BASE: &str = "https://api.ocblacktop.com/v1";
const API_KEY_HEADER: &str = "x-api-key";

// Distinct high id ranges per series so WRC/WEC/F1 ids never collide (they now
// share Sport::Motorsport, and match identity is (sport, id)). F1's are season-
// derived and stay below ~1e9; these sit far above.
const WRC_ID_BASE: i64 = 10_000_000_000;
const WEC_ID_BASE: i64 = 20_000_000_000;
const MOTOGP_ID_BASE: i64 = 30_000_000_000;

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
fn status_of(
    api: &str,
    begin: DateTime<Utc>,
    end: DateTime<Utc>,
    now: DateTime<Utc>,
) -> MatchStatus {
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
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// Deserialize a field the feed may send as JSON `null` (a DNS/DNF row's
/// `stageTime`/`diffFirst`/`lapTime`, etc.) into `T::default()`. `#[serde(default)]`
/// alone only covers an *absent* field; a present `null` would otherwise error and
/// fail the whole array. Pair with `default` so absent fields still work too.
fn de_null_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + serde::Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// A finishing position, blanking the literal string "null" some DNS/DNF rows
/// carry so it doesn't render as a "null" position.
fn clean_pos(p: &str) -> String {
    if p == "null" {
        String::new()
    } else {
        p.to_string()
    }
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

/// IANA timezone for a WRC rally's host country, mirroring [`wec_venue_tz`]. The
/// calendar runs one venue per country, so the alpha-2 code fixes the zone — with
/// one catch: Spain's round is the Rally Islas Canarias, an hour behind the
/// mainland, so "ES" pins Atlantic/Canary, not Europe/Madrid. `None` for an
/// unmapped country → the stage shows no venue-local clock. Update with the
/// calendar.
fn wrc_venue_tz(country_code: &str) -> Option<&'static str> {
    Some(match country_code.to_ascii_uppercase().as_str() {
        "MC" => "Europe/Monaco",
        "SE" => "Europe/Stockholm",
        "KE" => "Africa/Nairobi",
        "HR" => "Europe/Zagreb",
        // Rally Islas Canarias runs on the Canary Islands (WET), an hour behind
        // mainland Spain — pin the island zone, not Europe/Madrid.
        "ES" => "Atlantic/Canary",
        "PT" => "Europe/Lisbon",
        // The feed reports Japan as alpha-3 "JPN", not "JP" — accept both.
        "JP" | "JPN" => "Asia/Tokyo",
        "GR" => "Europe/Athens",
        "EE" => "Europe/Tallinn",
        "FI" => "Europe/Helsinki",
        "PY" => "America/Asuncion",
        "CL" => "America/Santiago",
        "IT" => "Europe/Rome",
        "SA" => "Asia/Riyadh",
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
        NormalizedTeam {
            label: label.to_string(),
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
    let mut m = motorsport_match(
        WRC_ID_BASE + stable_hash(&r.id),
        "WRC",
        &r.name,
        &r.name,
        begin.year(),
        begin,
        status,
        &r.location,
    );
    // The rally row links to the event's overall classification (empty session).
    m.motor_result_ref = Some(crate::types::MotorResultRef {
        series: "WRC".to_string(),
        event_id: r.id.clone(),
        session_id: String::new(),
    });
    Some(m)
}

// ----- WRC rally detail: /v1/wrc/rallies/{id} (the per-stage timetable) -------

/// The detail endpoint returns the rally object directly (no `data` wrapper),
/// carrying a `schedule` the list endpoint omits. We only need the timetable; the
/// rally's name/location come from the matching list entry.
#[derive(Deserialize, Default)]
struct RallyDetail {
    #[serde(default)]
    schedule: RallySchedule,
}

/// A rally's timetable: an optional shakedown, the numbered special stages, and
/// the power stage broken out on its own (the points-paying TV finale).
#[derive(Deserialize, Default)]
struct RallySchedule {
    #[serde(default)]
    shakedown: Vec<Stage>,
    #[serde(rename = "specialStages", default)]
    special_stages: Vec<Stage>,
    #[serde(rename = "powerStage", default)]
    power_stage: Option<Stage>,
}

#[derive(Deserialize)]
struct Stage {
    id: String,
    #[serde(rename = "stageCode", default)]
    stage_code: String,
    #[serde(default)]
    name: String,
    #[serde(rename = "startTime", default)]
    start_time: Option<String>,
    #[serde(default)]
    status: String,
}

/// The row label for a stage: its published name ("SS4 Stiri 1", "SS17 … Power
/// Stage", "Shakedown"), falling back to the bare stage code.
fn stage_label(s: &Stage) -> String {
    let name = s.name.trim();
    if !name.is_empty() {
        name.to_string()
    } else {
        s.stage_code.clone()
    }
}

/// One single-entity row per timed stage of a rally, each at its real UTC start
/// with the host country's venue-local time available (like a WEC session). A
/// stage with no start time is skipped. The id hashes the stage's own UUID, so it
/// never collides with the rally's date-only placeholder (keyed off the rally id).
fn rally_stage_matches(
    r: &Rally,
    detail: &RallyDetail,
    now: DateTime<Utc>,
) -> Vec<NormalizedMatch> {
    let venue_tz = wrc_venue_tz(&r.location.country.two_code);
    let sch = &detail.schedule;
    sch.shakedown
        .iter()
        .chain(sch.special_stages.iter())
        .chain(sch.power_stage.iter())
        .filter_map(|s| {
            let begin = s.start_time.as_deref().and_then(parse_dt)?;
            // Stages are short (a closed road, run once); give a vague status a
            // one-hour live window before it reads as finished.
            let status = status_of(&s.status, begin, begin + Duration::hours(1), now);
            let mut m = motorsport_match(
                WRC_ID_BASE + stable_hash(&s.id),
                "WRC",
                &stage_label(s),
                &r.name,
                begin.year(),
                begin,
                status,
                &r.location,
            );
            m.venue_tz = venue_tz.map(str::to_string);
            // Each stage row links to that stage's own times.
            m.motor_result_ref = Some(crate::types::MotorResultRef {
                series: "WRC".to_string(),
                event_id: r.id.clone(),
                session_id: s.id.clone(),
            });
            Some(m)
        })
        .collect()
}

/// The rally whose stage timetable to fetch this cycle, in priority order: the
/// live one; else a just-finished one (within a few days of ending) so its final
/// per-stage results are captured and then kept after the rally — see
/// [`crate::cache::reconcile_wrc`]; else the soonest upcoming whose start is
/// within the window the detailed schedule is typically published (~2 weeks out).
/// `None` off-week, so no detail request is spent. Picking just one keeps WRC at
/// one extra request per cycle.
fn active_rally(rallies: &[Rally], now: DateTime<Utc>) -> Option<&Rally> {
    let publish_lead = Duration::days(14);
    // A finished rally's stages need fetching only once more to persist them;
    // stay eligible a few days so a cold start (or the idle poll cadence) still
    // catches it, then stop re-spending the request.
    let recent_window = Duration::days(3);
    let mut recent: Option<(&Rally, DateTime<Utc>)> = None; // most-recently ended
    let mut soonest: Option<(&Rally, DateTime<Utc>)> = None; // soonest to start
    for r in rallies {
        let Some(begin) = parse_date(&r.date_start).map(|d| d + Duration::hours(12)) else {
            continue;
        };
        let end = parse_date(&r.date_end).unwrap_or(begin) + Duration::days(1);
        match status_of(&r.status, begin, end, now) {
            MatchStatus::Live => return Some(r),
            MatchStatus::Finished
                if now - end <= recent_window && recent.is_none_or(|(_, e)| end > e) =>
            {
                recent = Some((r, end));
            }
            MatchStatus::Upcoming
                if begin - now <= publish_lead && soonest.is_none_or(|(_, b)| begin < b) =>
            {
                soonest = Some((r, begin));
            }
            _ => {}
        }
    }
    // A just-finished rally takes precedence over an imminent one: its results are
    // final and worth keeping, whereas the upcoming one is only a schedule pre-fetch.
    recent.or(soonest).map(|(r, _)| r)
}

/// The WRC season as single-entity rows: one date-only placeholder per rally, with
/// the current/imminent rally expanded into its real per-stage timetable in place
/// of its placeholder. Returns the rows and the number of requests spent (1 for
/// the calendar alone, 2 when an active rally's detail was also fetched), so the
/// poller can keep its daily-quota accounting exact.
pub async fn fetch_wrc(
    client: &reqwest::Client,
    key: &str,
    year: i32,
    now: DateTime<Utc>,
) -> Result<(Vec<NormalizedMatch>, u64), reqwest::Error> {
    let url = format!("{BASE}/wrc/rallies?year={year}&limit=100");
    let resp: RalliesResp = get(client, key, &url).await?;
    let mut rows: Vec<NormalizedMatch> = resp
        .data
        .iter()
        .filter_map(|r| rally_to_match(r, now))
        .collect();

    // Spend a second request on the active (live / just-finished / imminent)
    // rally's stage timetable, when there is one. Until its schedule is published
    // the detail carries no timed stages, so we simply keep the placeholder.
    let Some(active) = active_rally(&resp.data, now) else {
        return Ok((rows, 1));
    };
    let detail_url = format!("{BASE}/wrc/rallies/{}", active.id);
    let Ok(detail) = get::<RallyDetail>(client, key, &detail_url).await else {
        return Ok((rows, 2));
    };
    let stages = rally_stage_matches(active, &detail, now);
    if !stages.is_empty() {
        let placeholder_id = WRC_ID_BASE + stable_hash(&active.id);
        rows.retain(|m| m.id != placeholder_id);
        rows.extend(stages);
    }
    Ok((rows, 2))
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
    /// The session's UUID, used to fetch its results (MotoGP). Absent in older
    /// WEC payloads, which don't fetch per-session results.
    #[serde(default)]
    id: String,
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

/// Stable, collision-resistant id for a session-based event row (WEC, MotoGP).
/// The race — and the date-only placeholder that stands in for it before a
/// schedule is published — take the event's base id, so the placeholder upserts
/// straight into the race row once times appear (and old single-row caches
/// migrate in place). Support sessions are keyed by name, stable across fetches.
fn session_match_id(id_base: i64, event_id: &str, session: Option<&Session>) -> i64 {
    match session {
        Some(s) if !s.kind.eq_ignore_ascii_case("race") => {
            id_base + stable_hash(&format!("{event_id}|{}", s.name))
        }
        _ => id_base + stable_hash(event_id),
    }
}

/// Whether a session carries a meaningful classification worth a results link:
/// the race, sprint, and qualifying do; free practice / warm-up don't.
fn session_has_result(kind: &str) -> bool {
    matches!(
        kind.to_ascii_lowercase().as_str(),
        "race" | "sprint" | "qualifying"
    )
}

/// Every session of a weekend as its own row (like an F1 weekend), each at its
/// real UTC start with the venue-local time available — the shared shape behind
/// both WEC and MotoGP. `race_window` is how long the race reads as live when the
/// feed's status is vague (WEC enduros run hours; a MotoGP race ~1 h). When
/// `with_results`, results-bearing sessions get a [`MotorResultRef`] so the detail
/// page can fetch that session's finishing order. An event whose schedule isn't
/// published yet yields a single date-only placeholder (noon UTC, no clock).
#[allow(clippy::too_many_arguments)]
fn sessions_to_matches(
    e: &Event,
    league: &str,
    id_base: i64,
    venue_tz: Option<&'static str>,
    race_window: Duration,
    with_results: bool,
    now: DateTime<Utc>,
) -> Vec<NormalizedMatch> {
    let mut out: Vec<NormalizedMatch> = e
        .schedule
        .iter()
        .filter_map(|s| {
            let begin = s.start_time.as_deref().and_then(parse_dt)?;
            let win = if s.kind.eq_ignore_ascii_case("race") {
                race_window
            } else {
                Duration::hours(2)
            };
            let status = status_of(&s.status, begin, begin + win, now);
            let mut m = motorsport_match(
                session_match_id(id_base, &e.id, Some(s)),
                league,
                &session_label(s),
                &e.name,
                begin.year(),
                begin,
                status,
                &e.location,
            );
            // A real session start → offer the venue-local time (like F1/MLB).
            m.venue_tz = venue_tz.map(str::to_string);
            // Link the detail page to this session's results, when the series
            // exposes them and the session is one that's classified.
            if with_results && !s.id.is_empty() && session_has_result(&s.kind) {
                m.motor_result_ref = Some(crate::types::MotorResultRef {
                    series: league.to_string(),
                    event_id: e.id.clone(),
                    session_id: s.id.clone(),
                });
            }
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
                session_match_id(id_base, &e.id, None),
                league,
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

/// Every WEC session of an event as its own row (placeholders for unpublished
/// rounds). WEC sessions carry results (race / qualifying), so they get result
/// links — the detail/event pages fetch each session's order on demand.
fn event_to_matches(e: &Event, now: DateTime<Utc>) -> Vec<NormalizedMatch> {
    sessions_to_matches(
        e,
        "WEC",
        WEC_ID_BASE,
        wec_venue_tz(&e.location.country.two_code),
        Duration::hours(8),
        true,
        now,
    )
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
    Ok(resp
        .data
        .iter()
        .flat_map(|e| event_to_matches(e, now))
        .collect())
}

// ----- MotoGP: /v1/moto-gp/events?year= -------------------------------------

/// IANA timezone for a MotoGP round's venue, mirroring [`wec_venue_tz`]. The 2026
/// calendar runs one zone per host country (Spain's four rounds are all mainland
/// → Madrid; the US round is COTA/Austin → Central; Brazil is Goiânia → São
/// Paulo; Indonesia's Mandalika sits in WITA), so the alpha-2 code fixes the
/// zone. `None` for an unmapped country → no venue-local clock. Update with the
/// calendar.
fn motogp_venue_tz(country_code: &str) -> Option<&'static str> {
    Some(match country_code.to_ascii_uppercase().as_str() {
        "ES" => "Europe/Madrid",
        "PT" => "Europe/Lisbon",
        "QA" => "Asia/Qatar",
        "MY" => "Asia/Kuala_Lumpur",
        // Phillip Island, Victoria.
        "AU" => "Australia/Melbourne",
        // Mandalika is on Lombok (Central Indonesia Time, UTC+8).
        "ID" => "Asia/Makassar",
        "JP" | "JPN" => "Asia/Tokyo",
        "AT" => "Europe/Vienna",
        // San Marino observes CET; the Misano circuit is in Italy.
        "SM" => "Europe/Rome",
        "GB" => "Europe/London",
        "DE" => "Europe/Berlin",
        "NL" => "Europe/Amsterdam",
        "CZ" => "Europe/Prague",
        "HU" => "Europe/Budapest",
        "IT" => "Europe/Rome",
        "FR" => "Europe/Paris",
        // Circuit of the Americas, Austin (US Central).
        "US" => "America/Chicago",
        "BR" => "America/Sao_Paulo",
        "TH" => "Asia/Bangkok",
        _ => return None,
    })
}

/// Every MotoGP session of a Grand Prix as its own row (like F1/WEC), each with
/// its venue-local time and — for the classified sessions (race / sprint /
/// qualifying) — a link to that session's results. The MotoGP events feed has the
/// same shape as WEC's.
pub async fn fetch_motogp(
    client: &reqwest::Client,
    key: &str,
    year: i32,
) -> Result<Vec<NormalizedMatch>, reqwest::Error> {
    let url = format!("{BASE}/moto-gp/events?year={year}&limit=100");
    let resp: EventsResp = get(client, key, &url).await?;
    let now = Utc::now();
    Ok(resp
        .data
        .iter()
        .flat_map(|e| {
            sessions_to_matches(
                e,
                "MotoGP",
                MOTOGP_ID_BASE,
                motogp_venue_tz(&e.location.country.two_code),
                Duration::hours(2),
                true,
                now,
            )
        })
        .collect())
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

// ----- MotoGP standings -----------------------------------------------------

/// Points come as a decimal string ("186.00"); show a whole number when it is one.
fn fmt_points_str(s: &str) -> String {
    s.parse::<f64>()
        .map(fmt_points)
        .unwrap_or_else(|_| s.to_string())
}

#[derive(Deserialize)]
struct MotoGpRider {
    #[serde(default)]
    position: i64,
    #[serde(default)]
    points: String,
    #[serde(rename = "firstName", default)]
    first: String,
    #[serde(rename = "lastName", default)]
    last: String,
}

#[derive(Deserialize)]
struct MotoGpTeamStand {
    #[serde(default)]
    position: i64,
    #[serde(default)]
    points: String,
    #[serde(default)]
    name: String,
}

/// MotoGP standings: two requests (riders + teams), each a bare array. The feed
/// carries no nationality, so rows have no flag (unlike WRC's). Any table that
/// fails or is empty is simply omitted.
pub async fn fetch_motogp_standings(client: &reqwest::Client, key: &str) -> MotorStandings {
    let mut tables = Vec::new();
    if let Ok(rows) =
        get::<Vec<MotoGpRider>>(client, key, &format!("{BASE}/moto-gp/standings/drivers")).await
    {
        let rows: Vec<MotorStandingRow> = rows
            .iter()
            .map(|r| MotorStandingRow {
                pos: r.position.to_string(),
                name: format!("{} {}", r.first, r.last).trim().to_string(),
                points: fmt_points_str(&r.points),
                flag: String::new(),
            })
            .collect();
        if !rows.is_empty() {
            tables.push(MotorStandingTable {
                group: String::new(),
                title: "Riders".to_string(),
                rows,
            });
        }
    }
    if let Ok(rows) =
        get::<Vec<MotoGpTeamStand>>(client, key, &format!("{BASE}/moto-gp/standings/teams")).await
    {
        let rows: Vec<MotorStandingRow> = rows
            .iter()
            .map(|r| MotorStandingRow {
                pos: r.position.to_string(),
                name: r.name.clone(),
                points: fmt_points_str(&r.points),
                flag: String::new(),
            })
            .collect();
        if !rows.is_empty() {
            tables.push(MotorStandingTable {
                group: String::new(),
                title: "Teams".to_string(),
                rows,
            });
        }
    }
    MotorStandings { tables }
}

// ----- Results (WRC stage/overall, MotoGP session) --------------------------

/// A driver/rider name pair from a results row.
#[derive(Deserialize, Default)]
struct Person {
    #[serde(rename = "firstName", default)]
    first: String,
    #[serde(rename = "lastName", default)]
    last: String,
}

impl Person {
    fn name(&self) -> String {
        format!("{} {}", self.first, self.last).trim().to_string()
    }
}

#[derive(Deserialize, Default)]
struct NamedRef {
    #[serde(default)]
    name: String,
}

/// A WRC results row — shared by the overall classification (`totalTime`) and a
/// single stage's times (`stageTime`); `diffFirst` is the gap to the leader.
#[derive(Deserialize)]
struct WrcResultRow {
    #[serde(default)]
    position: String,
    #[serde(default)]
    driver: Person,
    #[serde(rename = "coDriver", default)]
    co_driver: Person,
    #[serde(default)]
    team: Option<NamedRef>,
    #[serde(default)]
    manufacturer: Option<NamedRef>,
    #[serde(rename = "totalTime", default, deserialize_with = "de_null_default")]
    total_time: String,
    #[serde(rename = "stageTime", default, deserialize_with = "de_null_default")]
    stage_time: String,
    #[serde(rename = "diffFirst", default, deserialize_with = "de_null_default")]
    diff_first: String,
}

fn wrc_result_rows(rows: &[WrcResultRow]) -> Vec<crate::types::MotorResultRow> {
    rows.iter()
        .map(|r| {
            let total = if r.total_time.is_empty() {
                r.stage_time.clone()
            } else {
                r.total_time.clone()
            };
            // The leader shows the absolute time; everyone else the gap to first.
            let time = if r.diff_first.starts_with('+') {
                r.diff_first.clone()
            } else {
                total
            };
            let team = r
                .team
                .as_ref()
                .map(|t| t.name.clone())
                .filter(|s| !s.is_empty())
                .or_else(|| r.manufacturer.as_ref().map(|m| m.name.clone()))
                .unwrap_or_default();
            crate::types::MotorResultRow {
                pos: clean_pos(&r.position),
                name: r.driver.name(),
                codriver: r.co_driver.name(),
                team,
                time,
                flag: String::new(),
            }
        })
        .collect()
}

/// A MotoGP session results row. The leader's `lapTime` is the absolute time;
/// `gap` holds the deficit for the rest.
#[derive(Deserialize)]
struct MotoResultRow {
    #[serde(default)]
    position: String,
    #[serde(default)]
    driver: Person,
    #[serde(default)]
    team: Option<NamedRef>,
    #[serde(rename = "lapTime", default, deserialize_with = "de_null_default")]
    lap_time: String,
    #[serde(default)]
    gap: Option<String>,
}

fn motogp_result_rows(rows: &[MotoResultRow]) -> Vec<crate::types::MotorResultRow> {
    rows.iter()
        .map(|r| {
            let gap = r.gap.clone().unwrap_or_default();
            let time = if !gap.is_empty() && gap != "0.000" {
                gap
            } else {
                r.lap_time.clone()
            };
            crate::types::MotorResultRow {
                pos: clean_pos(&r.position),
                name: r.driver.name(),
                codriver: String::new(),
                team: r.team.as_ref().map(|t| t.name.clone()).unwrap_or_default(),
                time,
                flag: String::new(),
            }
        })
        .collect()
}

/// A WEC session results row. WEC lists one row per driver, so a crew's 2–3
/// drivers share a `position`; [`wec_result_rows`] groups them. The leader often
/// reports only a lap count, so the time falls back through
/// `gap`/`displayTime`/`lapTime` to "{laps} laps".
#[derive(Deserialize)]
struct WecResultRow {
    #[serde(default)]
    position: String,
    #[serde(default)]
    driver: Person,
    #[serde(default)]
    team: Option<NamedRef>,
    #[serde(default)]
    laps: Option<i64>,
    #[serde(rename = "lapTime", default)]
    lap_time: Option<String>,
    #[serde(rename = "displayTime", default)]
    display_time: Option<String>,
    #[serde(default)]
    gap: Option<String>,
}

/// Collapse a WEC session's per-driver rows into one row per car: consecutive
/// rows sharing a `position` are one crew (driver names joined with " · "),
/// carrying the team and a single time. Time prefers the gap to the leader, then
/// an absolute display/lap time, then the lap count.
fn wec_result_rows(rows: &[WecResultRow]) -> Vec<crate::types::MotorResultRow> {
    let nonempty = |s: &str| !s.is_empty() && s != "0.000";
    let mut out = Vec::new();
    let mut i = 0;
    while i < rows.len() {
        let pos = &rows[i].position;
        let mut j = i;
        let mut names = Vec::new();
        while j < rows.len() && rows[j].position == *pos {
            names.push(rows[j].driver.name());
            j += 1;
        }
        let head = &rows[i];
        let team = head
            .team
            .as_ref()
            .map(|t| t.name.clone())
            .unwrap_or_default();
        let time = head
            .gap
            .clone()
            .filter(|s| nonempty(s))
            .or_else(|| head.display_time.clone().filter(|s| nonempty(s)))
            .or_else(|| head.lap_time.clone().filter(|s| nonempty(s)))
            .or_else(|| head.laps.map(|l| format!("{l} laps")))
            .unwrap_or_default();
        out.push(crate::types::MotorResultRow {
            pos: clean_pos(pos),
            name: names.join(" · "),
            codriver: String::new(),
            team,
            time,
            flag: String::new(),
        });
        i = j;
    }
    out
}

/// Fetch the classification a [`crate::types::MotorResultRef`] points at: a WRC
/// rally's overall result (empty session), a single WRC stage's times, a WEC or
/// MotoGP session's finishing order. `None` on a fetch/parse error (so the caller
/// doesn't cache a transient failure); `Some(rows)` on success — `rows` is empty
/// only when the source genuinely returned nothing. One request.
pub async fn fetch_motor_result(
    client: &reqwest::Client,
    key: &str,
    r: &crate::types::MotorResultRef,
) -> Option<Vec<crate::types::MotorResultRow>> {
    match (r.series.as_str(), r.session_id.is_empty()) {
        ("WRC", true) => {
            let url = format!("{BASE}/wrc/rallies/{}/classification", r.event_id);
            get::<Vec<WrcResultRow>>(client, key, &url)
                .await
                .map(|rows| wrc_result_rows(&rows))
                .ok()
        }
        ("WRC", false) => {
            // The stage detail carries its results inline.
            #[derive(Deserialize, Default)]
            struct StageDetail {
                #[serde(default)]
                results: Vec<WrcResultRow>,
            }
            let url = format!("{BASE}/wrc/rallies/{}/stages/{}", r.event_id, r.session_id);
            get::<StageDetail>(client, key, &url)
                .await
                .map(|d| wrc_result_rows(&d.results))
                .ok()
        }
        ("WEC", _) => {
            let url = format!(
                "{BASE}/wec/events/{}/sessions/{}/results",
                r.event_id, r.session_id
            );
            get::<Vec<WecResultRow>>(client, key, &url)
                .await
                .map(|rows| wec_result_rows(&rows))
                .ok()
        }
        ("MotoGP", _) => {
            let url = format!(
                "{BASE}/moto-gp/events/{}/sessions/{}/results",
                r.event_id, r.session_id
            );
            get::<Vec<MotoResultRow>>(client, key, &url)
                .await
                .map(|rows| motogp_result_rows(&rows))
                .ok()
        }
        _ => Some(Vec::new()),
    }
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
        let ms: Vec<NormalizedMatch> = resp
            .data
            .iter()
            .filter_map(|r| rally_to_match(r, now))
            .collect();
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
    fn wrc_rally_detail_expands_each_stage_with_venue_time() {
        // The rally's name/location come from the list entry; the detail adds the
        // timetable (shakedown + numbered stages + the power stage broken out).
        let r: Rally = serde_json::from_str(
            r#"{"id":"r-gr","name":"EKO Acropolis Rally Greece","dateStart":"2026-06-25",
                "dateEnd":"2026-06-28","status":"ongoing",
                "location":{"name":"Loutraki","city":"Loutraki","country":{"name":"Greece","twoCode":"GR"}}}"#,
        )
        .unwrap();
        let detail: RallyDetail = serde_json::from_str(
            r#"{"schedule":{
                 "shakedown":[],
                 "specialStages":[
                   {"id":"ss1","stageCode":"SS1","name":"SS1 EKO SSS","type":"special_stage",
                    "status":"completed","startTime":"2026-06-25T16:02:00.000Z"},
                   {"id":"ss2","stageCode":"SS2","name":"SS2 Bauxites","type":"special_stage",
                    "status":"scheduled","startTime":"2026-06-26T05:45:00.000Z"}
                 ],
                 "powerStage":{"id":"ss17","stageCode":"SS17","name":"SS17 Loutraki 2 Wolf Power Stage",
                   "type":"power_stage","status":"scheduled","startTime":"2026-06-28T11:12:00.000Z"}
               }}"#,
        )
        .unwrap();
        // Mid-rally: SS1 has run, SS2 is on, the power stage is days away.
        let now = "2026-06-26T05:50:00Z".parse::<DateTime<Utc>>().unwrap();
        let ms = rally_stage_matches(&r, &detail, now);
        // One row per timed stage (2 special + the power stage), all WRC under the
        // season-qualified series, each carrying Greece's tz for a venue-local clock.
        assert_eq!(ms.len(), 3);
        assert!(ms
            .iter()
            .all(|m| m.league == "WRC" && m.series_name == "EKO Acropolis Rally Greece 2026"));
        assert!(ms
            .iter()
            .all(|m| m.venue_tz.as_deref() == Some("Europe/Athens")));
        let ss1 = ms.iter().find(|m| m.team_a.label == "SS1 EKO SSS").unwrap();
        assert_eq!(ss1.begin_at.to_rfc3339(), "2026-06-25T16:02:00+00:00");
        assert_eq!(ss1.status, MatchStatus::Finished);
        // A vague "scheduled" status is read off the window — SS2 is in progress.
        let ss2 = ms
            .iter()
            .find(|m| m.team_a.label == "SS2 Bauxites")
            .unwrap();
        assert_eq!(ss2.status, MatchStatus::Live);
        // The power stage rides in as its own row, labelled so the schedule can flag
        // the day, and still upcoming.
        let ps = ms
            .iter()
            .find(|m| m.team_a.label.contains("Power Stage"))
            .unwrap();
        assert_eq!(ps.begin_at.to_rfc3339(), "2026-06-28T11:12:00+00:00");
        assert_eq!(ps.status, MatchStatus::Upcoming);
        // Stage ids hash the stage UUID, so none collides with the rally's date-only
        // placeholder (keyed off the rally id) and all stay in the WRC id band.
        let placeholder = WRC_ID_BASE + stable_hash("r-gr");
        assert!(ms.iter().all(|m| m.id != placeholder));
        assert!(ms.iter().all(|m| m.id >= WRC_ID_BASE && m.id < WEC_ID_BASE));
    }

    #[test]
    fn active_rally_prefers_live_then_the_imminent_one() {
        let resp: RalliesResp = serde_json::from_str(
            r#"{"data":[
              {"id":"past","name":"A","dateStart":"2026-05-01","dateEnd":"2026-05-04",
               "status":"completed","location":{"country":{"twoCode":"FI"}}},
              {"id":"live","name":"B","dateStart":"2026-06-25","dateEnd":"2026-06-28",
               "status":"ongoing","location":{"country":{"twoCode":"GR"}}},
              {"id":"soon","name":"C","dateStart":"2026-07-02","dateEnd":"2026-07-05",
               "status":"scheduled","location":{"country":{"twoCode":"EE"}}},
              {"id":"far","name":"D","dateStart":"2026-09-01","dateEnd":"2026-09-04",
               "status":"scheduled","location":{"country":{"twoCode":"CL"}}}
            ]}"#,
        )
        .unwrap();
        let now = "2026-06-26T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        // A live rally wins outright, even over an earlier-listed finished one.
        assert_eq!(active_rally(&resp.data, now).unwrap().id, "live");
        // With none live, the soonest within the ~2-week publish lead is taken; the
        // far-off September rally is skipped, so no detail request is wasted on it.
        assert_eq!(active_rally(&resp.data[2..], now).unwrap().id, "soon");
        assert!(active_rally(&resp.data[3..], now).is_none());
    }

    #[test]
    fn active_rally_captures_a_just_finished_rally() {
        let resp: RalliesResp = serde_json::from_str(
            r#"{"data":[
              {"id":"justdone","name":"A","dateStart":"2026-06-25","dateEnd":"2026-06-28",
               "status":"completed","location":{"country":{"twoCode":"GR"}}},
              {"id":"soon","name":"B","dateStart":"2026-07-10","dateEnd":"2026-07-13",
               "status":"scheduled","location":{"country":{"twoCode":"EE"}}}
            ]}"#,
        )
        .unwrap();
        // A day after the rally ended its final stages are still worth fetching —
        // and take precedence over the upcoming round's schedule pre-fetch, so a
        // finished rally's per-stage results get captured and kept.
        let now = "2026-06-29T12:00:00Z".parse::<DateTime<Utc>>().unwrap();
        assert_eq!(active_rally(&resp.data, now).unwrap().id, "justdone");
        // Well after it ended (its stages are already persisted) fall back to the
        // soonest upcoming, so no request is wasted re-fetching the old rally.
        let later = "2026-07-05T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        assert_eq!(active_rally(&resp.data, later).unwrap().id, "soon");
    }

    #[test]
    fn wrc_venue_tz_maps_calendar_hosts() {
        assert_eq!(wrc_venue_tz("GR"), Some("Europe/Athens"));
        assert_eq!(wrc_venue_tz("jp"), Some("Asia/Tokyo")); // case-insensitive alpha-2
        assert_eq!(wrc_venue_tz("JPN"), Some("Asia/Tokyo")); // alpha-3, as the feed sends
                                                             // Spain's round runs on the Canary Islands, an hour behind the mainland.
        assert_eq!(wrc_venue_tz("ES"), Some("Atlantic/Canary"));
        // Off-calendar countries simply have no venue clock.
        assert_eq!(wrc_venue_tz("US"), None);
        assert_eq!(wrc_venue_tz(""), None);
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
        let ms: Vec<NormalizedMatch> = resp
            .data
            .iter()
            .flat_map(|e| event_to_matches(e, now))
            .collect();
        // One row per session, all under the same season-qualified series.
        assert_eq!(ms.len(), 3);
        assert!(ms
            .iter()
            .all(|m| m.league == "WEC" && m.series_name == "24 Hours of Le Mans 2025"));
        // Each session is labelled by its published name and carries the venue
        // tz, so every row gets the venue-local toggle (FR → Paris).
        assert!(ms
            .iter()
            .all(|m| m.venue_tz.as_deref() == Some("Europe/Paris")));
        let race = ms.iter().find(|m| m.team_a.label == "Race").unwrap();
        assert_eq!(race.begin_at.to_rfc3339(), "2025-06-14T14:00:00+00:00");
        assert_eq!(race.status, MatchStatus::Finished);
        // The race keeps the event's base id (so a placeholder upserts into it);
        // a support session is labelled by name and gets a distinct id.
        assert_eq!(race.id, WEC_ID_BASE + stable_hash("uuid-lm"));
        let quali = ms
            .iter()
            .find(|m| m.team_a.label == "Qualifying HYPERCAR")
            .unwrap();
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
        let ms: Vec<NormalizedMatch> = resp
            .data
            .iter()
            .flat_map(|e| event_to_matches(e, now))
            .collect();
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
    fn wec_results_group_crews_and_fall_back_to_laps() {
        // WEC lists one row per driver; a crew shares a position. The leader here
        // reports only a lap count (no gap/displayTime/lapTime).
        let json = r#"[
          {"position":"1","driver":{"firstName":"Robert","lastName":"Kubica"},"team":{"name":"AF Corse"},"laps":387,"gap":null,"displayTime":null,"lapTime":null},
          {"position":"1","driver":{"firstName":"Yifei","lastName":"Ye"},"team":{"name":"AF Corse"},"laps":387},
          {"position":"1","driver":{"firstName":"Phil","lastName":"Hanson"},"team":{"name":"AF Corse"},"laps":387},
          {"position":"2","driver":{"firstName":"Matt","lastName":"Campbell"},"team":{"name":"Porsche Penske Motorsport"},"laps":387,"gap":"+1 lap"}
        ]"#;
        let rows: Vec<WecResultRow> = serde_json::from_str(json).unwrap();
        let out = wec_result_rows(&rows);
        // Three drivers of the winning car collapse to one crew row.
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].pos, "1");
        assert_eq!(out[0].name, "Robert Kubica · Yifei Ye · Phil Hanson");
        assert_eq!(out[0].codriver, "");
        assert_eq!(out[0].team, "AF Corse");
        // Leader: no gap/time → fall back to the lap count.
        assert_eq!(out[0].time, "387 laps");
        // Runner-up: the gap to the leader.
        assert_eq!(out[1].name, "Matt Campbell");
        assert_eq!(out[1].time, "+1 lap");
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

    #[test]
    fn motogp_event_expands_sessions_with_result_links() {
        // MotoGP events have the same shape as WEC's, with typed sessions.
        let json = r#"{"data":[
          {"id":"gp-val","name":"GRAND PRIX OF VALENCIA","dateStart":"2026-11-27","dateEnd":"2026-11-29",
           "status":"completed",
           "location":{"name":"Circuit Ricardo Tormo","city":"Cheste","country":{"name":"Spain","twoCode":"ES"}},
           "schedule":[
             {"id":"s-fp1","name":"Free Practice 1","type":"practice","startTime":"2026-11-27T09:45:00.000Z","status":"completed"},
             {"id":"s-q2","name":"Qualifying 2","type":"qualifying","startTime":"2026-11-28T10:15:00.000Z","status":"completed"},
             {"id":"s-spr","name":"Sprint","type":"sprint","startTime":"2026-11-28T14:00:00.000Z","status":"completed"},
             {"id":"s-race","name":"Race","type":"race","startTime":"2026-11-29T13:00:00.000Z","status":"completed"}
           ]}
        ]}"#;
        let resp: EventsResp = serde_json::from_str(json).unwrap();
        let now = "2026-12-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let ms: Vec<NormalizedMatch> = resp
            .data
            .iter()
            .flat_map(|e| {
                sessions_to_matches(
                    e,
                    "MotoGP",
                    MOTOGP_ID_BASE,
                    motogp_venue_tz("ES"),
                    Duration::hours(2),
                    true,
                    now,
                )
            })
            .collect();
        // One row per session, all MotoGP under the season-qualified series, with
        // Spain's venue tz available.
        assert_eq!(ms.len(), 4);
        assert!(ms
            .iter()
            .all(|m| m.league == "MotoGP" && m.series_name == "GRAND PRIX OF VALENCIA 2026"));
        assert!(ms
            .iter()
            .all(|m| m.venue_tz.as_deref() == Some("Europe/Madrid")));
        assert!(ms.iter().all(|m| m.id >= MOTOGP_ID_BASE));
        // Results-bearing sessions (race/sprint/qualifying) carry a result ref;
        // free practice doesn't.
        let race = ms.iter().find(|m| m.team_a.label == "Race").unwrap();
        let rref = race.motor_result_ref.as_ref().unwrap();
        assert_eq!(
            (
                rref.series.as_str(),
                rref.event_id.as_str(),
                rref.session_id.as_str()
            ),
            ("MotoGP", "gp-val", "s-race")
        );
        assert!(ms
            .iter()
            .find(|m| m.team_a.label == "Sprint")
            .unwrap()
            .motor_result_ref
            .is_some());
        assert!(ms
            .iter()
            .find(|m| m.team_a.label == "Qualifying 2")
            .unwrap()
            .motor_result_ref
            .is_some());
        assert!(ms
            .iter()
            .find(|m| m.team_a.label == "Free Practice 1")
            .unwrap()
            .motor_result_ref
            .is_none());
    }

    #[test]
    fn parses_motogp_standings_riders_and_teams() {
        let riders = r#"[
          {"id":"r1","position":1,"points":"186.00","firstName":"Marco","lastName":"Bezzecchi","code":null,"number":null,"teams":[]},
          {"id":"r2","position":2,"points":"170.50","firstName":"Marc","lastName":"Marquez"}
        ]"#;
        let rows: Vec<MotoGpRider> = serde_json::from_str(riders).unwrap();
        // Whole-number points print without the decimal; fractional ones keep it.
        assert_eq!(fmt_points_str(&rows[0].points), "186");
        assert_eq!(fmt_points_str(&rows[1].points), "170.5");
        assert_eq!(
            format!("{} {}", rows[0].first, rows[0].last),
            "Marco Bezzecchi"
        );
        let teams = r##"[{"id":"t1","position":1,"points":"363.00","name":"Aprilia Racing","shortName":"Aprilia Racing","color":"#5f259f"}]"##;
        let ts: Vec<MotoGpTeamStand> = serde_json::from_str(teams).unwrap();
        assert_eq!(ts[0].name, "Aprilia Racing");
        assert_eq!(fmt_points_str(&ts[0].points), "363");
    }

    #[test]
    fn wrc_classification_rows_show_leader_time_then_gaps() {
        // The overall classification shape (driver + co-driver + team, total time).
        let json = r#"[
          {"position":"1","driver":{"firstName":"Oliver","lastName":"SOLBERG"},
           "coDriver":{"firstName":"Elliott","lastName":"EDMONDSON"},
           "team":{"name":"TOYOTA GAZOO RACING WRT"},"manufacturer":{"name":"Toyota"},
           "totalTime":"4:24:59.000","diffFirst":"0.000"},
          {"position":"2","driver":{"firstName":"Elfyn","lastName":"EVANS"},
           "coDriver":{"firstName":"Scott","lastName":"MARTIN"},
           "team":{"name":"TOYOTA GAZOO RACING WRT"},"manufacturer":{"name":"Toyota"},
           "totalTime":"4:25:50.800","diffFirst":"+51.800"}
        ]"#;
        let rows: Vec<WrcResultRow> = serde_json::from_str(json).unwrap();
        let out = wrc_result_rows(&rows);
        assert_eq!(out.len(), 2);
        // The leader shows the absolute time; the rest show the gap to first.
        assert_eq!(out[0].name, "Oliver SOLBERG");
        assert_eq!(out[0].codriver, "Elliott EDMONDSON");
        assert_eq!(out[0].team, "TOYOTA GAZOO RACING WRT");
        assert_eq!(out[0].time, "4:24:59.000");
        assert_eq!(out[1].time, "+51.800");
        // Results carry no nationality, so no flag.
        assert_eq!(out[0].flag, "");
    }

    #[test]
    fn wrc_stage_results_use_stage_time() {
        // A single stage's times come under `stageTime` (no `totalTime`).
        let json = r#"[
          {"position":"1","driver":{"firstName":"Elfyn","lastName":"EVANS"},
           "coDriver":{"firstName":"Scott","lastName":"MARTIN"},"team":{"name":"Toyota"},
           "stageTime":"16:05.700","diffFirst":"0.000"},
          {"position":"2","driver":{"firstName":"Oliver","lastName":"SOLBERG"},
           "coDriver":{"firstName":"Elliott","lastName":"EDMONDSON"},"team":{"name":"Toyota"},
           "stageTime":"16:06.600","diffFirst":"+0.900"}
        ]"#;
        let rows: Vec<WrcResultRow> = serde_json::from_str(json).unwrap();
        let out = wrc_result_rows(&rows);
        assert_eq!(out[0].time, "16:05.700");
        assert_eq!(out[1].time, "+0.900");
    }

    #[test]
    fn motogp_session_results_leader_total_then_gaps() {
        let json = r#"[
          {"position":"1","driver":{"firstName":"Marc","lastName":"Marquez"},
           "team":{"name":"Ducati Lenovo Team"},"lapTime":"39:51.297","gap":null},
          {"position":"2","driver":{"firstName":"Marco","lastName":"Bezzecchi"},
           "team":{"name":"Aprilia Racing"},"lapTime":"39:53.100","gap":"+1.803"}
        ]"#;
        let rows: Vec<MotoResultRow> = serde_json::from_str(json).unwrap();
        let out = motogp_result_rows(&rows);
        assert_eq!(out[0].name, "Marc Marquez");
        assert_eq!(out[0].codriver, ""); // bikes have no co-driver
        assert_eq!(out[0].team, "Ducati Lenovo Team");
        // Leader shows the absolute time; others the gap.
        assert_eq!(out[0].time, "39:51.297");
        assert_eq!(out[1].time, "+1.803");
    }

    #[test]
    fn wrc_results_tolerate_null_times_on_dns_rows() {
        // A DNS/DNF row sends stageTime/diffFirst as JSON null (present, not absent),
        // and position as the literal string "null". #[serde(default)] alone errors on
        // a present null and would drop the whole stage's results.
        let json = r#"[
          {"position":"1","driver":{"firstName":"Sébastien","lastName":"OGIER"},
           "team":{"name":"TOYOTA GAZOO RACING WRT"},"stageTime":"11:10.909","diffFirst":"0.000"},
          {"position":"null","status":"DNS","driver":{"firstName":"Dani","lastName":"SORDO"},
           "team":{"name":"Hyundai"},"stageTime":null,"diffFirst":null}
        ]"#;
        let rows: Vec<WrcResultRow> = serde_json::from_str(json).unwrap();
        let out = wrc_result_rows(&rows);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].name, "Sébastien OGIER");
        assert_eq!(out[0].time, "11:10.909");
        // The DNS row parses (instead of failing the whole array): blank position,
        // empty time.
        assert_eq!(out[1].name, "Dani SORDO");
        assert_eq!(out[1].pos, "");
        assert_eq!(out[1].time, "");
    }

    #[test]
    fn motogp_results_tolerate_null_lap_time() {
        // A DNF rider can send lapTime as null; it must not fail the whole session.
        let json = r#"[
          {"position":"1","driver":{"firstName":"Marc","lastName":"Marquez"},
           "team":{"name":"Ducati Lenovo Team"},"lapTime":"39:51.297","gap":null},
          {"position":"null","driver":{"firstName":"Joan","lastName":"Mir"},
           "team":{"name":"Honda HRC"},"lapTime":null,"gap":null}
        ]"#;
        let rows: Vec<MotoResultRow> = serde_json::from_str(json).unwrap();
        let out = motogp_result_rows(&rows);
        assert_eq!(out.len(), 2);
        assert_eq!(out[1].name, "Joan Mir");
        assert_eq!(out[1].pos, "");
        assert_eq!(out[1].time, "");
    }
}
