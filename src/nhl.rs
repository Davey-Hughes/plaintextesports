//! NHL schedule + standings from the keyless NHL Web API (`api-web.nhle.com`),
//! normalized into the same [`NormalizedMatch`] model as MLB and the esports
//! feeds. Two teams per sport (away at home), behind the esports/traditional
//! toggle. The schedule endpoint returns a week at a time, so a date-range fetch
//! walks it a week at a time and de-dupes.

use crate::bracket_build::{self, RawSeries};
use crate::feed::{NormalizedMatch, NormalizedTeam};
use crate::types::{
    stat_share, BoxScore, BracketRound, EventInfo, LeaderCard, LineRow, LineScore, MatchStatus,
    PlayerRow, PlayerTable, ScoreEvent, Sport, StandingRow, StatPair, StreamView,
};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::Deserialize;
use std::collections::HashSet;

const BASE: &str = "https://api-web.nhle.com/v1";

// ----- Schedule --------------------------------------------------------------

#[derive(Deserialize)]
struct ScheduleResp {
    #[serde(rename = "gameWeek", default)]
    game_week: Vec<DayBlock>,
}

#[derive(Deserialize)]
struct DayBlock {
    #[serde(default)]
    games: Vec<RawGame>,
}

#[derive(Deserialize)]
struct RawGame {
    id: i64,
    #[serde(rename = "startTimeUTC", default)]
    start_time_utc: String,
    #[serde(rename = "gameState", default)]
    game_state: String,
    #[serde(rename = "gameScheduleState", default)]
    schedule_state: String,
    /// 1 = preseason, 2 = regular season, 3 = playoffs.
    #[serde(rename = "gameType", default)]
    game_type: i64,
    #[serde(rename = "venueTimezone", default)]
    venue_timezone: String,
    #[serde(default)]
    venue: LangStr,
    #[serde(rename = "awayTeam", default)]
    away_team: RawTeam,
    #[serde(rename = "homeTeam", default)]
    home_team: RawTeam,
    #[serde(rename = "tvBroadcasts", default)]
    tv_broadcasts: Vec<RawBroadcast>,
}

#[derive(Deserialize, Default)]
struct RawTeam {
    #[serde(rename = "commonName", default)]
    common_name: LangStr,
    #[serde(rename = "placeName", default)]
    place_name: LangStr,
    #[serde(default)]
    abbrev: String,
    #[serde(default)]
    score: Option<i64>,
    /// SVG logo URL, e.g. "https://assets.nhle.com/logos/nhl/svg/TOR_light.svg".
    #[serde(default)]
    logo: String,
}

#[derive(Deserialize, Default)]
struct LangStr {
    #[serde(default)]
    default: String,
}

#[derive(Deserialize, Default)]
struct RawBroadcast {
    #[serde(default)]
    network: String,
    /// "N" national, "H" home, "A" away.
    #[serde(default)]
    market: String,
    #[serde(rename = "countryCode", default)]
    country: String,
    #[serde(rename = "sequenceNumber", default)]
    sequence: i64,
}

impl RawTeam {
    /// Short row label — the nickname ("Canadiens"), falling back to the abbrev.
    fn label(&self) -> String {
        if !self.common_name.default.is_empty() {
            self.common_name.default.clone()
        } else {
            self.abbrev.clone()
        }
    }

    /// Full name ("Montréal Canadiens") — keys the team page + subscription.
    fn full_name(&self) -> String {
        match (
            self.place_name.default.trim(),
            self.common_name.default.trim(),
        ) {
            (place, common) if !place.is_empty() && !common.is_empty() => {
                format!("{place} {common}")
            }
            _ => self.label(),
        }
    }
}

/// Map the NHL's game/schedule state to our status.
fn status_of(game_state: &str, schedule_state: &str) -> MatchStatus {
    if schedule_state.eq_ignore_ascii_case("CNCL") {
        return MatchStatus::Canceled;
    }
    match game_state {
        "OFF" | "FINAL" => MatchStatus::Finished,
        "LIVE" | "CRIT" => MatchStatus::Live,
        _ => MatchStatus::Upcoming, // FUT, PRE
    }
}

/// the non-regular-season label for a game type, e.g. "Playoffs" (empty for the
/// regular season, which is just "NHL").
fn series_name(game_type: i64) -> String {
    match game_type {
        1 => "Preseason".to_string(),
        3 => "Playoffs".to_string(),
        _ => String::new(),
    }
}

/// Friendly display name for a *national* NHL carrier's feed abbreviation; unknown
/// (local RSN) abbrevs pass through unchanged so they render as plain text.
fn nhl_network(abbrev: &str) -> &str {
    let a = abbrev.trim();
    let up = a.to_ascii_uppercase();
    if up == "TVAS" {
        return "TVA Sports";
    }
    if up == "CBC" {
        return "CBC";
    }
    if up.starts_with("SN") {
        // The Sportsnet family (SN, SNP, SNE, SNW, SNO, SN1, …) — all national CA.
        return "Sportsnet";
    }
    match up.as_str() {
        "ESPN" => "ESPN",
        "ESPN2" => "ESPN2",
        "ABC" => "ABC",
        "TNT" => "TNT",
        "TBS" => "TBS",
        "TRUTV" => "truTV",
        "NHLN" => "NHL Network",
        _ => a,
    }
}

