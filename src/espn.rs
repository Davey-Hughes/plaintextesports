//! NFL + NBA schedule and standings from ESPN's keyless public API
//! (`site.api.espn.com`). Both leagues share one shape — a date-range scoreboard
//! for the schedule and conference-grouped standings — so this one module serves
//! both, parameterized by [`EspnLeague`]. Normalized into the same
//! [`NormalizedMatch`] model as MLB/NHL (two teams, away at home).
//!
//! ESPN doesn't expose a venue IANA timezone, so these games carry no venue-time
//! toggle (like esports); the start time still shows in the viewer's zone.

use crate::bracket_build::{self, RawSeries};
use crate::feed::{NormalizedMatch, NormalizedTeam};
use crate::types::{
    stat_share, BoxScore, BracketRound, EventInfo, LeaderCard, LineRow, LineScore, MatchStatus,
    PlayerRow, PlayerTable, ScoreEvent, Sport, StandingRow, StatPair, StreamView,
};
use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, Utc};
use serde::Deserialize;
use std::time::Duration;

const SITE: &str = "https://site.api.espn.com/apis/site/v2/sports";
const CORE: &str = "https://site.api.espn.com/apis/v2/sports";

/// One ESPN-backed league (NFL or NBA) and the bits that differ between them.
pub struct EspnLeague {
    pub sport: Sport,
    pub name: &'static str,
    /// ESPN URL path parts, e.g. ("football", "nfl").
    espn_sport: &'static str,
    league: &'static str,
    /// The standings stat shown in the last table column (its `displayValue`).
    last_stat: &'static str,
    /// The numeric stat the table is ordered by, descending (wins for the North
    /// American leagues, points for soccer).
    rank_stat: &'static str,
    /// Base id for the conference standings tables (id = base + conference index).
    standings_base: i64,
    /// Whether this competition recurs as dated editions rather than one ongoing
    /// season — the World Cup, whose series carries the year so 2026 and 2030 are
    /// distinct events. The seasonal leagues (NFL/NBA/PL) leave their series empty.
    dated_event: bool,
}

pub const NFL: EspnLeague = EspnLeague {
    sport: Sport::Nfl,
    name: "NFL",
    espn_sport: "football",
    league: "nfl",
    last_stat: "winPercent",
    rank_stat: "wins",
    standings_base: 500,
    dated_event: false,
};

pub const NBA: EspnLeague = EspnLeague {
    sport: Sport::Nba,
    name: "NBA",
    espn_sport: "basketball",
    league: "nba",
    last_stat: "gamesBehind",
    rank_stat: "wins",
    standings_base: 400,
    dated_event: false,
};

pub const EPL: EspnLeague = EspnLeague {
    sport: Sport::Soccer,
    name: "Premier League",
    espn_sport: "soccer",
    league: "eng.1",
    // Soccer ranks by points; the table shows W-D-L (draws ride the ties slot).
    last_stat: "points",
    rank_stat: "points",
    standings_base: 700,
    dated_event: false,
};

pub const WORLD_CUP: EspnLeague = EspnLeague {
    sport: Sport::Soccer,
    name: "World Cup",
    espn_sport: "soccer",
    league: "fifa.world",
    last_stat: "points",
    rank_stat: "points",
    standings_base: 600,
    dated_event: true,
};

/// ESPN's season year for a European league (Aug–May; labelled by start year).
#[must_use]
pub fn european_season(now: DateTime<Utc>) -> i32 {
    if now.month() >= 7 {
        now.year()
    } else {
        now.year() - 1
    }
}

/// ESPN's season year for a league at `now`. NFL labels a season by its start
/// year (Sept–Feb); NBA by its end year (Oct–June).
#[must_use]
pub fn season_year(sport: Sport, now: DateTime<Utc>) -> i32 {
    let (y, m) = (now.year(), now.month());
    match sport {
        Sport::Nfl => {
            if m >= 9 {
                y
            } else {
                y - 1
            }
        }
        // NBA (and anything else ESPN-backed): end-year convention.
        _ => {
            if m >= 10 {
                y + 1
            } else {
                y
            }
        }
    }
}

// ----- Schedule --------------------------------------------------------------

#[derive(Deserialize)]
struct Scoreboard {
    #[serde(default)]
    events: Vec<Event>,
}

#[derive(Deserialize)]
struct Event {
    #[serde(default)]
    id: String,
    #[serde(default)]
    date: String,
    #[serde(default)]
    status: Status,
    #[serde(default)]
    competitions: Vec<Competition>,
    /// The season block: `type` distinguishes the round (a numeric stage id for
    /// soccer/NFL/NBA postseasons). Only used by the bracket builders. (The
    /// per-side feeder placeholders live on each competitor's `displayName`.)
    #[serde(default)]
    season: SeasonRef,
    /// The postseason week (NFL: 1 Wild Card … 5 Super Bowl). Bracket builders only.
    #[serde(default)]
    week: WeekRef,
}

#[derive(Deserialize, Default)]
struct SeasonRef {
    #[serde(rename = "type", default)]
    kind: i64,
}

#[derive(Deserialize, Default)]
struct WeekRef {
    #[serde(default)]
    number: i64,
}

#[derive(Deserialize, Default)]
struct Status {
    #[serde(default)]
    r#type: StatusType,
}

#[derive(Deserialize, Default)]
struct StatusType {
    #[serde(default)]
    state: String,
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
struct Competition {
    #[serde(default)]
    competitors: Vec<Competitor>,
    #[serde(default)]
    venue: Venue,
    #[serde(rename = "geoBroadcasts", default)]
    geo_broadcasts: Vec<GeoBroadcast>,
    /// Round headlines, e.g. "AFC Wild Card Playoffs", "East 1st Round - Game 6",
    /// "NBA Finals". Bracket builders only.
    #[serde(default)]
    notes: Vec<Note>,
    /// The embedded playoff-series summary (NBA: games-won per team, best-of
    /// length, completion). Absent outside a series. Bracket builders only.
    #[serde(default)]
    series: Option<SeriesRef>,
}

#[derive(Deserialize, Default)]
struct Note {
    #[serde(default)]
    headline: String,
}

#[derive(Deserialize, Default)]
struct SeriesRef {
    #[serde(default)]
    completed: bool,
    #[serde(default)]
    competitors: Vec<SeriesCompetitor>,
}

#[derive(Deserialize, Default)]
struct SeriesCompetitor {
    #[serde(default)]
    id: String,
    #[serde(default)]
    wins: i64,
}

#[derive(Deserialize, Default)]
struct GeoBroadcast {
    #[serde(rename = "type", default)]
    kind: GeoKind,
    #[serde(default)]
    market: GeoMarket,
    #[serde(default)]
    media: GeoMedia,
}

#[derive(Deserialize, Default)]
struct GeoKind {
    #[serde(rename = "shortName", default)]
    short_name: String, // "TV" | "Radio" | "Streaming" | …
}

#[derive(Deserialize, Default)]
struct GeoMarket {
    #[serde(rename = "type", default)]
    kind: String, // "National" | "Home" | "Away"
}

#[derive(Deserialize, Default)]
struct GeoMedia {
    #[serde(rename = "shortName", default)]
    short_name: String, // network name
}

#[derive(Deserialize, Default)]
struct Venue {
    #[serde(rename = "fullName", default)]
    full_name: String,
    #[serde(default)]
    address: VenueAddress,
}

#[derive(Deserialize, Default)]
struct VenueAddress {
    #[serde(default)]
    city: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    country: String,
}

impl Venue {
    /// "City, State" (US) or "City, Country", whichever the feed provides.
    fn location(&self) -> String {
        let a = &self.address;
        let region = if a.state.is_empty() {
            &a.country
        } else {
            &a.state
        };
        match (a.city.is_empty(), region.is_empty()) {
            (false, false) => format!("{}, {region}", a.city),
            (false, true) => a.city.clone(),
            _ => String::new(),
        }
    }
}

#[derive(Deserialize)]
struct Competitor {
    #[serde(rename = "homeAway", default)]
    home_away: String,
    #[serde(default)]
    score: String,
    #[serde(default)]
    team: TeamRef,
    /// Whether this side won (set on a finished game/series). Bracket builders only.
    #[serde(default)]
    winner: bool,
}

#[derive(Deserialize, Default)]
struct TeamRef {
    /// ESPN team id (matches `series.competitors[].id`). Bracket builders only.
    #[serde(default)]
    id: String,
    #[serde(rename = "shortDisplayName", default)]
    short_display_name: String,
    #[serde(rename = "displayName", default)]
    display_name: String,
    #[serde(default)]
    abbreviation: String,
    /// Full-size (500px) raster logo URL, e.g.
    /// "https://a.espncdn.com/i/teamlogos/nba/500/orl.png".
    #[serde(default)]
    logo: String,
}

impl TeamRef {
    fn label(&self) -> String {
        if !self.short_display_name.is_empty() {
            self.short_display_name.clone()
        } else {
            self.abbreviation.clone()
        }
    }
    fn full_name(&self) -> String {
        if self.display_name.is_empty() {
            self.label()
        } else {
            self.display_name.clone()
        }
    }
}

/// ESPN's logos default to 500px; request a small (80px, crisp at 2× DPI) copy
/// through its image combiner so we ship ~5KB, not ~70KB. Empty if not an ESPN
/// team-logo URL.
fn espn_logo(url: &str) -> String {
    match url.find("/i/teamlogos/") {
        Some(i) => format!(
            "https://a.espncdn.com/combiner/i?img={}&h=80&w=80",
            &url[i..]
        ),
        None => String::new(),
    }
}

/// A best-effort IANA timezone for a US/Canada/Mexico venue, from its address —
/// ESPN exposes no tz, so the schedule's venue-time toggle needs this. Covers the
/// host states/cities of the leagues we show (World Cup, NBA, NFL); `None` (no
/// toggle) for anywhere not mapped.
fn venue_tz(city: &str, country: &str) -> Option<&'static str> {
    match country {
        // US venues read "City, State"; the state fixes the zone.
        "USA" => {
            let state = city.rsplit(',').next().map(str::trim).unwrap_or_default();
            Some(match state {
                "California" | "Washington" | "Oregon" | "Nevada" => "America/Los_Angeles",
                "Arizona" => "America/Phoenix", // no DST
                "Colorado" | "Utah" => "America/Denver",
                "Texas" | "Missouri" | "Illinois" | "Minnesota" | "Wisconsin" | "Louisiana"
                | "Oklahoma" | "Tennessee" | "Kansas" | "Nebraska" => "America/Chicago",
                "Pennsylvania"
                | "Florida"
                | "New Jersey"
                | "New York"
                | "Georgia"
                | "Massachusetts"
                | "Michigan"
                | "Ohio"
                | "North Carolina"
                | "Indiana"
                | "District of Columbia"
                | "Virginia"
                | "South Carolina"
                | "Maryland" => "America/New_York",
                _ => return None,
            })
        }
        "Canada" => Some(match city {
            "Toronto" | "Montreal" | "Ottawa" => "America/Toronto",
            "Vancouver" => "America/Vancouver",
            "Calgary" | "Edmonton" => "America/Edmonton",
            "Winnipeg" => "America/Winnipeg",
            _ => return None,
        }),
        "Mexico" => Some("America/Mexico_City"),
        _ => None,
    }
}

