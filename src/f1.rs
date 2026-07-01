//! Formula 1 schedule from the free, keyless Jolpica API (the Ergast successor,
//! `api.jolpi.ca`). A Grand Prix weekend is a set of sessions (practice /
//! qualifying / sprint / race); we emit one [`NormalizedMatch`] per session,
//! grouped under the Grand Prix as the event (`league = "F1"`, `series = the GP
//! name`). F1 is single-entity — a session has no opposing team — so the row's
//! one label is the session name (e.g. "Race"); results live on the event page.

use crate::feed::{NormalizedMatch, NormalizedTeam};
use crate::http::USER_AGENT;
use crate::types::{F1Result, F1ResultRow, F1StandingRow, F1Standings, MatchStatus, Sport};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::Deserialize;

const BASE: &str = "https://api.jolpi.ca/ergast/f1";

#[derive(Deserialize)]
struct Resp {
    #[serde(rename = "MRData")]
    data: MrData,
}

#[derive(Deserialize)]
struct MrData {
    #[serde(rename = "RaceTable")]
    race_table: RaceTable,
}

#[derive(Deserialize)]
struct RaceTable {
    #[serde(rename = "Races", default)]
    races: Vec<RawRace>,
}

#[derive(Deserialize)]
struct RawRace {
    season: String,
    round: String,
    #[serde(rename = "raceName")]
    race_name: String,
    #[serde(default)]
    url: String, // Jolpica's Wikipedia race-page URL
    #[serde(rename = "Circuit", default)]
    circuit: RawCircuit,
    /// The race session's own date/time.
    date: String,
    #[serde(default)]
    time: Option<String>,
    #[serde(rename = "FirstPractice", default)]
    fp1: Option<RawSession>,
    #[serde(rename = "SecondPractice", default)]
    fp2: Option<RawSession>,
    #[serde(rename = "ThirdPractice", default)]
    fp3: Option<RawSession>,
    #[serde(rename = "SprintQualifying", default)]
    sprint_quali: Option<RawSession>,
    #[serde(rename = "Sprint", default)]
    sprint: Option<RawSession>,
    #[serde(rename = "Qualifying", default)]
    qualifying: Option<RawSession>,
}

#[derive(Deserialize)]
struct RawSession {
    date: String,
    #[serde(default)]
    time: Option<String>,
}

#[derive(Deserialize, Default)]
struct RawCircuit {
    #[serde(rename = "circuitId", default)]
    circuit_id: String,
    #[serde(rename = "circuitName", default)]
    circuit_name: String,
    #[serde(rename = "Location", default)]
    location: RawCircuitLocation,
}

#[derive(Deserialize, Default)]
struct RawCircuitLocation {
    #[serde(default)]
    locality: String,
    #[serde(default)]
    country: String,
}

impl RawCircuitLocation {
    /// "Locality, Country" (e.g. "Melbourne, Australia").
    fn label(&self) -> String {
        match (self.locality.is_empty(), self.country.is_empty()) {
            (false, false) => format!("{}, {}", self.locality, self.country),
            (false, true) => self.locality.clone(),
            (true, false) => self.country.clone(),
            _ => String::new(),
        }
    }
}

/// The Grand Prix host country's flag (a vector SVG from flagcdn) — F1 has no
/// teams, so the GP's national flag stands in as its icon. Empty for an unmapped
/// country. Ergast/Jolpica country names are mostly the common English ones.
fn country_flag(country: &str) -> String {
    let code = match country {
        "Australia" => "au",
        "Bahrain" => "bh",
        "Saudi Arabia" => "sa",
        "Japan" => "jp",
        "China" => "cn",
        "USA" | "United States" => "us",
        "Italy" => "it",
        "Monaco" => "mc",
        "Canada" => "ca",
        "Spain" => "es",
        "Austria" => "at",
        "UK" | "United Kingdom" => "gb",
        "Hungary" => "hu",
        "Belgium" => "be",
        "Netherlands" => "nl",
        "Azerbaijan" => "az",
        "Singapore" => "sg",
        "Mexico" => "mx",
        "Brazil" => "br",
        "Qatar" => "qa",
        "UAE" | "United Arab Emirates" => "ae",
        "France" => "fr",
        "Germany" => "de",
        "Portugal" => "pt",
        "Turkey" => "tr",
        "Russia" => "ru",
        "South Africa" => "za",
        "Argentina" => "ar",
        _ => return String::new(),
    };
    format!("https://flagcdn.com/{code}.svg")
}