/// NHL TV carriers as [`StreamView`]s, mirroring the MLB/ESPN model: national
/// carriers (US + Canadian) get a watch link, local RSNs render as text; non-US
/// entries carry a country marker. This feed is TV-only (no radio).
fn broadcasts(raw: &[RawBroadcast]) -> Vec<StreamView> {
    let market_rank = |m: &str| match m {
        "N" => 0,
        "H" => 1,
        "A" => 2,
        _ => 3,
    };
    let mut sorted: Vec<&RawBroadcast> = raw.iter().collect();
    sorted.sort_by_key(|x| (market_rank(&x.market), x.sequence));
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for x in sorted {
        let name = nhl_network(&x.network);
        if name.is_empty() {
            continue;
        }
        let (scope, national) = match x.market.as_str() {
            "N" => ("national", true),
            "H" => ("home", false),
            "A" => ("away", false),
            _ => ("", false),
        };
        if !seen.insert(format!("{name}|{}", x.market)) {
            continue;
        }
        let mut tag = if scope.is_empty() {
            "TV".to_string()
        } else {
            format!("{scope} · TV")
        };
        if !x.country.is_empty() && !x.country.eq_ignore_ascii_case("US") {
            tag.push_str(&format!(" · {}", x.country.to_ascii_uppercase()));
        }
        let url = if national {
            crate::watch::national_watch_url(name)
                .unwrap_or_default()
                .to_string()
        } else {
            String::new()
        };
        out.push(StreamView {
            url,
            official: national,
            main: true,
            name: name.to_string(),
            tag,
            group: "tv".to_string(),
            ..Default::default()
        });
    }
    out
}

fn to_match(g: RawGame) -> Option<NormalizedMatch> {
    let begin_at = DateTime::parse_from_rfc3339(&g.start_time_utc)
        .ok()?
        .with_timezone(&Utc);
    let mut m = NormalizedMatch::team_sport(
        g.id,
        Sport::Nhl,
        "NHL",
        begin_at,
        status_of(&g.game_state, &g.schedule_state),
        NormalizedTeam {
            label: g.away_team.label(),
            name: g.away_team.full_name(),
            abbrev: g.away_team.abbrev.clone(),
            score: g.away_team.score,
        },
        NormalizedTeam {
            label: g.home_team.label(),
            name: g.home_team.full_name(),
            abbrev: g.home_team.abbrev.clone(),
            score: g.home_team.score,
        },
    );
    // The non-regular-season round (e.g. "Playoffs"), and the arena's IANA tz for
    // the local-time-at-the-venue toggle.
    m.series_name = series_name(g.game_type);
    m.venue_tz = Some(g.venue_timezone).filter(|s| !s.is_empty());
    // The arena name; the city is the home team's place (e.g. "Toronto").
    m.venue_name = g.venue.default;
    m.venue_location = g.home_team.place_name.default.trim().to_string();
    // The NHL schedule carries each team's SVG logo URL directly.
    m.team_a_logo = g.away_team.logo;
    m.team_b_logo = g.home_team.logo;
    m.streams = broadcasts(&g.tv_broadcasts);
    Some(m)
}

/// Fetch every NHL sport in the inclusive UTC-day range, normalized. The schedule
/// endpoint returns a week per call, so we step through the window a week at a
/// time and de-dupe by sport id. The first week's call propagates a network error
/// (so a real outage keeps the old data); later weeks are best-effort.
pub async fn fetch_schedule(
    client: &reqwest::Client,
    start: NaiveDate,
    end: NaiveDate,
) -> Result<Vec<NormalizedMatch>, reqwest::Error> {
    // Fetch the first week up front — its error propagates, so a real outage
    // keeps the old data. The remaining weeks are independent best-effort requests
    // (a flaky later week mustn't drop the weeks we have), so fetch them
    // concurrently rather than walking week-by-week and paying a round-trip each.
    let week_url = |d: NaiveDate| format!("{BASE}/schedule/{}", d.format("%Y-%m-%d"));

    let first: ScheduleResp = client
        .get(week_url(start))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let mut anchors = Vec::new();
    let mut anchor = start + Duration::days(7);
    while anchor <= end {
        anchors.push(anchor);
        anchor += Duration::days(7);
    }
    let rest = futures::future::join_all(anchors.into_iter().map(|a| {
        let url = week_url(a);
        async move {
            match client
                .get(&url)
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
            {
                Ok(r) => r.json::<ScheduleResp>().await.ok(),
                Err(_) => None,
            }
        }
    }))
    .await;

    // Process first week then the rest in ascending order, de-duping by id.
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for resp in std::iter::once(Some(first)).chain(rest) {
        let Some(resp) = resp else { continue };
        for day in resp.game_week {
            for g in day.games {
                // Dedup on the raw id before parsing: overlapping week windows
                // return the same games repeatedly, and to_match now does real
                // work (broadcast sort/dedup) we'd otherwise repeat then discard.
                // begin_at derives only from `g`, so a game's in-range status is
                // invariant across its appearances — deduping first is safe.
                if !seen.insert(g.id) {
                    continue;
                }
                if let Some(m) = to_match(g) {
                    let d = m.begin_at.date_naive();
                    if d >= start && d <= end {
                        out.push(m);
                    }
                }
            }
        }
    }
    Ok(out)
}