/// Parse an ESPN timestamp, which omits seconds ("2026-01-15T19:00Z").
fn parse_date(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%MZ")
                .ok()
                .map(|n| n.and_utc())
        })
}

fn status_of(state: &str, name: &str) -> MatchStatus {
    let n = name.to_ascii_uppercase();
    if n.contains("CANCEL") || n.contains("POSTPON") || n.contains("FORFEIT") {
        return MatchStatus::Canceled;
    }
    match state {
        "post" => MatchStatus::Finished,
        "in" => MatchStatus::Live,
        _ => MatchStatus::Upcoming, // "pre"
    }
}

/// Sort key so the most useful "where to watch" entries lead: video before
/// radio, national before local (home before away).
fn geo_rank(g: &GeoBroadcast) -> (i32, i32) {
    let radio = if g.kind.short_name.eq_ignore_ascii_case("Radio") {
        1
    } else {
        0
    };
    let side = match g.market.kind.as_str() {
        "National" => 0,
        "Home" => 1,
        "Away" => 2,
        _ => 3,
    };
    (radio, side)
}

/// Turn ESPN's `geoBroadcasts` into [`StreamView`]s: national networks carry a
/// watch link, local RSNs / radio are text (ESPN provides no per-network URL).
/// Deduped by name+medium; grouped video (`tv`) then radio. Radio is the only
/// non-video medium; TV/Streaming/Web all read as watchable video.
fn broadcasts(raw: &[GeoBroadcast]) -> Vec<StreamView> {
    let mut sorted: Vec<&GeoBroadcast> = raw.iter().collect();
    sorted.sort_by_key(|g| geo_rank(g));
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out = Vec::new();
    for g in sorted {
        let name = g.media.short_name.trim();
        if name.is_empty() {
            continue;
        }
        let is_radio = g.kind.short_name.eq_ignore_ascii_case("Radio");
        let medium = if is_radio { "radio" } else { "TV" };
        if !seen.insert(format!("{name}|{medium}")) {
            continue;
        }
        let (scope, national) = match g.market.kind.as_str() {
            "National" => ("national", true),
            "Home" => ("home", false),
            "Away" => ("away", false),
            _ => ("", false),
        };
        let tag = if scope.is_empty() {
            medium.to_string()
        } else {
            format!("{scope} · {medium}")
        };
        let url = if national {
            crate::watch::national_watch_url(name)
                .unwrap_or_default()
                .to_string()
        } else {
            String::new()
        };
        out.push(StreamView {
            url,
            language: String::new(),
            official: national,
            main: !is_radio,
            name: name.to_string(),
            tag,
            group: if is_radio { "radio" } else { "tv" }.to_string(),
            ..Default::default()
        });
    }
    out
}

/// Every Prime Video NFL game (all TNF + Amazon's Black Friday/playoff games) is
/// simulcast free on Amazon's Twitch channel; no feed flags it, so add it when an
/// NFL game already carries a Prime/Amazon broadcast.
fn append_tnf_twitch(streams: &mut Vec<StreamView>, sport: Sport) {
    // Only NFL games get the Prime→Twitch simulcast; gate before the scan so
    // NHL/NBA (the bulk of ESPN games) don't pay the per-stream uppercase allocs.
    if sport != Sport::Nfl {
        return;
    }
    let has_prime = streams.iter().any(|s| {
        let n = s.name.to_ascii_uppercase();
        n.contains("PRIME") || n.contains("AMAZON")
    });
    let has_twitch = streams
        .iter()
        .any(|s| s.name.eq_ignore_ascii_case("Twitch"));
    if has_prime && !has_twitch {
        streams.push(StreamView {
            url: "https://www.twitch.tv/primevideo".to_string(),
            official: true,
            main: true,
            name: "Twitch".to_string(),
            tag: "national · streaming".to_string(),
            group: "tv".to_string(),
            ..Default::default()
        });
    }
}