/// The formula1.com race-page slug for a circuit (verified against the 2026
/// calendar). `None` for circuits we haven't confirmed — the caller then uses the
/// Jolpica Wikipedia URL, so a new/renamed circuit never yields a broken link.
///
/// Slugs are country/location names that can shift between seasons (`yas_marina`
/// has differed year to year); re-verify each entry against
/// `formula1.com/en/racing/{season}/{slug}` when advancing the season. This is the
/// one case the Wikipedia fallback does NOT catch — a *mapped* circuit whose slug
/// went stale yields a wrong/404 link rather than falling back. Keep the circuit
/// list in sync with [`circuit_tz`] (same `circuit_id`s; `madring` is deliberately
/// only in `circuit_tz`, so it falls back to Wikipedia here).
fn circuit_slug(circuit_id: &str) -> Option<&'static str> {
    Some(match circuit_id {
        "albert_park" => "australia",
        "shanghai" => "china",
        "suzuka" => "japan",
        "miami" => "miami",
        "villeneuve" => "canada",
        "monaco" => "monaco",
        "catalunya" => "spain",
        "red_bull_ring" => "austria",
        "silverstone" => "great-britain",
        "spa" => "belgium",
        "hungaroring" => "hungary",
        "zandvoort" => "netherlands",
        "monza" => "italy",
        "baku" => "azerbaijan",
        "marina_bay" => "singapore",
        "americas" => "united-states",
        "rodriguez" => "mexico",
        "interlagos" => "brazil",
        "vegas" => "las-vegas",
        "losail" => "qatar",
        "yas_marina" => "united-arab-emirates",
        _ => return None,
    })
}

/// The best race-page URL for a Grand Prix: the official formula1.com page when
/// we have a verified slug for the circuit, else the Jolpica Wikipedia URL.
/// `None` only when both are unavailable.
fn race_page_url(circuit_id: &str, season: i64, wiki_url: &str) -> Option<String> {
    if let Some(slug) = circuit_slug(circuit_id) {
        return Some(format!(
            "https://www.formula1.com/en/racing/{season}/{slug}"
        ));
    }
    let w = wiki_url.trim();
    (!w.is_empty()).then(|| w.to_string())
}

/// The IANA timezone of each Grand Prix circuit (Jolpica gives only a locality,
/// not a tz), so the schedule can show the local time at the track. Keyed on the
/// stable `circuitId`. Adding a circuit means updating [`circuit_slug`] too (its
/// race-page slug), unless — like `madring` — it should fall back to Wikipedia.
fn circuit_tz(circuit_id: &str) -> Option<&'static str> {
    Some(match circuit_id {
        "albert_park" => "Australia/Melbourne",
        "shanghai" => "Asia/Shanghai",
        "suzuka" => "Asia/Tokyo",
        "miami" => "America/New_York",
        "villeneuve" => "America/Toronto",
        "monaco" => "Europe/Monaco",
        "catalunya" | "madring" => "Europe/Madrid",
        "red_bull_ring" => "Europe/Vienna",
        "silverstone" => "Europe/London",
        "spa" => "Europe/Brussels",
        "hungaroring" => "Europe/Budapest",
        "zandvoort" => "Europe/Amsterdam",
        "monza" => "Europe/Rome",
        "baku" => "Asia/Baku",
        "marina_bay" => "Asia/Singapore",
        "americas" => "America/Chicago",
        "rodriguez" => "America/Mexico_City",
        "interlagos" => "America/Sao_Paulo",
        "vegas" => "America/Los_Angeles",
        "losail" => "Asia/Qatar",
        "yas_marina" => "Asia/Dubai",
        _ => return None,
    })
}

/// One session of a weekend: its order (for a stable id + display sort), label,
/// and start.
struct Session {
    ord: i64,
    label: &'static str,
    date: String,
    time: Option<String>,
}