// ----- Standings -------------------------------------------------------------

#[derive(Deserialize)]
struct StandingsResp {
    #[serde(default)]
    standings: Vec<RawStanding>,
}

#[derive(Deserialize, Default)]
struct RawStanding {
    #[serde(rename = "teamName", default)]
    team_name: LangStr,
    #[serde(rename = "divisionName", default)]
    division_name: String,
    #[serde(rename = "divisionSequence", default)]
    division_sequence: i32,
    #[serde(default)]
    points: i32,
    #[serde(default)]
    wins: i32,
    #[serde(default)]
    losses: i32,
    #[serde(rename = "otLosses", default)]
    ot_losses: i32,
}

/// Display order + a stable table id for an NHL division (Eastern then Western).
fn division_meta(name: &str) -> Option<(i32, i64)> {
    match name {
        "Atlantic" => Some((0, 300)),
        "Metropolitan" => Some((1, 301)),
        "Central" => Some((2, 302)),
        "Pacific" => Some((3, 303)),
        _ => None,
    }
}

/// Build one [`EventInfo`] (standings table) per division from the flat 32-team
/// list, ordered Eastern (Atlantic, Metropolitan) then Western (Central, Pacific).
fn divisions_from(resp: StandingsResp) -> Vec<EventInfo> {
    use std::collections::HashMap;
    let mut by_div: HashMap<String, Vec<RawStanding>> = HashMap::new();
    for t in resp.standings {
        by_div.entry(t.division_name.clone()).or_default().push(t);
    }
    let mut divisions: Vec<(i32, EventInfo)> = by_div
        .into_iter()
        .filter_map(|(name, mut teams)| {
            let (order, id) = division_meta(&name)?;
            teams.sort_by_key(|t| t.division_sequence);
            let rows = teams
                .into_iter()
                .map(|t| StandingRow {
                    rank: t.division_sequence,
                    team: t.team_name.default,
                    wins: t.wins,
                    losses: t.losses,
                    // OT losses ride the "ties" slot → rendered as W-L-OTL.
                    ties: t.ot_losses,
                    game_wins: 0,
                    game_losses: 0,
                    // The last column shows championship points for NHL.
                    gb: t.points.to_string(),
                })
                .collect();
            Some((
                order,
                EventInfo {
                    event: "NHL".to_string(),
                    tournament_id: id,
                    stage: name,
                    sport: Sport::Nhl,
                    standings: rows,
                    rounds: Vec::new(),
                    swiss: Vec::new(),
                },
            ))
        })
        .collect();
    divisions.sort_by_key(|(order, _)| *order);
    divisions.into_iter().map(|(_, e)| e).collect()
}

/// Fetch the current NHL standings, one table per division. Keyless, like MLB.
pub async fn fetch_standings(client: &reqwest::Client) -> Result<Vec<EventInfo>, reqwest::Error> {
    let url = format!("{BASE}/standings/now");
    let resp: StandingsResp = client
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(divisions_from(resp))
}

// ---- Box score raw structs -------------------------------------------------

#[derive(Deserialize, Default)]
struct NameDefault {
    #[serde(default)]
    default: String,
}
#[derive(Deserialize, Default)]
struct PeriodDescriptor {
    #[serde(default)]
    number: i64,
    #[serde(default, rename = "periodType")]
    period_type: String,
}

// ----- Playoff bracket -------------------------------------------------------
//
// The NHL Web API exposes a ready-made bracket: one object per series with its
// round (`playoffRound`), a stable `seriesLetter` (A–H first round, I–L second,
// M/N conference finals, O the Cup final), each seed's team and games won, and the
// winner id. Letters A–D / I–J / M are one conference, E–H / K–L / N the other;
// which is East vs West comes from the conference-final titles. Feeders fall out
// of the standard lettered tree (A&B → I, …) so the shared builder derives them.

#[derive(Deserialize, Default)]
struct BracketResp {
    #[serde(default)]
    series: Vec<BracketSeries>,
}

#[derive(Deserialize, Default)]
struct BracketSeries {
    #[serde(rename = "seriesLetter", default)]
    letter: String,
    #[serde(rename = "seriesTitle", default)]
    title: String,
    #[serde(rename = "playoffRound", default)]
    round: i64,
    #[serde(rename = "topSeedWins", default)]
    top_wins: i64,
    #[serde(rename = "bottomSeedWins", default)]
    bottom_wins: i64,
    #[serde(rename = "winningTeamId", default)]
    winner_id: i64,
    #[serde(rename = "topSeedTeam", default)]
    top: BracketTeam,
    #[serde(rename = "bottomSeedTeam", default)]
    bottom: BracketTeam,
}