fn to_match(e: Event, lg: &EspnLeague) -> Option<NormalizedMatch> {
    let id = e.id.parse::<i64>().ok()?;
    let begin_at = parse_date(&e.date)?;
    let comp = e.competitions.into_iter().next()?;
    let away = comp.competitors.iter().find(|c| c.home_away == "away")?;
    let home = comp.competitors.iter().find(|c| c.home_away == "home")?;
    let score = |c: &Competitor| c.score.parse::<i64>().ok();
    // ESPN exposes no venue IANA tz, so these carry no venue-time toggle.
    let mut m = NormalizedMatch::team_sport(
        id,
        lg.sport,
        lg.name,
        begin_at,
        status_of(&e.status.r#type.state, &e.status.r#type.name),
        NormalizedTeam {
            label: away.team.label(),
            name: away.team.full_name(),
            abbrev: away.team.abbreviation.clone(),
            score: score(away),
        },
        NormalizedTeam {
            label: home.team.label(),
            name: home.team.full_name(),
            abbrev: home.team.abbreviation.clone(),
            score: score(home),
        },
    );
    m.venue_name = comp.venue.full_name.clone();
    m.venue_location = comp.venue.location();
    m.team_a_logo = espn_logo(&away.team.logo);
    m.team_b_logo = espn_logo(&home.team.logo);
    // ESPN gives no venue tz; derive one (where known) so the schedule's
    // venue-time toggle works — notably for the World Cup's US/Canada/Mexico hosts.
    m.venue_tz =
        venue_tz(&comp.venue.address.city, &comp.venue.address.country).map(str::to_string);
    // A dated tournament (the World Cup) carries its year as the series, so each
    // edition is a distinct event; the seasonal leagues keep an empty series.
    if lg.dated_event {
        m.series_name = begin_at.year().to_string();
    }
    m.streams = broadcasts(&comp.geo_broadcasts);
    append_tnf_twitch(&mut m.streams, lg.sport);
    Some(m)
}

/// Fetch every game of `lg` in the inclusive UTC-day range, normalized. ESPN's
/// scoreboard accepts a `YYYYMMDD-YYYYMMDD` range, so the whole window is one call.
pub async fn fetch_schedule(
    client: &reqwest::Client,
    lg: &EspnLeague,
    start: NaiveDate,
    end: NaiveDate,
) -> Result<Vec<NormalizedMatch>, reqwest::Error> {
    let url = format!(
        "{SITE}/{}/{}/scoreboard?dates={}-{}&limit=500",
        lg.espn_sport,
        lg.league,
        start.format("%Y%m%d"),
        end.format("%Y%m%d"),
    );
    let resp: Scoreboard = client
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(resp
        .events
        .into_iter()
        .filter_map(|e| to_match(e, lg))
        .filter(|m| {
            let d = m.begin_at.date_naive();
            d >= start && d <= end
        })
        .collect())
}

// ----- Standings -------------------------------------------------------------

#[derive(Deserialize)]
struct StandingsResp {
    #[serde(default)]
    children: Vec<ConfGroup>,
}

#[derive(Deserialize)]
struct ConfGroup {
    #[serde(default)]
    name: String,
    #[serde(default)]
    abbreviation: String,
    #[serde(default)]
    standings: Entries,
}

#[derive(Deserialize, Default)]
struct Entries {
    #[serde(default)]
    entries: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    #[serde(default)]
    team: TeamRef,
    #[serde(default)]
    stats: Vec<Stat>,
}

#[derive(Deserialize)]
struct Stat {
    #[serde(default)]
    name: String,
    #[serde(default)]
    value: Option<f64>,
    #[serde(rename = "displayValue", default)]
    display_value: String,
}

impl Entry {
    fn stat_i32(&self, name: &str) -> i32 {
        self.stats
            .iter()
            .find(|s| s.name == name)
            .and_then(|s| s.value)
            .map_or(0, |v| v as i32)
    }
    fn stat_str(&self, name: &str) -> String {
        self.stats
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.display_value.clone())
            .unwrap_or_default()
    }
}

/// A short label for a conference: its abbreviation (AFC/NFC), else the name
/// without a trailing " Conference" ("Eastern Conference" → "Eastern").
fn conf_label(name: &str, abbrev: &str) -> String {
    match abbrev {
        "AFC" | "NFC" => abbrev.to_string(),
        _ => name.trim_end_matches(" Conference").to_string(),
    }
}

fn conferences_from(resp: StandingsResp, lg: &EspnLeague) -> Vec<EventInfo> {
    resp.children
        .into_iter()
        .enumerate()
        .map(|(i, conf)| {
            // (sort_key, row) — sort by the league's ranking stat (wins / points).
            let mut rows: Vec<(i32, StandingRow)> = conf
                .standings
                .entries
                .into_iter()
                .map(|e| {
                    let last = e.stat_str(lg.last_stat);
                    let row = StandingRow {
                        rank: 0,
                        team: e.team.full_name(),
                        wins: e.stat_i32("wins"),
                        losses: e.stat_i32("losses"),
                        ties: e.stat_i32("ties"),
                        game_wins: 0,
                        game_losses: 0,
                        // Games-back "-" reads better as an em dash.
                        gb: if last == "-" { "—".to_string() } else { last },
                    };
                    (e.stat_i32(lg.rank_stat), row)
                })
                .collect();
            // ESPN normally returns these in standing order, but be defensive:
            // order by the ranking stat (then fewer losses) and number from there.
            rows.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.losses.cmp(&b.1.losses)));
            let rows: Vec<StandingRow> = rows
                .into_iter()
                .enumerate()
                .map(|(idx, (_, mut r))| {
                    r.rank = idx as i32 + 1;
                    r
                })
                .collect();
            EventInfo {
                event: lg.name.to_string(),
                tournament_id: lg.standings_base + i as i64,
                stage: conf_label(&conf.name, &conf.abbreviation),
                sport: lg.sport,
                standings: rows,
                rounds: Vec::new(),
                swiss: Vec::new(),
            }
        })
        .filter(|e| !e.standings.is_empty())
        .collect()
}

/// Fetch `lg`'s standings, one table per group/conference. `season` is ESPN's
/// season year (see [`season_year`] / [`european_season`]); `None` lets ESPN pick
/// the current one (used for the World Cup, which isn't a yearly season).
pub async fn fetch_standings(
    client: &reqwest::Client,
    lg: &EspnLeague,
    season: Option<i32>,
) -> Result<Vec<EventInfo>, reqwest::Error> {
    let url = match season {
        Some(s) => format!(
            "{CORE}/{}/{}/standings?season={s}",
            lg.espn_sport, lg.league
        ),
        None => format!("{CORE}/{}/{}/standings", lg.espn_sport, lg.league),
    };
    let resp: StandingsResp = client
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(conferences_from(resp, lg))
}

// ----- Box score ---------------------------------------------------------------

#[derive(Deserialize, Default)]
pub struct RawSummary {
    #[serde(default)]
    boxscore: RawBox,
    #[serde(default)]
    leaders: Vec<RawTeamLeaders>,
    #[serde(default)]
    header: RawHeader,
    #[serde(default, rename = "scoringPlays")]
    scoring_plays: Vec<RawPlay>,
}
#[derive(Deserialize, Default)]
struct RawBox {
    #[serde(default)]
    teams: Vec<RawBoxTeam>,
    #[serde(default)]
    players: Vec<RawBoxPlayers>,
}
#[derive(Deserialize, Default)]
struct RawTeamRef {
    #[serde(default)]
    abbreviation: String,
}
#[derive(Deserialize, Default)]
struct RawBoxTeam {
    #[serde(default, rename = "homeAway")]
    home_away: String,
    #[serde(default)]
    statistics: Vec<RawLabeledStat>,
}
#[derive(Deserialize, Default)]
struct RawLabeledStat {
    #[serde(default)]
    label: String,
    #[serde(default, rename = "displayValue")]
    display_value: String,
}
#[derive(Deserialize, Default)]
struct RawBoxPlayers {
    #[serde(default)]
    team: RawTeamRef,
    #[serde(default)]
    statistics: Vec<RawPlayerGroup>,
}
#[derive(Deserialize, Default)]
struct RawPlayerGroup {
    #[serde(default)]
    name: String,
    #[serde(default)]
    labels: Vec<String>,
    #[serde(default)]
    athletes: Vec<RawAthleteLine>,
}
#[derive(Deserialize, Default)]
struct RawAthleteLine {
    #[serde(default)]
    athlete: RawAthlete,
    #[serde(default)]
    stats: Vec<String>,
}
#[derive(Deserialize, Default)]
struct RawAthlete {
    #[serde(default, rename = "displayName")]
    display_name: String,
}
#[derive(Deserialize, Default)]
struct RawTeamLeaders {
    #[serde(default)]
    team: RawTeamRef,
    #[serde(default)]
    leaders: Vec<RawLeaderCat>,
}
#[derive(Deserialize, Default)]
struct RawLeaderCat {
    #[serde(default, rename = "displayName")]
    display_name: String,
    #[serde(default)]
    leaders: Vec<RawLeaderEntry>,
}
#[derive(Deserialize, Default)]
struct RawLeaderEntry {
    #[serde(default, rename = "displayValue")]
    display_value: String,
    #[serde(default)]
    athlete: RawAthlete,
}
#[derive(Deserialize, Default)]
struct RawHeader {
    #[serde(default)]
    competitions: Vec<RawCompetition>,
}
#[derive(Deserialize, Default)]
struct RawCompetition {
    #[serde(default)]
    competitors: Vec<RawCompetitor>,
}
#[derive(Deserialize, Default)]
struct RawCompetitor {
    #[serde(default, rename = "homeAway")]
    home_away: String,
    #[serde(default)]
    team: RawTeamRef,
    #[serde(default)]
    score: String,
    #[serde(default)]
    linescores: Vec<RawLineValue>,
}
#[derive(Deserialize, Default)]
struct RawLineValue {
    // ESPN's summary linescores carry a string `displayValue`, not a numeric
    // `value` — the fixture (a finished NFL game) has no `value` field at all.
    #[serde(default, rename = "displayValue")]
    display_value: String,
}
#[derive(Deserialize, Default)]
struct RawPlay {
    #[serde(default)]
    period: RawPeriodNum,
    #[serde(default)]
    clock: RawClock,
    #[serde(default)]
    team: RawTeamRef,
    #[serde(default, rename = "type")]
    kind: RawPlayType,
    #[serde(default)]
    text: String,
}
#[derive(Deserialize, Default)]
struct RawPeriodNum {
    #[serde(default)]
    number: i64,
}
#[derive(Deserialize, Default)]
struct RawClock {
    #[serde(default, rename = "displayValue")]
    display_value: String,
}
#[derive(Deserialize, Default)]
struct RawPlayType {
    #[serde(default)]
    abbreviation: String,
}