/// Parse Ergast's split date ("2026-06-28") + time ("13:00:00Z") into UTC.
/// A session with no time (rare) is anchored to the start of its day.
fn parse_dt(date: &str, time: Option<&str>) -> Option<DateTime<Utc>> {
    let t = time.unwrap_or("00:00:00Z");
    DateTime::parse_from_rfc3339(&format!("{date}T{t}"))
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// Every session of a weekend, in chronological/order sequence. Sprint weekends
/// omit FP2/FP3 and add Sprint Qualifying + Sprint; the absent ones are skipped.
fn sessions(r: &RawRace) -> Vec<Session> {
    let mut out = Vec::new();
    let mut push = |ord, label, s: &Option<RawSession>| {
        if let Some(x) = s {
            out.push(Session {
                ord,
                label,
                date: x.date.clone(),
                time: x.time.clone(),
            });
        }
    };
    push(1, "Practice 1", &r.fp1);
    push(2, "Practice 2", &r.fp2);
    push(3, "Practice 3", &r.fp3);
    push(4, "Sprint Qualifying", &r.sprint_quali);
    push(5, "Sprint", &r.sprint);
    push(6, "Qualifying", &r.qualifying);
    // The race itself lives on the race object's own date/time.
    out.push(Session {
        ord: 7,
        label: "Race",
        date: r.date.clone(),
        time: r.time.clone(),
    });
    out
}

fn to_matches(r: &RawRace, now: DateTime<Utc>) -> Vec<NormalizedMatch> {
    let season: i64 = r.season.parse().unwrap_or(0);
    let round: i64 = r.round.parse().unwrap_or(0);
    // The circuit's local timezone, so the row's time is clickable to show the
    // local time at the track (like the venue time on other sports).
    let venue_tz = circuit_tz(&r.circuit.circuit_id).map(str::to_string);
    sessions(r)
        .into_iter()
        .filter_map(|s| {
            let begin_at = parse_dt(&s.date, s.time.as_deref())?;
            // No live feed; a session is "done" a few hours after it starts
            // (race ~2h, others ~1h). The view layer shows it as live in between.
            let status = if begin_at + Duration::hours(3) < now {
                MatchStatus::Finished
            } else {
                MatchStatus::Upcoming
            };
            let mut m = NormalizedMatch::team_sport(
                // Stable, collision-free id from (season, round, session order).
                season * 100_000 + round * 100 + s.ord,
                Sport::Motorsport,
                "F1",
                begin_at,
                status,
                // Single-entity: the one label is the session; no opponent.
                NormalizedTeam {
                    label: s.label.to_string(),
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
            // The Grand Prix is the event, qualified by season so each year's
            // edition is distinct (e.g. "Austrian Grand Prix 2026") — the same GP
            // recurs annually and must not collide across seasons.
            m.series_name = format!("{} {season}", r.race_name);
            m.venue_tz = venue_tz.clone();
            m.venue_name = r.circuit.circuit_name.clone();
            m.venue_location = r.circuit.location.label();
            // F1 has no teams; use the GP host country's flag as the session icon.
            m.team_a_logo = country_flag(&r.circuit.location.country);
            m.league_url = race_page_url(&r.circuit.circuit_id, season, &r.url);
            Some(m)
        })
        .collect()
}

/// Fetch the season's calendar and expand every Grand Prix into its sessions.
/// One keyless request; Jolpica is rate-limited, so the poller caches the result.
pub async fn fetch_schedule(
    client: &reqwest::Client,
    season: i32,
) -> Result<Vec<NormalizedMatch>, reqwest::Error> {
    let url = format!("{BASE}/{season}.json?limit=100");
    let resp: Resp = client
        .get(&url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let now = Utc::now();
    Ok(resp
        .data
        .race_table
        .races
        .iter()
        .flat_map(|r| to_matches(r, now))
        .collect())
}

// ----- Results (full finishing order, for the GP event page) ----------------

#[derive(Deserialize)]
struct ResultsResp {
    #[serde(rename = "MRData")]
    data: ResultsMr,
}
#[derive(Deserialize)]
struct ResultsMr {
    #[serde(rename = "RaceTable")]
    table: ResultsTable,
}
#[derive(Deserialize)]
struct ResultsTable {
    #[serde(rename = "Races", default)]
    races: Vec<ResultsRace>,
}
#[derive(Deserialize, Default)]
struct ResultsRace {
    // The race date, also present on the schedule's race object. Read here so the
    // OpenF1 weekend-match date comes free with the results instead of costing a
    // separate round lookup.
    #[serde(rename = "date", default)]
    date: String,
    #[serde(rename = "Results", default)]
    results: Vec<RawResult>,
    #[serde(rename = "SprintResults", default)]
    sprint: Vec<RawResult>,
    #[serde(rename = "QualifyingResults", default)]
    qualifying: Vec<RawQuali>,
}
#[derive(Deserialize)]
struct RawDriver {
    #[serde(rename = "givenName", default)]
    given: String,
    #[serde(rename = "familyName", default)]
    family: String,
    #[serde(default)]
    nationality: String,
}
#[derive(Deserialize)]
struct RawConstructor {
    #[serde(rename = "constructorId", default)]
    constructor_id: String,
    #[serde(default)]
    name: String,
}

/// A driver's nationality flag (flagcdn SVG) by Ergast demonym; empty if unmapped.
fn nationality_flag(nationality: &str) -> String {
    let code = match nationality {
        "British" => "gb",
        "Dutch" => "nl",
        "Spanish" => "es",
        "Monegasque" => "mc",
        "Mexican" => "mx",
        "Australian" => "au",
        "Canadian" => "ca",
        "French" => "fr",
        "German" => "de",
        "Finnish" => "fi",
        "Danish" => "dk",
        "Japanese" => "jp",
        "Thai" => "th",
        "Chinese" => "cn",
        "American" => "us",
        "Italian" => "it",
        "Argentine" | "Argentinian" => "ar",
        "Brazilian" => "br",
        "Austrian" => "at",
        "Belgian" => "be",
        "Swiss" => "ch",
        "New Zealander" => "nz",
        "Russian" => "ru",
        _ => return String::new(),
    };
    format!("https://flagcdn.com/{code}.svg")
}

/// A constructor's logo (F1's media CDN, ~1KB PNG) by Ergast `constructorId`. The
/// CDN slug + year aren't a clean 1:1 with the id, so they're mapped here; teams
/// new in 2026 (Audi, Cadillac) have no CDN logo yet, so they get none.
fn constructor_logo(constructor_id: &str) -> String {
    // Maps the Ergast/Jolpica constructorId to F1's own 2026 logo slug. We use the
    // transparent "white" silhouette (the alternative is colour, but several 2026
    // logos — Audi, Cadillac, Mercedes — are near-black and vanish on the dark
    // theme); it's rendered as a `currentColor` CSS mask so it adapts to the theme
    // and is always visible. Covers the full 2026 grid, including the new teams.
    let slug = match constructor_id {
        "red_bull" => "redbullracing",
        "rb" | "racing_bulls" => "racingbulls",
        // The Sauber entry becomes Audi for 2026; accept either id.
        "sauber" | "audi" => "audi",
        "cadillac" => "cadillac",
        "aston_martin" => "astonmartin",
        "mercedes" => "mercedes",
        "ferrari" => "ferrari",
        "mclaren" => "mclaren",
        "alpine" => "alpine",
        "williams" => "williams",
        "haas" => "haasf1team",
        _ => return String::new(),
    };
    format!(
        "https://media.formula1.com/image/upload/c_lfill,w_64/q_auto/common/f1/2026/{slug}/2026{slug}logowhite.webp"
    )
}

/// A constructor's short code by Ergast `constructorId` (e.g. "FER"), shown in the
/// tables where the full name won't fit. Empty when unmapped.
fn constructor_abbrev(constructor_id: &str) -> String {
    match constructor_id {
        "red_bull" => "RBR",
        "rb" | "racing_bulls" => "RB",
        "sauber" | "audi" => "AUD",
        "cadillac" => "CAD",
        "aston_martin" => "AMR",
        "mercedes" => "MER",
        "ferrari" => "FER",
        "mclaren" => "MCL",
        "alpine" => "ALP",
        "williams" => "WIL",
        "haas" => "HAA",
        _ => "",
    }
    .to_string()
}

#[derive(Deserialize, Default)]
struct RawTime {
    #[serde(default)]
    time: String,
}
#[derive(Deserialize)]
struct RawResult {
    #[serde(default)]
    position: String,
    #[serde(rename = "Driver")]
    driver: RawDriver,
    #[serde(rename = "Constructor")]
    constructor: RawConstructor,
    #[serde(default)]
    status: String,
    #[serde(rename = "Time", default)]
    time: Option<RawTime>,
}
#[derive(Deserialize)]
struct RawQuali {
    #[serde(default)]
    position: String,
    #[serde(rename = "Driver")]
    driver: RawDriver,
    #[serde(rename = "Constructor")]
    constructor: RawConstructor,
    #[serde(rename = "Q1", default)]
    q1: Option<String>,
    #[serde(rename = "Q2", default)]
    q2: Option<String>,
    #[serde(rename = "Q3", default)]
    q3: Option<String>,
}

fn driver_name(d: &RawDriver) -> String {
    format!("{} {}", d.given, d.family).trim().to_string()
}

fn race_rows(rs: &[RawResult]) -> Vec<F1ResultRow> {
    rs.iter()
        .map(|r| F1ResultRow {
            pos: r.position.clone(),
            driver: driver_name(&r.driver),
            constructor: r.constructor.name.clone(),
            // The finishing time/gap when classified, else the status (lapped /
            // DNF reason).
            detail: r
                .time
                .as_ref()
                .map(|t| t.time.clone())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| r.status.clone()),
            flag: nationality_flag(&r.driver.nationality),
            constructor_logo: constructor_logo(&r.constructor.constructor_id),
            constructor_abbrev: constructor_abbrev(&r.constructor.constructor_id),
        })
        .collect()
}

fn quali_rows(rs: &[RawQuali]) -> Vec<F1ResultRow> {
    rs.iter()
        .map(|q| F1ResultRow {
            pos: q.position.clone(),
            driver: driver_name(&q.driver),
            constructor: q.constructor.name.clone(),
            // Best lap of the session (the pole time for P1).
            detail: q
                .q3
                .clone()
                .or_else(|| q.q2.clone())
                .or_else(|| q.q1.clone())
                .unwrap_or_default(),
            flag: nationality_flag(&q.driver.nationality),
            constructor_logo: constructor_logo(&q.constructor.constructor_id),
            constructor_abbrev: constructor_abbrev(&q.constructor.constructor_id),
        })
        .collect()
}

/// The Grand Prix's race date, used to match the OpenF1 weekend that carries the
/// practice timing (OpenF1 has no round number and a different meeting order).
async fn round_date(client: &reqwest::Client, season: i64, round: i64) -> Option<NaiveDate> {
    let url = format!("{BASE}/{season}/{round}.json");
    let resp: Resp = client
        .get(&url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .await
        .ok()?;
    let race = resp.data.race_table.races.into_iter().next()?;
    NaiveDate::parse_from_str(race.date.get(..10)?, "%Y-%m-%d").ok()
}

async fn get_results(client: &reqwest::Client, url: &str) -> Option<ResultsRace> {
    let resp: ResultsResp = client
        .get(url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .await
        .ok()?;
    resp.data.table.races.into_iter().next()
}

/// A Grand Prix's finished-session results as full finishing orders — Race, then
/// Sprint, then Qualifying. Missing/empty sessions (an upcoming race, or a
/// non-sprint weekend) are simply absent; any fetch error yields no section.
pub async fn fetch_results(client: &reqwest::Client, season: i64, round: i64) -> Vec<F1Result> {
    let base = format!("{BASE}/{season}/{round}");
    let (race_url, sprint_url, quali_url) = (
        format!("{base}/results.json?limit=40"),
        format!("{base}/sprint.json?limit=40"),
        format!("{base}/qualifying.json?limit=40"),
    );
    let (race, sprint, quali) = tokio::join!(
        get_results(client, &race_url),
        get_results(client, &sprint_url),
        get_results(client, &quali_url),
    );
    // The race date (for OpenF1 weekend-matching) rides along in any of the
    // results payloads, so take it from there rather than spending a 4th request.
    let date_from_results = [&race, &sprint, &quali]
        .into_iter()
        .flatten()
        .find_map(|r| NaiveDate::parse_from_str(r.date.get(..10)?, "%Y-%m-%d").ok());
    let mut out = Vec::new();
    let mut push = |session: &str, rows: Vec<F1ResultRow>| {
        if !rows.is_empty() {
            out.push(F1Result {
                session: session.to_string(),
                rows,
            });
        }
    };
    push(
        "Race",
        race.map(|r| race_rows(&r.results)).unwrap_or_default(),
    );
    push(
        "Sprint",
        sprint.map(|s| race_rows(&s.sprint)).unwrap_or_default(),
    );
    push(
        "Qualifying",
        quali.map(|q| quali_rows(&q.qualifying)).unwrap_or_default(),
    );
    // Practice timing (FP1/FP2/FP3) isn't in Jolpica — pull it from OpenF1 and
    // append it under qualifying. Matched to the OpenF1 weekend by the race date,
    // taken from the results above; only an upcoming GP with no results yet falls
    // back to the dedicated round lookup.
    let date = match date_from_results {
        Some(d) => Some(d),
        None => round_date(client, season, round).await,
    };
    if let Some(date) = date {
        out.extend(crate::openf1::fetch_practice(client, season, date).await);
    }
    out
}

// ----- Championship standings (for the GP event page) ------------------------

#[derive(Deserialize)]
struct StandingsResp {
    #[serde(rename = "MRData")]
    data: StandingsMr,
}
#[derive(Deserialize)]
struct StandingsMr {
    #[serde(rename = "StandingsTable")]
    table: StandingsTable,
}
#[derive(Deserialize)]
struct StandingsTable {
    #[serde(rename = "StandingsLists", default)]
    lists: Vec<StandingsList>,
}
#[derive(Deserialize, Default)]
struct StandingsList {
    #[serde(default)]
    round: String,
    #[serde(rename = "DriverStandings", default)]
    drivers: Vec<RawDriverStanding>,
    #[serde(rename = "ConstructorStandings", default)]
    constructors: Vec<RawConstructorStanding>,
}
#[derive(Deserialize)]
struct RawDriverStanding {
    #[serde(default)]
    position: String,
    #[serde(default)]
    points: String,
    #[serde(default)]
    wins: String,
    #[serde(rename = "Driver")]
    driver: RawDriver,
    #[serde(rename = "Constructors", default)]
    constructors: Vec<RawConstructor>,
}
#[derive(Deserialize)]
struct RawConstructorStanding {
    #[serde(default)]
    position: String,
    #[serde(default)]
    points: String,
    #[serde(default)]
    wins: String,
    #[serde(rename = "Constructor")]
    constructor: RawConstructor,
}

fn driver_standing_rows(rs: &[RawDriverStanding]) -> Vec<F1StandingRow> {
    rs.iter()
        .map(|r| {
            let team = r.constructors.first();
            F1StandingRow {
                pos: r.position.clone(),
                name: driver_name(&r.driver),
                detail: team.map(|c| c.name.clone()).unwrap_or_default(),
                points: r.points.clone(),
                wins: r.wins.clone(),
                flag: nationality_flag(&r.driver.nationality),
                constructor_logo: team
                    .map(|c| constructor_logo(&c.constructor_id))
                    .unwrap_or_default(),
                constructor_abbrev: team
                    .map(|c| constructor_abbrev(&c.constructor_id))
                    .unwrap_or_default(),
            }
        })
        .collect()
}

fn constructor_standing_rows(rs: &[RawConstructorStanding]) -> Vec<F1StandingRow> {
    rs.iter()
        .map(|r| F1StandingRow {
            pos: r.position.clone(),
            name: r.constructor.name.clone(),
            detail: String::new(),
            points: r.points.clone(),
            wins: r.wins.clone(),
            flag: String::new(),
            constructor_logo: constructor_logo(&r.constructor.constructor_id),
            constructor_abbrev: constructor_abbrev(&r.constructor.constructor_id),
        })
        .collect()
}

async fn get_standings_list(client: &reqwest::Client, url: &str) -> Option<StandingsList> {
    let resp: StandingsResp = client
        .get(url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json()
        .await
        .ok()?;
    resp.data.table.lists.into_iter().next()
}

/// The drivers' and constructors' championship standings as of a Grand Prix's
/// round. For a finished GP these reflect the round; for an upcoming one (the
/// round isn't raced yet, so Jolpica returns nothing) they fall back to the
/// latest completed round — the "going into the weekend" picture. Empty on a
/// fetch error.
pub async fn fetch_standings(client: &reqwest::Client, season: i64, round: i64) -> F1Standings {
    let drv = format!("{BASE}/{season}/{round}/driverStandings.json?limit=40");
    let con = format!("{BASE}/{season}/{round}/constructorStandings.json?limit=40");
    let (mut drivers, mut constructors) = tokio::join!(
        get_standings_list(client, &drv),
        get_standings_list(client, &con)
    );
    // An unraced round has no standings yet — use the latest completed instead.
    if drivers.as_ref().is_none_or(|l| l.drivers.is_empty()) {
        let drv2 = format!("{BASE}/{season}/driverStandings.json?limit=40");
        let con2 = format!("{BASE}/{season}/constructorStandings.json?limit=40");
        let (d2, c2) = tokio::join!(
            get_standings_list(client, &drv2),
            get_standings_list(client, &con2)
        );
        drivers = d2;
        constructors = c2;
    }
    let asof = drivers
        .as_ref()
        .and_then(|l| l.round.parse().ok())
        .unwrap_or(round);
    F1Standings {
        round: asof,
        drivers: drivers
            .map(|l| driver_standing_rows(&l.drivers))
            .unwrap_or_default(),
        constructors: constructors
            .map(|l| constructor_standing_rows(&l.constructors))
            .unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_a_weekend_into_sessions() {
        // A regular (non-sprint) weekend: 3 practices, qualifying, race.
        let json = r#"{"MRData":{"RaceTable":{"Races":[{
          "season":"2026","round":"11","raceName":"Austrian Grand Prix",
          "Circuit":{"circuitName":"Red Bull Ring","Location":{"locality":"Spielberg","country":"Austria"}},
          "date":"2026-06-28","time":"13:00:00Z",
          "FirstPractice":{"date":"2026-06-26","time":"11:30:00Z"},
          "SecondPractice":{"date":"2026-06-26","time":"15:00:00Z"},
          "ThirdPractice":{"date":"2026-06-27","time":"10:30:00Z"},
          "Qualifying":{"date":"2026-06-27","time":"14:00:00Z"}
        }]}}}"#;
        let resp: Resp = serde_json::from_str(json).unwrap();
        let now = "2026-01-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let ms: Vec<NormalizedMatch> = resp
            .data
            .race_table
            .races
            .iter()
            .flat_map(|r| to_matches(r, now))
            .collect();
        // FP1, FP2, FP3, Qualifying, Race — no sprint sessions this weekend.
        assert_eq!(ms.len(), 5);
        assert!(ms
            .iter()
            .all(|m| m.sport == Sport::Motorsport && m.league == "F1"));
        // The series is the GP qualified by season, so editions don't collide.
        assert!(ms
            .iter()
            .all(|m| m.series_name == "Austrian Grand Prix 2026"));
        let labels: Vec<&str> = ms.iter().map(|m| m.team_a.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Practice 1",
                "Practice 2",
                "Practice 3",
                "Qualifying",
                "Race"
            ]
        );
        // Every session is in the future relative to `now`, so all upcoming.
        assert!(ms.iter().all(|m| m.status == MatchStatus::Upcoming));
        // The race session carries the race object's own time (13:00Z on the 28th).
        let race = ms.iter().find(|m| m.team_a.label == "Race").unwrap();
        assert_eq!(race.begin_at.to_rfc3339(), "2026-06-28T13:00:00+00:00");
        // Ids are stable and distinct per session.
        assert_eq!(race.id, 2026 * 100_000 + 11 * 100 + 7);
    }

    #[test]
    fn sprint_weekend_has_sprint_sessions() {
        let json = r#"{"MRData":{"RaceTable":{"Races":[{
          "season":"2026","round":"5","raceName":"Chinese Grand Prix",
          "Circuit":{"circuitName":"Shanghai","Location":{"locality":"Shanghai","country":"China"}},
          "date":"2026-03-15","time":"07:00:00Z",
          "FirstPractice":{"date":"2026-03-13","time":"03:30:00Z"},
          "SprintQualifying":{"date":"2026-03-13","time":"07:30:00Z"},
          "Sprint":{"date":"2026-03-14","time":"03:00:00Z"},
          "Qualifying":{"date":"2026-03-14","time":"07:00:00Z"}
        }]}}}"#;
        let resp: Resp = serde_json::from_str(json).unwrap();
        let now = "2026-01-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let ms: Vec<NormalizedMatch> = resp
            .data
            .race_table
            .races
            .iter()
            .flat_map(|r| to_matches(r, now))
            .collect();
        let labels: Vec<&str> = ms.iter().map(|m| m.team_a.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Practice 1",
                "Sprint Qualifying",
                "Sprint",
                "Qualifying",
                "Race"
            ]
        );
    }

    #[test]
    fn parses_driver_and_constructor_standings() {
        let drv = r#"{"MRData":{"StandingsTable":{"StandingsLists":[{"round":"7",
          "DriverStandings":[
            {"position":"1","points":"156","wins":"5","Driver":{"givenName":"Kimi","familyName":"Antonelli"},"Constructors":[{"name":"Mercedes"}]},
            {"position":"2","points":"115","wins":"1","Driver":{"givenName":"Lewis","familyName":"Hamilton"},"Constructors":[{"name":"Ferrari"}]}
          ]}]}}}"#;
        let resp: StandingsResp = serde_json::from_str(drv).unwrap();
        let list = resp.data.table.lists.into_iter().next().unwrap();
        assert_eq!(list.round, "7");
        let rows = driver_standing_rows(&list.drivers);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "Kimi Antonelli");
        assert_eq!(rows[0].detail, "Mercedes");
        assert_eq!(rows[0].points, "156");

        let con = r#"{"MRData":{"StandingsTable":{"StandingsLists":[{"round":"7",
          "ConstructorStandings":[
            {"position":"1","points":"245","wins":"4","Constructor":{"name":"Mercedes"}}
          ]}]}}}"#;
        let resp: StandingsResp = serde_json::from_str(con).unwrap();
        let list = resp.data.table.lists.into_iter().next().unwrap();
        let rows = constructor_standing_rows(&list.constructors);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "Mercedes");
        assert_eq!(rows[0].detail, "");
        assert_eq!(rows[0].points, "245");
    }

    #[test]
    fn race_url_prefers_formula1_for_mapped_circuits() {
        let u = race_page_url(
            "silverstone",
            2026,
            "https://en.wikipedia.org/wiki/2026_British_Grand_Prix",
        );
        assert_eq!(
            u.as_deref(),
            Some("https://www.formula1.com/en/racing/2026/great-britain")
        );
        let u2 = race_page_url("yas_marina", 2026, "https://en.wikipedia.org/wiki/x");
        assert_eq!(
            u2.as_deref(),
            Some("https://www.formula1.com/en/racing/2026/united-arab-emirates")
        );
    }

    #[test]
    fn race_url_falls_back_to_wikipedia_for_unmapped_circuits() {
        // A circuit not in the slug map (e.g. the new 2026 Madrid round) → Wikipedia.
        let u = race_page_url(
            "madring",
            2026,
            "https://en.wikipedia.org/wiki/2026_Madrid_Grand_Prix",
        );
        assert_eq!(
            u.as_deref(),
            Some("https://en.wikipedia.org/wiki/2026_Madrid_Grand_Prix")
        );
        // Unmapped AND no Wikipedia url → None.
        assert_eq!(race_page_url("madring", 2026, ""), None);
    }
}