#[derive(Deserialize, Default)]
struct BracketTeam {
    #[serde(default)]
    id: i64,
    #[serde(default)]
    abbrev: String,
    #[serde(rename = "commonName", default)]
    common_name: NameDefault,
}

impl BracketTeam {
    /// Short bracket label ("Maple Leafs"), falling back to the abbreviation.
    fn label(&self) -> String {
        if self.common_name.default.is_empty() {
            self.abbrev.clone()
        } else {
            self.common_name.default.clone()
        }
    }
}

/// The NHL playoff bracket for a season (empty off-season / before the playoffs).
pub async fn fetch_bracket(
    client: &reqwest::Client,
    year: i32,
) -> Result<Vec<BracketRound>, reqwest::Error> {
    let url = format!("{BASE}/playoff-bracket/{year}");
    let resp: BracketResp = client
        .get(&url)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(nhl_bracket(resp.series))
}

fn nhl_round_title(round: i64) -> String {
    match round {
        1 => "First Round",
        2 => "Second Round",
        3 => "Conference Finals",
        _ => "Stanley Cup Final",
    }
    .to_string()
}

/// Assemble NHL bracket series into rounds. Only series that have begun (both
/// seeds set) contribute; the letter order within each round places matches so the
/// standard tree connects them.
fn nhl_bracket(series: Vec<BracketSeries>) -> Vec<BracketRound> {
    if series.is_empty() {
        return Vec::new();
    }
    // Is the alphabetically-first conference-final half the Eastern one? (Letters
    // M/N are the two conference finals; M is the first half.)
    let first_half_is_east = {
        let mut finals: Vec<&BracketSeries> = series.iter().filter(|s| s.round == 3).collect();
        finals.sort_by(|a, b| a.letter.cmp(&b.letter));
        !matches!(finals.first(), Some(f) if f.title.to_lowercase().contains("west"))
    };

    let mut raws: Vec<RawSeries> = Vec::new();
    for round in 1..=4 {
        let mut in_round: Vec<&BracketSeries> =
            series.iter().filter(|s| s.round == round).collect();
        in_round.sort_by(|a, b| a.letter.cmp(&b.letter));
        let half_size = in_round.len().div_ceil(2).max(1);
        for (i, s) in in_round.iter().enumerate() {
            let (section, slot) = if round == 4 {
                ("final".to_string(), 0)
            } else {
                let first_half = i < half_size;
                let east = first_half == first_half_is_east;
                let sec = if east { "east" } else { "west" };
                (sec.to_string(), (i % half_size) as u32)
            };
            let winner = if s.winner_id != 0 && s.winner_id == s.top.id {
                "a"
            } else if s.winner_id != 0 && s.winner_id == s.bottom.id {
                "b"
            } else {
                ""
            };
            raws.push(RawSeries {
                section,
                round: (round - 1) as u32,
                slot,
                round_title: nhl_round_title(round),
                team_a: s.top.label(),
                team_b: s.bottom.label(),
                score_a: Some(s.top_wins),
                score_b: Some(s.bottom_wins),
                winner: winner.to_string(),
                match_id: 0, // the bracket endpoint carries no per-game id to link
                feed: [None, None],
            });
        }
    }
    bracket_build::build(raws)
}

// ---- landing ----
#[derive(Deserialize, Default)]
pub struct RawLanding {
    #[serde(default, rename = "awayTeam")]
    away_team: RawNhlTeam,
    #[serde(default, rename = "homeTeam")]
    home_team: RawNhlTeam,
    #[serde(default)]
    summary: RawSummary,
}
#[derive(Deserialize, Default)]
struct RawNhlTeam {
    #[serde(default)]
    abbrev: String,
    #[serde(default, rename = "commonName")]
    common_name: NameDefault,
    #[serde(default, rename = "placeName")]
    place_name: NameDefault,
    #[serde(default)]
    score: i64,
}
impl RawNhlTeam {
    /// Full display name ("Dallas Stars"); falls back to the abbrev when the
    /// place/common names are absent.
    fn full_name(&self) -> String {
        match (
            self.place_name.default.trim(),
            self.common_name.default.trim(),
        ) {
            (place, common) if !place.is_empty() && !common.is_empty() => {
                format!("{place} {common}")
            }
            (_, common) if !common.is_empty() => common.to_string(),
            _ => self.abbrev.clone(),
        }
    }
}
#[derive(Deserialize, Default)]
struct RawSummary {
    #[serde(default)]
    scoring: Vec<RawScoringPeriod>,
    #[serde(default, rename = "threeStars")]
    three_stars: Vec<RawStar>,
}
#[derive(Deserialize, Default)]
struct RawScoringPeriod {
    #[serde(default, rename = "periodDescriptor")]
    period: PeriodDescriptor,
    #[serde(default)]
    goals: Vec<RawGoal>,
}
#[derive(Deserialize, Default)]
struct RawGoal {
    #[serde(default, rename = "timeInPeriod")]
    time_in_period: String,
    #[serde(default, rename = "teamAbbrev")]
    team_abbrev: TeamAbbrev,
    #[serde(default, rename = "firstName")]
    first_name: NameDefault,
    #[serde(default, rename = "lastName")]
    last_name: NameDefault,
    #[serde(default)]
    assists: Vec<RawAssist>,
    #[serde(default, rename = "strength")]
    strength: String,
}
#[derive(Deserialize, Default)]
struct TeamAbbrev {
    #[serde(default)]
    default: String,
}
#[derive(Deserialize, Default)]
struct RawAssist {
    #[serde(default, rename = "firstName")]
    first_name: NameDefault,
    #[serde(default, rename = "lastName")]
    last_name: NameDefault,
}
#[derive(Deserialize, Default)]
struct RawStar {
    #[serde(default)]
    star: i64,
    #[serde(default)]
    name: NameDefault,
    #[serde(default, rename = "teamAbbrev")]
    team_abbrev: String,
    #[serde(default)]
    position: String,
    #[serde(default)]
    goals: i64,
    #[serde(default)]
    assists: i64,
    #[serde(default)]
    saves: i64,
    #[serde(default, rename = "savePctg")]
    save_pctg: f64,
}