pub async fn fetch_box_score(
    client: &reqwest::Client,
    lg: &EspnLeague,
    event_id: i64,
) -> Result<RawSummary, reqwest::Error> {
    client
        .get(format!(
            "{SITE}/{}/{}/summary?event={event_id}",
            lg.espn_sport, lg.league
        ))
        .send()
        .await?
        .error_for_status()?
        .json::<RawSummary>()
        .await
}

pub fn to_box_score(s: &RawSummary) -> BoxScore {
    // Away/home from the header competitors.
    let comp = s.header.competitions.first();
    let away = comp.and_then(|c| c.competitors.iter().find(|t| t.home_away == "away"));
    let home = comp.and_then(|c| c.competitors.iter().find(|t| t.home_away == "home"));

    let line = match (away, home) {
        (Some(a), Some(h)) => {
            let n = a.linescores.len().max(h.linescores.len());
            let segments: Vec<String> = (0..n)
                .map(|i| {
                    if i < 4 {
                        (i + 1).to_string()
                    } else {
                        "OT".to_string()
                    }
                })
                .collect();
            let seg = |c: &RawCompetitor| {
                c.linescores
                    .iter()
                    .map(|l| l.display_value.clone())
                    .collect::<Vec<_>>()
            };
            let mut away_seg = seg(a);
            away_seg.resize(segments.len(), String::new());
            let mut home_seg = seg(h);
            home_seg.resize(segments.len(), String::new());
            Some(LineScore {
                segments,
                away: LineRow {
                    team: a.team.abbreviation.clone(),
                    abbrev: a.team.abbreviation.clone(),
                    segment_values: away_seg,
                    total: a.score.clone(),
                },
                home: LineRow {
                    team: h.team.abbreviation.clone(),
                    abbrev: h.team.abbreviation.clone(),
                    segment_values: home_seg,
                    total: h.score.clone(),
                },
                totals: vec![StatPair {
                    label: "T".into(),
                    away: a.score.clone(),
                    home: h.score.clone(),
                    away_share: None,
                }],
            })
        }
        _ => None,
    };

    // Team stats: labels present on the away team, matched by label on the home team.
    let box_away = s.boxscore.teams.iter().find(|t| t.home_away == "away");
    let box_home = s.boxscore.teams.iter().find(|t| t.home_away == "home");
    let team_stats = match (box_away, box_home) {
        (Some(a), Some(h)) => a
            .statistics
            .iter()
            .filter_map(|as_| {
                let hs = h.statistics.iter().find(|x| x.label == as_.label)?;
                let (av, hv) = (as_.display_value.clone(), hs.display_value.clone());
                Some(StatPair {
                    away_share: stat_share(&av, &hv),
                    label: as_.label.clone(),
                    away: av,
                    home: hv,
                })
            })
            .collect(),
        _ => Vec::new(),
    };

    // Leaders: top athlete per category, per team.
    let leaders = s
        .leaders
        .iter()
        .flat_map(|tl| {
            let abbr = tl.team.abbreviation.clone();
            tl.leaders.iter().filter_map(move |cat| {
                let e = cat.leaders.first()?;
                Some(LeaderCard {
                    name: e.athlete.display_name.clone(),
                    team: abbr.clone(),
                    category: cat.display_name.clone(),
                    line: e.display_value.clone(),
                })
            })
        })
        .collect();

    // Player tables: one per (team, stat group).
    let player_tables = s
        .boxscore
        .players
        .iter()
        .flat_map(|tp| {
            let abbr = tp.team.abbreviation.clone();
            tp.statistics
                .iter()
                .filter(|g| !g.athletes.is_empty())
                .map(move |g| PlayerTable {
                    title: if g.name.is_empty() {
                        abbr.clone()
                    } else {
                        format!("{} — {}", g.name, abbr)
                    },
                    team: abbr.clone(),
                    columns: g.labels.clone(),
                    rows: g
                        .athletes
                        .iter()
                        .map(|a| PlayerRow {
                            name: a.athlete.display_name.clone(),
                            note: String::new(),
                            values: a.stats.clone(),
                        })
                        .collect(),
                })
        })
        .collect();

    // Timeline: scoring plays.
    let timeline = s
        .scoring_plays
        .iter()
        .map(|p| ScoreEvent {
            segment: format!("Q{}", p.period.number),
            clock: p.clock.display_value.clone(),
            team: p.team.abbreviation.clone(),
            kind: p.kind.abbreviation.to_lowercase(),
            description: p.text.clone(),
        })
        .collect();

    BoxScore {
        line,
        team_stats,
        leaders,
        player_tables,
        timeline,
        games: Vec::new(),
        unavailable: false,
    }
}

// ---- Playoff brackets -------------------------------------------------------
//
// ESPN's scoreboard already carries everything a knockout bracket needs: each
// event's `season.type` (the round), the matchup `name` (which, before a match is
// seeded, is a "Quarterfinal 1 Winner at …" feeder placeholder), per-competitor
// `winner`, and — for the series sports — an embedded `series` games-won summary.
// These builders turn that into the shared `Vec<BracketRound>` the renderer draws.

/// Fetch a league's scoreboard over the inclusive UTC-day range as raw events
/// (parsed but not normalized) — the input to the bracket builders.
async fn fetch_scoreboard_events(
    client: &reqwest::Client,
    espn_sport: &str,
    league: &str,
    start: NaiveDate,
    end: NaiveDate,
) -> Result<Vec<Event>, reqwest::Error> {
    let url = format!(
        "{SITE}/{espn_sport}/{league}/scoreboard?dates={}-{}&limit=500",
        start.format("%Y%m%d"),
        end.format("%Y%m%d"),
    );
    let resp: Scoreboard = client
        .get(&url)
        .timeout(Duration::from_secs(15))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(resp.events)
}

/// A "… N Winner"/"… N Loser" feeder placeholder → `(feeder index 1-based, is the
/// loser slot)`. `None` for a real, seeded team name.
fn parse_placeholder(name: &str) -> Option<(u32, bool)> {
    let (rest, is_loser) = name
        .strip_suffix(" Winner")
        .map(|r| (r, false))
        .or_else(|| name.strip_suffix(" Loser").map(|r| (r, true)))?;
    let n: u32 = rest.rsplit(' ').next()?.parse().ok()?;
    Some((n, is_loser))
}

/// One parsed knockout match. Side 0 is the away team, side 1 the home team
/// (matching the "away at home" name and the schedule's team-a/team-b order).
#[derive(Clone)]
struct KnMatch {
    id: i64,
    /// ESPN `season.type` — a per-round stage id used only to bucket matches.
    kind: i64,
    /// ISO date string, for ordering the two one-match rounds (3rd place < final).
    date: String,
    /// Seeded team name per side, empty while still a placeholder.
    name: [String; 2],
    score: [Option<i64>; 2],
    won: [bool; 2],
    /// Feeder placeholder per side (`(index, is_loser)`), when not yet seeded.
    ph: [Option<(u32, bool)>; 2],
}

impl KnMatch {
    fn winner_team(&self) -> &str {
        if self.won[0] {
            &self.name[0]
        } else if self.won[1] {
            &self.name[1]
        } else {
            ""
        }
    }
    fn winner_code(&self) -> String {
        if self.won[0] {
            "a".into()
        } else if self.won[1] {
            "b".into()
        } else {
            String::new()
        }
    }
}

/// The World Cup (or any single-elimination soccer knockout) bracket: fetch the
/// whole year's scoreboard (one call captures every knockout round wherever the
/// tournament falls; the group stage is dropped by round size) and assemble it.
pub async fn fetch_soccer_bracket(
    client: &reqwest::Client,
    lg: &EspnLeague,
    year: i32,
) -> Result<Vec<BracketRound>, reqwest::Error> {
    let start = NaiveDate::from_ymd_opt(year, 1, 1).unwrap_or_default();
    let end = NaiveDate::from_ymd_opt(year, 12, 31).unwrap_or_default();
    let events = fetch_scoreboard_events(client, lg.espn_sport, lg.league, start, end).await?;
    Ok(soccer_bracket(parse_knockout(events)))
}

/// Parse scoreboard events into knockout matches (dropping anything unparseable).
fn parse_knockout(events: Vec<Event>) -> Vec<KnMatch> {
    let mut out = Vec::new();
    for e in events {
        let Ok(id) = e.id.parse::<i64>() else {
            continue;
        };
        let Some(comp) = e.competitions.into_iter().next() else {
            continue;
        };
        let finished = e.status.r#type.state == "post";
        let side = |ha: &str| -> Option<(String, Option<i64>, bool, Option<(u32, bool)>)> {
            let c = comp.competitors.iter().find(|c| c.home_away == ha)?;
            let ph = parse_placeholder(&c.team.display_name);
            let name = if ph.is_some() {
                String::new()
            } else {
                c.team.label()
            };
            let score = finished.then(|| c.score.parse::<i64>().ok()).flatten();
            Some((name, score, c.winner, ph))
        };
        let (Some(a), Some(b)) = (side("away"), side("home")) else {
            continue;
        };
        out.push(KnMatch {
            id,
            kind: e.season.kind,
            date: e.date,
            name: [a.0, b.0],
            score: [a.1, b.1],
            won: [a.2, b.2],
            ph: [a.3, b.3],
        });
    }
    out
}

/// The round title for a knockout column of `size` matches (16 → Round of 32, …).
fn soccer_round_title(size: usize) -> String {
    match size {
        16 => "Round of 32",
        8 => "Round of 16",
        4 => "Quarterfinals",
        2 => "Semifinals",
        1 => "Final",
        _ => "",
    }
    .to_string()
}

/// Assemble parsed knockout matches into bracket rounds. Rounds are the
/// power-of-two-≤16 `season.type` buckets (the group stage, being larger, drops
/// out); within a round, matches sort by id — which is ESPN's FIFA match order,
/// the same 1-based index the feeder placeholders reference. Feeders come from a
/// side's placeholder index when unseeded, or from which prior match its team won
/// when seeded — so the real bracket crossovers (QF1 ← R16 #1 & #2, QF2 ← #5 & #6)
/// are honored rather than assumed. The two one-match buckets split into the final
/// and, one day earlier, the third-place match (its own labeled column).
fn soccer_bracket(matches: Vec<KnMatch>) -> Vec<BracketRound> {
    use std::collections::HashMap;
    // Bucket by round stage id, keep only knockout-sized buckets.
    let mut buckets: HashMap<i64, Vec<KnMatch>> = HashMap::new();
    for m in matches {
        buckets.entry(m.kind).or_default().push(m);
    }
    let mut buckets: Vec<Vec<KnMatch>> = buckets
        .into_values()
        .filter(|v| matches!(v.len(), 1 | 2 | 4 | 8 | 16))
        .collect();
    for b in &mut buckets {
        b.sort_by(|x, y| x.id.cmp(&y.id));
    }
    if buckets.is_empty() {
        return Vec::new();
    }

    // Multi-match rounds (R32 → SF), largest first; the one-match buckets are the
    // final and the third-place match.
    let mut main: Vec<Vec<KnMatch>> = buckets.iter().filter(|b| b.len() > 1).cloned().collect();
    main.sort_by(|a, b| b.len().cmp(&a.len()));
    let mut ones: Vec<Vec<KnMatch>> = buckets.into_iter().filter(|b| b.len() == 1).collect();
    // Third place is the earlier of the two one-match games (or the one whose
    // placeholder marks it a "Loser" bracket); the later/other is the final.
    ones.sort_by(|a, b| a[0].date.cmp(&b[0].date));
    let third = if ones.len() >= 2 {
        let is_loser = |m: &KnMatch| m.ph.iter().flatten().any(|(_, l)| *l);
        // Prefer an explicit "… Loser" placeholder; else the earlier date.
        let idx = ones.iter().position(|b| is_loser(&b[0])).unwrap_or(0);
        Some(ones.remove(idx))
    } else {
        None
    };
    if let Some(f) = ones.into_iter().next() {
        main.push(f); // the final is the last main-tree round
    }

    // Feeder slot (into the prior round) for every match/side. Resolved so the real
    // bracket crossovers survive: a seeded side is matched to the prior match its
    // team won; an unseeded side uses its "… N Winner" placeholder index, but only
    // when that index points at a still-undecided prior match (a placeholder never
    // feeds from a decided one). Anything left over — a placeholder whose index
    // collided with an already-seeded match — is matched by elimination against the
    // remaining undecided prior matches, in id order.
    let feed_slots: Vec<Vec<[Option<u32>; 2]>> = {
        let mut all = Vec::with_capacity(main.len());
        for r in 0..main.len() {
            let mut fr = vec![[None, None]; main[r].len()];
            if r > 0 {
                let prev = &main[r - 1];
                let prev_won: Vec<&str> = prev.iter().map(KnMatch::winner_team).collect();
                let mut claimed = vec![false; prev.len()];
                let mut pending: Vec<(usize, usize)> = Vec::new();
                for (i, m) in main[r].iter().enumerate() {
                    for side in 0..2 {
                        if !m.name[side].is_empty() {
                            // Seeded: the prior slot whose winner is this team.
                            if let Some(s) = prev_won.iter().position(|w| *w == m.name[side]) {
                                fr[i][side] = Some(s as u32);
                                claimed[s] = true;
                            }
                        } else if let Some((n, _)) = m.ph[side] {
                            let s = n.saturating_sub(1) as usize;
                            if s < prev.len() && prev_won[s].is_empty() && !claimed[s] {
                                fr[i][side] = Some(s as u32);
                                claimed[s] = true;
                            } else {
                                pending.push((i, side));
                            }
                        } else {
                            pending.push((i, side));
                        }
                    }
                }
                let mut pool = (0..prev.len()).filter(|&s| !claimed[s]);
                for (i, side) in pending {
                    if let Some(s) = pool.next() {
                        fr[i][side] = Some(s as u32);
                    }
                }
            }
            all.push(fr);
        }
        all
    };

    let mut series: Vec<RawSeries> = Vec::new();
    for (r, round) in main.iter().enumerate() {
        let title = soccer_round_title(round.len());
        for (slot, m) in round.iter().enumerate() {
            let feed = |side: usize| -> Option<bracket_build::FeederRef> {
                feed_slots[r][slot][side].map(|s| (String::new(), (r - 1) as u32, s))
            };
            series.push(RawSeries {
                section: String::new(),
                round: r as u32,
                slot: slot as u32,
                round_title: title.clone(),
                team_a: m.name[0].clone(),
                team_b: m.name[1].clone(),
                score_a: m.score[0],
                score_b: m.score[1],
                winner: m.winner_code(),
                match_id: m.id,
                feed: [feed(0), feed(1)],
            });
        }
    }
    if let Some(t) = third {
        let m = &t[0];
        // A lone labeled column, no drawn feeders (both semifinal losers would
        // otherwise cross the final's connectors).
        series.push(RawSeries {
            section: "third".into(),
            round: 0,
            slot: 0,
            round_title: "Third Place".into(),
            team_a: m.name[0].clone(),
            team_b: m.name[1].clone(),
            score_a: m.score[0],
            score_b: m.score[1],
            winner: m.winner_code(),
            match_id: m.id,
            feed: [None, None],
        });
    }
    bracket_build::build(series)
}

// ----- NFL playoff bracket ---------------------------------------------------