// ---- right-rail ----
#[derive(Deserialize, Default)]
pub struct RawRightRail {
    #[serde(default)]
    linescore: RawLine,
    #[serde(default, rename = "teamGameStats")]
    team_game_stats: Vec<RawTgs>,
}
#[derive(Deserialize, Default)]
struct RawLine {
    #[serde(default, rename = "byPeriod")]
    by_period: Vec<RawLinePeriod>,
}
#[derive(Deserialize, Default)]
struct RawLinePeriod {
    #[serde(default, rename = "periodDescriptor")]
    period: PeriodDescriptor,
    #[serde(default)]
    away: i64,
    #[serde(default)]
    home: i64,
}
#[derive(Deserialize, Default)]
struct RawTgs {
    #[serde(default)]
    category: String,
    // awayValue/homeValue are sometimes a number, sometimes a string ("24/52",
    // "0/3") — capture as serde_json::Value and render with `val_str` below.
    #[serde(default, rename = "awayValue")]
    away_value: serde_json::Value,
    #[serde(default, rename = "homeValue")]
    home_value: serde_json::Value,
}

// ---- boxscore ----
#[derive(Deserialize, Default)]
pub struct RawNhlBox {
    #[serde(default, rename = "playerByGameStats")]
    players: RawPbg,
}
#[derive(Deserialize, Default)]
struct RawPbg {
    #[serde(default, rename = "awayTeam")]
    away: RawPbgTeam,
    #[serde(default, rename = "homeTeam")]
    home: RawPbgTeam,
}
#[derive(Deserialize, Default)]
struct RawPbgTeam {
    #[serde(default)]
    forwards: Vec<RawSkater>,
    #[serde(default)]
    defense: Vec<RawSkater>,
    #[serde(default)]
    goalies: Vec<RawGoalie>,
}
#[derive(Deserialize, Default)]
struct RawSkater {
    #[serde(default)]
    name: NameDefault,
    #[serde(default)]
    position: String,
    #[serde(default)]
    goals: i64,
    #[serde(default)]
    assists: i64,
    #[serde(default)]
    points: i64,
    #[serde(default, rename = "plusMinus")]
    plus_minus: i64,
    #[serde(default)]
    sog: i64,
    #[serde(default)]
    hits: i64,
    #[serde(default, rename = "blockedShots")]
    blocked: i64,
    #[serde(default)]
    pim: i64,
    #[serde(default)]
    toi: String,
}
#[derive(Deserialize, Default)]
struct RawGoalie {
    #[serde(default)]
    name: NameDefault,
    #[serde(default, rename = "saveShotsAgainst")]
    save_shots_against: String, // "31/33"
    #[serde(default, rename = "goalsAgainst")]
    goals_against: i64,
    #[serde(default)]
    toi: String,
}

/// Render an NHL right-rail stat value (number or string like "24/52", 0.538).
fn val_str(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => {
            // Percentages come as a 0..1 float; show whole numbers plainly.
            if let Some(f) = n.as_f64() {
                if f.fract() != 0.0 {
                    format!("{:.1}%", f * 100.0)
                } else {
                    format!("{f}")
                }
            } else {
                n.to_string()
            }
        }
        _ => String::new(),
    }
}

/// Fetch a game's landing (summary/scoring/stars), right-rail (linescore + team
/// stats), and boxscore (per-player stats) — three keyless calls.
pub async fn fetch_box_score(
    client: &reqwest::Client,
    game_id: i64,
) -> Result<(RawLanding, RawRightRail, RawNhlBox), reqwest::Error> {
    let landing = client
        .get(format!("{BASE}/gamecenter/{game_id}/landing"))
        .send()
        .await?
        .error_for_status()?
        .json::<RawLanding>()
        .await?;
    let rr = client
        .get(format!("{BASE}/gamecenter/{game_id}/right-rail"))
        .send()
        .await?
        .error_for_status()?
        .json::<RawRightRail>()
        .await?;
    let bs = client
        .get(format!("{BASE}/gamecenter/{game_id}/boxscore"))
        .send()
        .await?
        .error_for_status()?
        .json::<RawNhlBox>()
        .await?;
    Ok((landing, rr, bs))
}