/// The NFL playoff bracket for a season (empty outside the postseason). The
/// postseason plays in January/February of the year after the season's start
/// year, so the window spans the season boundary.
pub async fn fetch_nfl_bracket(
    client: &reqwest::Client,
    lg: &EspnLeague,
    season_start_year: i32,
) -> Result<Vec<BracketRound>, reqwest::Error> {
    let start = NaiveDate::from_ymd_opt(season_start_year, 12, 1).unwrap_or_default();
    let end = NaiveDate::from_ymd_opt(season_start_year + 1, 3, 1).unwrap_or_default();
    let events = fetch_scoreboard_events(client, lg.espn_sport, lg.league, start, end).await?;
    Ok(nfl_bracket(events))
}

/// `(round-in-conference, column title)` for an NFL postseason week; `None` skips
/// non-bracket weeks (week 4 is the Pro Bowl).
fn nfl_week_round(week: i64) -> Option<(u32, &'static str)> {
    match week {
        1 => Some((0, "Wild Card")),
        2 => Some((1, "Divisional")),
        3 => Some((2, "Conf. Championship")),
        5 => Some((0, "Super Bowl")), // the final's own section, one column
        _ => None,
    }
}

fn nfl_bracket(events: Vec<Event>) -> Vec<BracketRound> {
    use std::collections::HashMap;
    let mut raws: Vec<RawSeries> = Vec::new();
    let mut slot_counter: HashMap<(String, u32), u32> = HashMap::new();
    // Sort by (week, id) so slots within a round are stable and the final is last.
    let mut evs: Vec<Event> = events.into_iter().filter(|e| e.season.kind == 3).collect();
    evs.sort_by(|a, b| a.week.number.cmp(&b.week.number).then(a.id.cmp(&b.id)));
    for e in evs {
        let Some((round, title)) = nfl_week_round(e.week.number) else {
            continue;
        };
        let Ok(id) = e.id.parse::<i64>() else {
            continue;
        };
        let headline = e
            .competitions
            .first()
            .and_then(|c| c.notes.first())
            .map(|n| n.headline.as_str())
            .unwrap_or("");
        let section = if headline.contains("Super Bowl") || e.week.number == 5 {
            "final"
        } else if headline.starts_with("AFC") {
            "afc"
        } else if headline.starts_with("NFC") {
            "nfc"
        } else {
            continue;
        };
        let Some(comp) = e.competitions.into_iter().next() else {
            continue;
        };
        let (Some(away), Some(home)) = (
            comp.competitors.iter().find(|c| c.home_away == "away"),
            comp.competitors.iter().find(|c| c.home_away == "home"),
        ) else {
            continue;
        };
        let finished = e.status.r#type.state == "post";
        let winner = if !finished {
            ""
        } else if away.winner {
            "a"
        } else if home.winner {
            "b"
        } else {
            ""
        };
        let slot = slot_counter
            .entry((section.to_string(), round))
            .or_insert(0);
        let this_slot = *slot;
        *slot += 1;
        raws.push(RawSeries {
            section: section.to_string(),
            round,
            slot: this_slot,
            round_title: title.to_string(),
            team_a: away.team.label(),
            team_b: home.team.label(),
            score_a: finished.then(|| away.score.parse::<i64>().ok()).flatten(),
            score_b: finished.then(|| home.score.parse::<i64>().ok()).flatten(),
            winner: winner.to_string(),
            match_id: id,
            feed: [None, None],
        });
    }
    bracket_build::build(raws)
}

// ----- NBA playoff bracket ---------------------------------------------------

/// The NBA playoff bracket for a season (empty outside the playoffs). The playoffs
/// run mid-April to mid-June of the season's end year.
pub async fn fetch_nba_bracket(
    client: &reqwest::Client,
    lg: &EspnLeague,
    end_year: i32,
) -> Result<Vec<BracketRound>, reqwest::Error> {
    let start = NaiveDate::from_ymd_opt(end_year, 4, 1).unwrap_or_default();
    let end = NaiveDate::from_ymd_opt(end_year, 7, 15).unwrap_or_default();
    let events = fetch_scoreboard_events(client, lg.espn_sport, lg.league, start, end).await?;
    Ok(nba_bracket(events))
}

/// `(section, round-in-section, title)` for an NBA series from a game's round
/// headline ("East 1st Round", "West Conference Finals", "NBA Finals"); `None`
/// skips the play-in and anything unrecognized.
fn nba_round(headline: &str) -> Option<(&'static str, u32, &'static str)> {
    if headline.contains("Play-In") || headline.contains("Play In") {
        return None;
    }
    if headline.contains("NBA Finals") {
        return Some(("final", 0, "NBA Finals"));
    }
    let section = if headline.starts_with("East") {
        "east"
    } else if headline.starts_with("West") {
        "west"
    } else {
        return None;
    };
    // ESPN labels the conference finals "East Finals"/"West Finals" (the NBA
    // Finals is already handled above), the second round "… Semifinals".
    let (round, title) = if headline.contains("1st Round") {
        (0, "First Round")
    } else if headline.contains("2nd Round") || headline.contains("Semifinal") {
        (1, "Conf. Semifinals")
    } else if headline.contains("Finals") {
        (2, "Conference Finals")
    } else {
        return None;
    };
    Some((section, round, title))
}

fn nba_bracket(events: Vec<Event>) -> Vec<BracketRound> {
    use std::collections::HashMap;
    // Aggregate games into series by (section, round, matchup); each game carries
    // the running series score, so keep the latest game's snapshot per matchup.
    struct Acc {
        section: &'static str,
        round: u32,
        title: &'static str,
        best_id: i64,
        first_id: i64,
        team_a: String,
        team_b: String,
        id_a: String,
        id_b: String,
        wins_a: i64,
        wins_b: i64,
        completed: bool,
    }
    let mut map: HashMap<(String, u32, String), Acc> = HashMap::new();
    for e in events {
        if e.season.kind != 3 {
            continue;
        }
        let Ok(id) = e.id.parse::<i64>() else {
            continue;
        };
        let Some(comp) = e.competitions.into_iter().next() else {
            continue;
        };
        let headline = comp
            .notes
            .first()
            .map(|n| n.headline.as_str())
            .unwrap_or("");
        let Some((section, round, title)) = nba_round(headline) else {
            continue;
        };
        let (Some(away), Some(home)) = (
            comp.competitors.iter().find(|c| c.home_away == "away"),
            comp.competitors.iter().find(|c| c.home_away == "home"),
        ) else {
            continue;
        };
        let (ida, idb) = (away.team.id.clone(), home.team.id.clone());
        if ida.is_empty() || idb.is_empty() {
            continue;
        }
        let pair = {
            let (lo, hi) = if ida <= idb {
                (&ida, &idb)
            } else {
                (&idb, &ida)
            };
            format!("{lo}-{hi}")
        };
        // Series score for this game, matched by team id.
        let series = comp.series.as_ref();
        let wins_of = |tid: &str| -> i64 {
            series
                .and_then(|s| s.competitors.iter().find(|c| c.id == tid))
                .map(|c| c.wins)
                .unwrap_or(0)
        };
        let entry = map
            .entry((section.to_string(), round, pair))
            .or_insert(Acc {
                section,
                round,
                title,
                best_id: 0,
                first_id: i64::MAX,
                team_a: String::new(),
                team_b: String::new(),
                id_a: String::new(),
                id_b: String::new(),
                wins_a: 0,
                wins_b: 0,
                completed: false,
            });
        entry.first_id = entry.first_id.min(id);
        // Keep the most recent game's snapshot (latest series score / teams).
        if id >= entry.best_id {
            entry.best_id = id;
            entry.team_a = away.team.label();
            entry.team_b = home.team.label();
            entry.id_a = ida.clone();
            entry.id_b = idb.clone();
            entry.wins_a = wins_of(&ida);
            entry.wins_b = wins_of(&idb);
            entry.completed = series.map(|s| s.completed).unwrap_or(false);
        }
    }
    let mut accs: Vec<Acc> = map.into_values().collect();
    accs.sort_by(|x, y| {
        nba_section_rank(x.section)
            .cmp(&nba_section_rank(y.section))
            .then(x.round.cmp(&y.round))
            .then(x.first_id.cmp(&y.first_id))
    });
    let mut slot_counter: HashMap<(&str, u32), u32> = HashMap::new();
    let mut raws: Vec<RawSeries> = Vec::new();
    for a in accs {
        let slot = slot_counter.entry((a.section, a.round)).or_insert(0);
        let this_slot = *slot;
        *slot += 1;
        let winner = if !a.completed {
            ""
        } else if a.wins_a > a.wins_b {
            "a"
        } else {
            "b"
        };
        let _ = (&a.id_a, &a.id_b);
        raws.push(RawSeries {
            section: a.section.to_string(),
            round: a.round,
            slot: this_slot,
            round_title: a.title.to_string(),
            team_a: a.team_a,
            team_b: a.team_b,
            score_a: Some(a.wins_a),
            score_b: Some(a.wins_b),
            winner: winner.to_string(),
            match_id: a.best_id,
            feed: [None, None],
        });
    }
    bracket_build::build(raws)
}

fn nba_section_rank(section: &str) -> u8 {
    match section {
        "east" => 0,
        "west" => 1,
        _ => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A finished knockout match with real teams (side `w` won).
    fn kn(kind: i64, id: i64, a: &str, b: &str, w: usize) -> KnMatch {
        KnMatch {
            id,
            kind,
            date: format!("2026-07-{id:02}"),
            name: [a.into(), b.into()],
            score: [Some(1), Some(0)],
            won: [w == 0, w == 1],
            ph: [None, None],
        }
    }

    /// An unseeded knockout match, both sides "<prev> N Winner" placeholders.
    fn kn_ph(kind: i64, id: i64, date: &str, a: (u32, bool), b: (u32, bool)) -> KnMatch {
        KnMatch {
            id,
            kind,
            date: date.into(),
            name: [String::new(), String::new()],
            score: [None, None],
            won: [false, false],
            ph: [Some(a), Some(b)],
        }
    }

    #[test]
    fn soccer_bracket_honors_placeholder_crossovers_and_third_place() {
        // A tiny 8-team knockout: QF (4) → SF (2) → Final (1), + a 3rd-place game.
        // QF ids 1..4 are the seeded, played round. SF/Final/3rd are placeholders,
        // whose indices encode the real crossover (SF1 ← QF1&QF2, SF2 ← QF3&QF4).
        let matches = vec![
            kn(4, 1, "A", "B", 0),                             // QF1 → A
            kn(4, 2, "C", "D", 1),                             // QF2 → D
            kn(4, 3, "E", "F", 0),                             // QF3 → E
            kn(4, 4, "G", "H", 1),                             // QF4 → H
            kn_ph(2, 5, "2026-07-10", (2, false), (1, false)), // SF1 ← QF2, QF1
            kn_ph(2, 6, "2026-07-11", (4, false), (3, false)), // SF2 ← QF4, QF3
            kn_ph(3, 7, "2026-07-18", (1, true), (2, true)),   // 3rd ← SF losers
            kn_ph(1, 8, "2026-07-19", (2, false), (1, false)), // Final ← SF winners
        ];
        let rounds = soccer_bracket(matches);
        // Columns: QF, SF, Final (main tree) then Third.
        assert_eq!(rounds.len(), 4);
        assert_eq!(rounds[0].title, "Quarterfinals");
        assert_eq!(rounds[0].matches.len(), 4);
        assert_eq!(rounds[3].section, "third");
        assert_eq!(rounds[3].title, "Third Place");
        assert!(
            rounds[3].matches[0].feeders.is_empty(),
            "3rd place is feeder-less"
        );
        // SF column is index 1. SF1 (slot0) feeds from QF slots 1 and 0 (placeholder
        // indices 2 and 1) — a real crossover, not a 2k/2k+1 assumption.
        let sf1 = &rounds[1].matches[0];
        assert_eq!(sf1.feeders.len(), 2);
        assert!(sf1.feeders.contains(&(0, 1)) && sf1.feeders.contains(&(0, 0)));
        // Final (column 2) feeds from the two SFs (column 1).
        assert_eq!(rounds[2].title, "Final");
        let fin = &rounds[2].matches[0];
        assert!(fin.feeders.contains(&(1, 0)) && fin.feeders.contains(&(1, 1)));
    }

    #[test]
    fn soccer_bracket_resolves_placeholder_index_collision() {
        // Mirrors the real World-Cup case: a next-round match names one seeded side
        // and one "… N Winner" placeholder whose index lands on the *seeded* side's
        // own prior match. Elimination must send it to the remaining undecided one.
        let mixed = |id: i64, real: &str, ph: u32| KnMatch {
            id,
            kind: 8,
            date: format!("2026-07-{id:02}"),
            name: [real.into(), String::new()],
            score: [None, None],
            won: [false, false],
            ph: [None, Some((ph, false))],
        };
        let matches = vec![
            kn(16, 1, "P", "x", 0),                    // R32 s0 → P (decided)
            kn(16, 2, "Q", "y", 0),                    // R32 s1 → Q (decided)
            kn_ph(16, 3, "d", (1, false), (2, false)), // R32 s2 undecided
            kn_ph(16, 4, "d", (3, false), (4, false)), // R32 s3 undecided
            mixed(5, "P", 3), // R16 s0: P + "R32 #3 Winner" (→ undecided s2)
            mixed(6, "Q", 2), // R16 s1: Q + "R32 #2 Winner" (→ decided s1: collision)
        ];
        let rounds = soccer_bracket(matches);
        // Two columns (the 4-match round then the 2-match round); the next round is
        // col 1, fed from col 0.
        let r16 = &rounds[1];
        // s0: P (won prior s0) + placeholder #3 → undecided prior s2.
        assert_eq!(sorted(&r16.matches[0].feeders), vec![(0, 0), (0, 2)]);
        // s1: Q (won R32 s1) + placeholder #2 collided with decided s1 → elimination
        // gives the only remaining undecided match, s3.
        assert_eq!(sorted(&r16.matches[1].feeders), vec![(0, 1), (0, 3)]);
    }

    fn sorted(v: &[(usize, usize)]) -> Vec<(usize, usize)> {
        let mut v = v.to_vec();
        v.sort();
        v
    }

    /// Live smoke test against ESPN — run with `cargo test --features ssr -- --ignored
    /// espn_live_soccer_bracket --nocapture` while a World Cup is on. Prints the
    /// assembled bracket for eyeballing; asserts it isn't empty and converges.
    #[tokio::test]
    #[ignore = "hits the live ESPN API"]
    async fn espn_live_soccer_bracket() {
        let client = reqwest::Client::new();
        let rounds = fetch_soccer_bracket(&client, &WORLD_CUP, 2026)
            .await
            .unwrap();
        for r in &rounds {
            println!("[{}] {} ({} matches)", r.section, r.title, r.matches.len());
            for (i, m) in r.matches.iter().enumerate() {
                println!(
                    "   {i}: {} {:?} vs {} {:?}  win={}  feeders={:?}",
                    m.team_a, m.score_a, m.team_b, m.score_b, m.winner, m.feeders
                );
            }
        }
        assert!(!rounds.is_empty(), "a World Cup knockout bracket was built");
        assert!(
            rounds.iter().any(|r| r.title == "Final"),
            "the bracket reaches a Final"
        );
    }

    fn print_bracket(label: &str, rounds: &[BracketRound]) {
        println!("== {label} ==");
        for r in rounds {
            println!("[{}] {} ({} series)", r.section, r.title, r.matches.len());
            for (i, m) in r.matches.iter().enumerate() {
                println!(
                    "   {i}: {} {:?} vs {} {:?} win={} feeders={:?}",
                    m.team_a, m.score_a, m.team_b, m.score_b, m.winner, m.feeders
                );
            }
        }
    }

    #[tokio::test]
    #[ignore = "hits the live ESPN API"]
    async fn espn_live_nba_bracket() {
        let client = reqwest::Client::new();
        let rounds = fetch_nba_bracket(&client, &NBA, 2026).await.unwrap();
        print_bracket("NBA 2026", &rounds);
        assert!(!rounds.is_empty());
        assert!(rounds.iter().any(|r| r.section == "final"));
    }

    #[tokio::test]
    #[ignore = "hits the live ESPN API"]
    async fn espn_live_nfl_bracket() {
        let client = reqwest::Client::new();
        // The 2025 NFL season's playoffs play in Jan/Feb 2026.
        let rounds = fetch_nfl_bracket(&client, &NFL, 2025).await.unwrap();
        print_bracket("NFL 2025", &rounds);
        assert!(!rounds.is_empty());
        assert!(rounds.iter().any(|r| r.section == "final"));
    }

    #[test]
    fn parses_a_scheduled_and_a_final_game() {
        let json = r#"{"events":[
          {"id":"401810001","date":"2026-01-15T19:00Z","status":{"type":{"state":"pre","name":"STATUS_SCHEDULED"}},
           "competitions":[{"competitors":[
             {"homeAway":"home","score":"0","team":{"shortDisplayName":"Magic","displayName":"Orlando Magic","abbreviation":"ORL"}},
             {"homeAway":"away","score":"0","team":{"shortDisplayName":"Grizzlies","displayName":"Memphis Grizzlies","abbreviation":"MEM"}}]}]},
          {"id":"401810002","date":"2026-01-16T00:30Z","status":{"type":{"state":"post","name":"STATUS_FINAL"}},
           "competitions":[{"competitors":[
             {"homeAway":"home","score":"118","team":{"shortDisplayName":"Magic","displayName":"Orlando Magic","abbreviation":"ORL"}},
             {"homeAway":"away","score":"111","team":{"shortDisplayName":"Grizzlies","displayName":"Memphis Grizzlies","abbreviation":"MEM"}}]}]}
        ]}"#;
        let resp: Scoreboard = serde_json::from_str(json).unwrap();
        let games: Vec<NormalizedMatch> = resp
            .events
            .into_iter()
            .filter_map(|e| to_match(e, &NBA))
            .collect();
        assert_eq!(games.len(), 2);
        assert_eq!(games[0].sport, Sport::Nba);
        // Away at home: team_a is the away side.
        assert_eq!(games[0].team_a.label, "Grizzlies");
        assert_eq!(games[0].team_b.name, "Orlando Magic");
        assert_eq!(games[0].status, MatchStatus::Upcoming);
        assert_eq!(games[0].venue_tz, None);
        assert_eq!(games[1].status, MatchStatus::Finished);
        assert_eq!(games[1].team_a.score, Some(111));
        assert_eq!(games[1].team_b.score, Some(118));
    }

    #[test]
    fn builds_conference_tables() {
        let json = r#"{"children":[
          {"name":"Eastern Conference","abbreviation":"East","standings":{"entries":[
            {"team":{"displayName":"Detroit Pistons"},"stats":[{"name":"wins","value":60.0,"displayValue":"60"},{"name":"losses","value":22.0,"displayValue":"22"},{"name":"gamesBehind","value":0.0,"displayValue":"-"}]}
          ]}},
          {"name":"Western Conference","abbreviation":"West","standings":{"entries":[
            {"team":{"displayName":"Los Angeles Lakers"},"stats":[{"name":"wins","value":53.0,"displayValue":"53"},{"name":"losses","value":29.0,"displayValue":"29"},{"name":"gamesBehind","value":3.0,"displayValue":"3.0"}]}
          ]}}
        ]}"#;
        let resp: StandingsResp = serde_json::from_str(json).unwrap();
        let confs = conferences_from(resp, &NBA);
        let names: Vec<&str> = confs.iter().map(|c| c.stage.as_str()).collect();
        assert_eq!(names, ["Eastern", "Western"]);
        assert_eq!(confs[0].standings[0].team, "Detroit Pistons");
        assert_eq!(confs[0].standings[0].wins, 60);
        assert_eq!(confs[0].standings[0].gb, "—"); // leader's "-" → em dash
        assert_eq!(confs[0].tournament_id, 400);
        assert_eq!(confs[1].tournament_id, 401);
    }

    #[test]
    fn nfl_and_nba_use_different_season_conventions() {
        let jun_2026 = "2026-06-24T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        assert_eq!(season_year(Sport::Nfl, jun_2026), 2025);
        assert_eq!(season_year(Sport::Nba, jun_2026), 2026);
        let nov_2026 = "2026-11-15T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        assert_eq!(season_year(Sport::Nfl, nov_2026), 2026);
        assert_eq!(season_year(Sport::Nba, nov_2026), 2027);
    }

    #[test]
    fn geo_broadcasts_map_to_streams() {
        let raw: Vec<GeoBroadcast> = serde_json::from_str(
            r#"[
              {"type":{"shortName":"TV"},"market":{"type":"National"},"media":{"shortName":"ESPN"}},
              {"type":{"shortName":"TV"},"market":{"type":"Home"},"media":{"shortName":"NBC Sports BA"}},
              {"type":{"shortName":"Radio"},"market":{"type":"Away"},"media":{"shortName":"WFAN"}},
              {"type":{"shortName":"TV"},"market":{"type":"National"},"media":{"shortName":"ESPN"}}
            ]"#,
        )
        .unwrap();
        let sv = broadcasts(&raw);
        // National TV leads, deduped; ESPN national gets a watch URL.
        assert_eq!(sv.len(), 3);
        assert_eq!(sv[0].name, "ESPN");
        assert_eq!(sv[0].tag, "national · TV");
        assert_eq!(sv[0].group, "tv");
        assert!(sv[0].url.contains("espn.com"), "national should link");
        assert!(sv[0].official);
        // Local RSN: no URL, home tag.
        let rsn = sv.iter().find(|s| s.name == "NBC Sports BA").unwrap();
        assert!(rsn.url.is_empty(), "local RSN must not link");
        assert_eq!(rsn.tag, "home · TV");
        assert!(!rsn.official);
        // Radio grouped separately.
        let radio = sv.iter().find(|s| s.name == "WFAN").unwrap();
        assert_eq!(radio.group, "radio");
        assert_eq!(radio.tag, "away · radio");
    }

    #[test]
    fn empty_geo_broadcasts_yield_no_streams() {
        assert!(broadcasts(&[]).is_empty());
    }

    #[test]
    fn tnf_twitch_appended_for_nfl_prime_only() {
        let sv = |name: &str| StreamView {
            name: name.to_string(),
            ..Default::default()
        };
        // NFL + a Prime broadcast → one Twitch link appended.
        let mut s = vec![sv("Prime Video")];
        append_tnf_twitch(&mut s, Sport::Nfl);
        let tw: Vec<_> = s.iter().filter(|x| x.name == "Twitch").collect();
        assert_eq!(tw.len(), 1);
        assert_eq!(tw[0].url, "https://www.twitch.tv/primevideo");
        assert!(tw[0].official);
        // Idempotent — running again does not double-append.
        append_tnf_twitch(&mut s, Sport::Nfl);
        assert_eq!(s.iter().filter(|x| x.name == "Twitch").count(), 1);
        // NFL without Prime → no Twitch.
        let mut s2 = vec![sv("CBS")];
        append_tnf_twitch(&mut s2, Sport::Nfl);
        assert!(!s2.iter().any(|x| x.name == "Twitch"));
        // NBA with Prime → no Twitch (rule is NFL-only).
        let mut s3 = vec![sv("Prime Video")];
        append_tnf_twitch(&mut s3, Sport::Nba);
        assert!(!s3.iter().any(|x| x.name == "Twitch"));
    }
}