fn period_label(p: &PeriodDescriptor) -> String {
    match p.period_type.as_str() {
        "OT" => "OT".to_string(),
        "SO" => "SO".to_string(),
        _ => p.number.to_string(),
    }
}

fn skater_table(abbrev: &str, team: &RawPbgTeam) -> PlayerTable {
    let rows = team
        .forwards
        .iter()
        .chain(team.defense.iter())
        .map(|s| PlayerRow {
            name: s.name.default.clone(),
            note: s.position.clone(),
            values: vec![
                s.goals.to_string(),
                s.assists.to_string(),
                s.points.to_string(),
                format!("{:+}", s.plus_minus),
                s.sog.to_string(),
                s.hits.to_string(),
                s.blocked.to_string(),
                s.pim.to_string(),
                s.toi.clone(),
            ],
        })
        .collect();
    PlayerTable {
        title: format!("Skaters — {abbrev}"),
        team: abbrev.to_string(),
        columns: ["G", "A", "P", "+/-", "SOG", "HITS", "BLK", "PIM", "TOI"]
            .map(String::from)
            .to_vec(),
        rows,
    }
}

fn goalie_table(abbrev: &str, team: &RawPbgTeam) -> PlayerTable {
    let rows = team
        .goalies
        .iter()
        .map(|g| {
            let (sv, sa) = g
                .save_shots_against
                .split_once('/')
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .unwrap_or_default();
            PlayerRow {
                name: g.name.default.clone(),
                note: String::new(),
                values: vec![sa, sv, g.goals_against.to_string(), g.toi.clone()],
            }
        })
        .collect();
    PlayerTable {
        title: format!("Goalies — {abbrev}"),
        team: abbrev.to_string(),
        columns: ["SA", "SV", "GA", "TOI"].map(String::from).to_vec(),
        rows,
    }
}

/// Normalize a game's landing + right-rail + boxscore into the shared `BoxScore`.
pub fn to_box_score(landing: &RawLanding, rr: &RawRightRail, bs: &RawNhlBox) -> BoxScore {
    let (aw, hm) = (&landing.away_team, &landing.home_team);

    // Line: goals by period + G / SOG totals.
    let segments: Vec<String> = rr
        .linescore
        .by_period
        .iter()
        .map(|p| period_label(&p.period))
        .collect();
    let sog = rr.team_game_stats.iter().find(|t| t.category == "sog");
    let totals = vec![
        StatPair {
            label: "G".into(),
            away: aw.score.to_string(),
            home: hm.score.to_string(),
            away_share: None,
        },
        StatPair {
            label: "SOG".into(),
            away: sog.map(|t| val_str(&t.away_value)).unwrap_or_default(),
            home: sog.map(|t| val_str(&t.home_value)).unwrap_or_default(),
            away_share: None,
        },
    ];
    let line = LineScore {
        segments,
        away: LineRow {
            team: aw.full_name(),
            abbrev: aw.abbrev.clone(),
            segment_values: rr
                .linescore
                .by_period
                .iter()
                .map(|p| p.away.to_string())
                .collect(),
            total: aw.score.to_string(),
        },
        home: LineRow {
            team: hm.full_name(),
            abbrev: hm.abbrev.clone(),
            segment_values: rr
                .linescore
                .by_period
                .iter()
                .map(|p| p.home.to_string())
                .collect(),
            total: hm.score.to_string(),
        },
        totals,
    };

    // Team stats comparison.
    let label_for = |cat: &str| -> Option<&'static str> {
        Some(match cat {
            "sog" => "Shots",
            "faceoffWinningPctg" => "Faceoff %",
            "powerPlay" => "Power play",
            "pim" => "Penalty minutes",
            "hits" => "Hits",
            "blockedShots" => "Blocked shots",
            "giveaways" => "Giveaways",
            "takeaways" => "Takeaways",
            _ => return None,
        })
    };
    let team_stats = rr
        .team_game_stats
        .iter()
        .filter_map(|t| {
            let label = label_for(&t.category)?;
            let (a, h) = (val_str(&t.away_value), val_str(&t.home_value));
            Some(StatPair {
                away_share: stat_share(&a, &h),
                label: label.into(),
                away: a,
                home: h,
            })
        })
        .collect();

    // Three stars → leaders.
    let leaders = landing
        .summary
        .three_stars
        .iter()
        .map(|s| {
            let line = if s.position == "G" {
                format!("{} SV, {:.3} SV%", s.saves, s.save_pctg)
            } else {
                format!("{}G {}A", s.goals, s.assists)
            };
            LeaderCard {
                name: s.name.default.clone(),
                team: s.team_abbrev.clone(),
                category: format!("★{}", s.star),
                line,
            }
        })
        .collect();

    // Player tables.
    let player_tables = vec![
        skater_table(&aw.abbrev, &bs.players.away),
        goalie_table(&aw.abbrev, &bs.players.away),
        skater_table(&hm.abbrev, &bs.players.home),
        goalie_table(&hm.abbrev, &bs.players.home),
    ];

    // Goals timeline.
    let timeline = landing
        .summary
        .scoring
        .iter()
        .flat_map(|per| {
            let seg = period_label(&per.period);
            per.goals.iter().map(move |g| {
                let scorer = format!("{} {}", g.first_name.default, g.last_name.default);
                let assists: Vec<String> = g
                    .assists
                    .iter()
                    .map(|a| format!("{} {}", a.first_name.default, a.last_name.default))
                    .collect();
                let desc = if assists.is_empty() {
                    format!("{scorer} ({})", g.strength)
                } else {
                    format!("{scorer} ({}) — {}", g.strength, assists.join(", "))
                };
                ScoreEvent {
                    segment: seg.clone(),
                    clock: g.time_in_period.clone(),
                    team: g.team_abbrev.default.clone(),
                    kind: "goal".into(),
                    description: desc,
                }
            })
        })
        .collect();

    BoxScore {
        line: Some(line),
        team_stats,
        leaders,
        player_tables,
        timeline,
        games: Vec::new(),
        unavailable: false,
    }
}