#[cfg(all(test, feature = "ssr"))]
mod boxscore_tests {
    use super::*;

    #[test]
    fn espn_to_box_score_maps_line_stats_leaders_and_players() {
        let s: RawSummary =
            serde_json::from_str(include_str!("testdata/espn_nfl_summary_401772988.json")).unwrap();
        let out = to_box_score(&s);

        let line = out.line.expect("line score");
        assert!(line.segments.len() >= 4, "at least four quarters");
        assert_eq!(
            line.away.segment_values.len(),
            line.segments.len(),
            "line row aligns to segments"
        );
        assert_eq!(
            line.home.segment_values.len(),
            line.segments.len(),
            "home line row aligns to segments"
        );

        // Team-stat comparison is non-empty and every bar-bearing row has a share
        // only when both sides parsed as numbers.
        assert!(!out.team_stats.is_empty(), "team stats present");

        // Leaders present (passing/rushing/receiving for NFL).
        assert!(!out.leaders.is_empty(), "leaders present");

        // Player tables present and column-aligned.
        assert!(!out.player_tables.is_empty(), "player tables present");
        let t = &out.player_tables[0];
        assert!(!t.rows.is_empty(), "player rows populated");
        assert_eq!(
            t.rows[0].values.len(),
            t.columns.len(),
            "row aligns to columns"
        );
        assert!(!out.unavailable);
    }
}