#[cfg(all(test, feature = "ssr"))]
mod boxscore_tests {
    use super::*;

    #[test]
    fn nhl_to_box_score_maps_line_stats_stars_and_players() {
        let landing: RawLanding =
            serde_json::from_str(include_str!("testdata/nhl_landing_2025021301.json")).unwrap();
        let rr: RawRightRail =
            serde_json::from_str(include_str!("testdata/nhl_rightrail_2025021301.json")).unwrap();
        let bs: RawNhlBox =
            serde_json::from_str(include_str!("testdata/nhl_boxscore_2025021301.json")).unwrap();
        let out = to_box_score(&landing, &rr, &bs);

        let line = out.line.expect("line score");
        assert!(line.segments.len() >= 3, "at least three periods");

        // Team-stat comparison includes Shots with a computed lead share.
        let shots = out
            .team_stats
            .iter()
            .find(|s| s.label == "Shots")
            .expect("Shots row");
        assert!(shots.away_share.is_some());

        // Three stars → leaders.
        assert!(!out.leaders.is_empty(), "three stars yield leaders");

        // Skater + goalie tables per team (4 total).
        assert_eq!(out.player_tables.len(), 4);
        assert!(out
            .player_tables
            .iter()
            .any(|t| t.title.starts_with("Skaters")));
        assert!(out
            .player_tables
            .iter()
            .any(|t| t.title.starts_with("Goalies")));
        let skaters = out
            .player_tables
            .iter()
            .find(|t| t.title.starts_with("Skaters"))
            .unwrap();
        assert!(!skaters.rows.is_empty(), "skater rows populated");
        assert_eq!(
            skaters.rows[0].values.len(),
            skaters.columns.len(),
            "row aligns to columns"
        );

        let goalies = out
            .player_tables
            .iter()
            .find(|t| t.title.starts_with("Goalies"))
            .unwrap();
        assert!(!goalies.rows.is_empty(), "goalie rows populated");
        assert_eq!(
            goalies.rows[0].values.len(),
            goalies.columns.len(),
            "goalie row aligns to columns"
        );

        // Goals timeline populated for a finished game.
        assert!(!out.timeline.is_empty(), "scoring timeline populated");
        assert!(!out.unavailable);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Live smoke test — `cargo test --features ssr -- --ignored nhl_live_bracket
    /// --nocapture`. Prints the assembled bracket; asserts the two conferences feed
    /// the Cup final.
    #[tokio::test]
    #[ignore = "hits the live NHL API"]
    async fn nhl_live_bracket() {
        let client = reqwest::Client::new();
        let rounds = fetch_bracket(&client, 2025).await.unwrap();
        for r in &rounds {
            println!("[{}] {} ({} series)", r.section, r.title, r.matches.len());
            for (i, m) in r.matches.iter().enumerate() {
                println!(
                    "   {i}: {} {:?} vs {} {:?} win={} feeders={:?}",
                    m.team_a, m.score_a, m.team_b, m.score_b, m.winner, m.feeders
                );
            }
        }
        assert!(!rounds.is_empty());
        let sections: Vec<&str> = rounds.iter().map(|r| r.section.as_str()).collect();
        assert!(sections.contains(&"east") && sections.contains(&"west"));
        assert!(rounds.iter().any(|r| r.section == "final"));
    }

    fn b(network: &str, market: &str, country: &str, seq: i64) -> RawBroadcast {
        RawBroadcast {
            network: network.into(),
            market: market.into(),
            country: country.into(),
            sequence: seq,
        }
    }

    #[test]
    fn nhl_network_maps_nationals_passes_through_rsn() {
        assert_eq!(nhl_network("ESPN"), "ESPN");
        assert_eq!(nhl_network("TNT"), "TNT");
        assert_eq!(nhl_network("ABC"), "ABC");
        assert_eq!(nhl_network("NHLN"), "NHL Network");
        assert_eq!(nhl_network("SNP"), "Sportsnet");
        assert_eq!(nhl_network("SNW"), "Sportsnet");
        assert_eq!(nhl_network("TVAS"), "TVA Sports");
        assert_eq!(nhl_network("CBC"), "CBC");
        assert_eq!(nhl_network("FDSNSO"), "FDSNSO"); // unknown RSN → raw abbrev
    }

    #[test]
    fn broadcasts_national_us_ca_and_local_rsn() {
        let raw = vec![
            b("NESN", "H", "US", 405),
            b("ESPN", "N", "US", 10),
            b("SNP", "N", "CA", 33),
            b("MSG", "A", "US", 417),
        ];
        let sv = broadcasts(&raw);
        // National first (by sequence): ESPN then Sportsnet, then home, then away.
        assert_eq!(sv[0].name, "ESPN");
        assert!(sv[0].official && sv[0].url.contains("espn.com"));
        assert_eq!(sv[0].tag, "national · TV");
        assert_eq!(sv[1].name, "Sportsnet");
        assert!(sv[1].official && sv[1].url.contains("sportsnet.ca"));
        assert_eq!(sv[1].tag, "national · TV · CA"); // non-US marker
        let home = sv.iter().find(|s| s.name == "NESN").unwrap();
        assert!(!home.official && home.url.is_empty());
        assert_eq!(home.tag, "home · TV");
        assert_eq!(
            sv.iter().find(|s| s.name == "MSG").unwrap().tag,
            "away · TV"
        );
        assert!(sv.iter().all(|s| s.group == "tv"));
    }

    #[test]
    fn broadcasts_dedups_and_empty() {
        // Two ESPN national entries collapse to one; empty input → empty.
        assert_eq!(
            broadcasts(&[b("ESPN", "N", "US", 1), b("ESPN", "N", "US", 2)]).len(),
            1
        );
        assert!(broadcasts(&[]).is_empty());
    }

    #[test]
    fn parses_a_scheduled_and_a_final_game() {
        let json = r#"{"gameWeek":[{"date":"2025-10-08","games":[
          {"id":2025020001,"startTimeUTC":"2025-10-08T23:00:00Z","gameState":"FUT",
           "gameScheduleState":"OK","gameType":2,"venueTimezone":"America/Toronto",
           "awayTeam":{"commonName":{"default":"Canadiens"},"placeName":{"default":"Montréal"},"abbrev":"MTL"},
           "homeTeam":{"commonName":{"default":"Maple Leafs"},"placeName":{"default":"Toronto"},"abbrev":"TOR"}},
          {"id":2025020002,"startTimeUTC":"2025-10-09T02:00:00Z","gameState":"OFF",
           "gameScheduleState":"OK","gameType":2,"venueTimezone":"America/Edmonton",
           "awayTeam":{"commonName":{"default":"Flames"},"placeName":{"default":"Calgary"},"abbrev":"CGY","score":2},
           "homeTeam":{"commonName":{"default":"Oilers"},"placeName":{"default":"Edmonton"},"abbrev":"EDM","score":5}}
        ]}]}"#;
        let resp: ScheduleResp = serde_json::from_str(json).unwrap();
        let games: Vec<NormalizedMatch> = resp
            .game_week
            .into_iter()
            .flat_map(|d| d.games)
            .filter_map(to_match)
            .collect();
        assert_eq!(games.len(), 2);
        assert_eq!(games[0].sport, Sport::Nhl);
        assert_eq!(games[0].team_a.label, "Canadiens");
        assert_eq!(games[0].team_b.name, "Toronto Maple Leafs");
        assert_eq!(games[0].status, MatchStatus::Upcoming);
        assert_eq!(games[0].team_a.score, None);
        assert_eq!(games[0].venue_tz.as_deref(), Some("America/Toronto"));
        assert_eq!(games[1].status, MatchStatus::Finished);
        assert_eq!(games[1].team_a.score, Some(2));
        assert_eq!(games[1].team_b.score, Some(5));
    }

    #[test]
    fn builds_division_tables_in_order() {
        let json = r#"{"standings":[
          {"teamName":{"default":"Colorado Avalanche"},"divisionName":"Central","divisionSequence":1,"points":121,"wins":55,"losses":16,"otLosses":11},
          {"teamName":{"default":"Carolina Hurricanes"},"divisionName":"Metropolitan","divisionSequence":1,"points":113,"wins":52,"losses":21,"otLosses":9},
          {"teamName":{"default":"Toronto Maple Leafs"},"divisionName":"Atlantic","divisionSequence":1,"points":100,"wins":45,"losses":25,"otLosses":10}
        ]}"#;
        let resp: StandingsResp = serde_json::from_str(json).unwrap();
        let divs = divisions_from(resp);
        // Eastern first: Atlantic, Metropolitan, then Central (Western).
        let names: Vec<&str> = divs.iter().map(|d| d.stage.as_str()).collect();
        assert_eq!(names, ["Atlantic", "Metropolitan", "Central"]);
        let central = divs.iter().find(|d| d.stage == "Central").unwrap();
        assert_eq!(central.standings[0].team, "Colorado Avalanche");
        assert_eq!(central.standings[0].gb, "121"); // points in the last column
        assert_eq!(central.standings[0].ties, 11); // OT losses
        assert_eq!(central.sport, Sport::Nhl);
    }
}
