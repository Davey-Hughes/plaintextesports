//! MLB schedule from the official, free MLB Stats API (statsapi.mlb.com — no key
//! required), normalized into the same [`NormalizedMatch`] model as the esports
//! feeds so it flows through the existing schedule UI unchanged.

use crate::pandascore::{NormTeam, NormalizedMatch};
use crate::types::{EventInfo, Game, MatchStatus, StandingRow, StreamView};
use chrono::{DateTime, NaiveDate, Utc};
use serde::Deserialize;
use std::collections::HashSet;

const BASE: &str = "https://statsapi.mlb.com/api/v1";

#[derive(Deserialize)]
struct ScheduleResp {
    #[serde(default)]
    dates: Vec<DateBlock>,
}

#[derive(Deserialize)]
struct DateBlock {
    #[serde(default)]
    games: Vec<RawGame>,
}

#[derive(Deserialize)]
struct RawGame {
    #[serde(rename = "gamePk")]
    game_pk: i64,
    #[serde(rename = "gameDate", default)]
    game_date: String,
    #[serde(default)]
    status: RawStatus,
    #[serde(default)]
    teams: RawTeams,
    #[serde(rename = "seriesDescription", default)]
    series: String,
    #[serde(default)]
    broadcasts: Vec<RawBroadcast>,
    #[serde(default)]
    venue: RawVenue,
}

#[derive(Deserialize, Default)]
struct RawVenue {
    #[serde(rename = "timeZone", default)]
    time_zone: RawTimeZone,
}

#[derive(Deserialize, Default)]
struct RawTimeZone {
    /// IANA id, e.g. "America/Detroit" (from `hydrate=venue(timezone)`).
    #[serde(default)]
    id: String,
}

#[derive(Deserialize, Default)]
struct RawBroadcast {
    #[serde(default)]
    name: String,
    /// "TV", "AM", "FM", "Streaming", …
    #[serde(rename = "type", default)]
    kind: String,
    /// "home", "away", or "national".
    #[serde(rename = "homeAway", default)]
    home_away: String,
    #[serde(rename = "isNational", default)]
    is_national: bool,
}

#[derive(Deserialize, Default)]
struct RawStatus {
    #[serde(rename = "abstractGameState", default)]
    abstract_state: String,
    #[serde(rename = "detailedState", default)]
    detailed_state: String,
}

#[derive(Deserialize, Default)]
struct RawTeams {
    #[serde(default)]
    away: RawSide,
    #[serde(default)]
    home: RawSide,
}

#[derive(Deserialize, Default)]
struct RawSide {
    #[serde(default)]
    score: Option<i64>,
    #[serde(default)]
    team: RawTeamRef,
}

#[derive(Deserialize, Default)]
struct RawTeamRef {
    #[serde(default)]
    name: String,
    #[serde(rename = "teamName", default)]
    team_name: String,
    #[serde(default)]
    abbreviation: String,
}

impl RawTeamRef {
    /// Prefer the nickname ("Rangers"), then the abbreviation, then the full name.
    fn label(&self) -> String {
        if !self.team_name.is_empty() {
            self.team_name.clone()
        } else if !self.abbreviation.is_empty() {
            self.abbreviation.clone()
        } else {
            self.name.clone()
        }
    }

    /// Full name ("Miami Marlins") to key the team page/subscription; falls back
    /// to the short label when absent.
    fn full_name(&self) -> String {
        if self.name.is_empty() {
            self.label()
        } else {
            self.name.clone()
        }
    }
}

fn status_of(s: &RawStatus) -> MatchStatus {
    let d = s.detailed_state.to_lowercase();
    if d.contains("postpone") || d.contains("cancel") || d.contains("suspend") || d.contains("forfeit") {
        return MatchStatus::Canceled;
    }
    match s.abstract_state.as_str() {
        "Final" => MatchStatus::Finished,
        "Live" => MatchStatus::Live,
        _ => MatchStatus::Upcoming, // "Preview"
    }
}

/// Drop a trailing sponsor clause ("Rangers Sports Network, presented by …")
/// so the broadcast label stays short.
fn clean_network(name: &str) -> String {
    let lower = name.to_lowercase();
    match lower.find(" presented by ") {
        Some(i) => name[..i].trim_end_matches([',', ' ']).to_string(),
        None => name.to_string(),
    }
}

/// Sort key: TV before radio, national before local, home before away. Lower
/// sorts first, so the most useful "where to watch" entries lead the list.
fn broadcast_rank(b: &RawBroadcast) -> (i32, i32, i32) {
    let tv = if b.kind.eq_ignore_ascii_case("TV") { 0 } else { 1 };
    let nat = if b.is_national || b.home_away == "national" { 0 } else { 1 };
    let side = match b.home_away.as_str() {
        "national" => 0,
        "home" => 1,
        "away" => 2,
        _ => 3,
    };
    (tv, nat, side)
}

/// Turn the API's broadcast list into [`StreamView`]s — link-less entries that
/// carry a network `name` and a "national · TV" style `tag`. Deduped by
/// name+medium, ordered TV-then-radio / national-then-local.
fn broadcasts(raw: &[RawBroadcast]) -> Vec<StreamView> {
    let mut sorted: Vec<&RawBroadcast> = raw.iter().collect();
    sorted.sort_by_key(|b| broadcast_rank(b));
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for b in sorted {
        let name = clean_network(&b.name);
        if name.is_empty() {
            continue;
        }
        let medium = if b.kind.eq_ignore_ascii_case("TV") { "TV" } else { "radio" };
        if !seen.insert(format!("{name}|{medium}")) {
            continue;
        }
        let scope = if b.is_national || b.home_away == "national" {
            "national"
        } else if b.home_away == "home" {
            "home"
        } else if b.home_away == "away" {
            "away"
        } else {
            ""
        };
        let tag = if scope.is_empty() {
            medium.to_string()
        } else {
            format!("{scope} · {medium}")
        };
        out.push(StreamView {
            url: String::new(),
            language: String::new(),
            official: b.is_national,
            main: medium == "TV",
            name,
            tag,
        });
    }
    out
}

fn to_match(g: RawGame) -> Option<NormalizedMatch> {
    let begin_at = DateTime::parse_from_rfc3339(&g.game_date).ok()?.with_timezone(&Utc);
    Some(NormalizedMatch {
        id: g.game_pk,
        game: Game::Mlb,
        league: "MLB".to_string(),
        league_url: None,
        // A regular-season game is just "MLB"; the postseason/all-star names its
        // round. Strip a redundant leading "MLB " (the API's All-Star series is
        // "MLB All-Star Game") so the league+serie heading doesn't read "MLB MLB …".
        serie_name: if g.series.eq_ignore_ascii_case("Regular Season") {
            String::new()
        } else {
            g.series
                .strip_prefix("MLB ")
                .unwrap_or(&g.series)
                .to_string()
        },
        tier: "S".to_string(),
        begin_at,
        status: status_of(&g.status),
        // No best-of label: each MLB game is a single game, not an esports series.
        best_of: None,
        team_a: NormTeam {
            label: g.teams.away.team.label(),
            name: g.teams.away.team.full_name(),
            score: g.teams.away.score,
        },
        team_b: NormTeam {
            label: g.teams.home.team.label(),
            name: g.teams.home.team.full_name(),
            score: g.teams.home.score,
        },
        stream_url: None,
        tournament_id: None,
        // The stadium's IANA timezone, so the UI can show the local time at the
        // ballpark (handles cross-country and neutral-site games).
        venue_tz: Some(g.venue.time_zone.id).filter(|s| !s.is_empty()),
        streams: broadcasts(&g.broadcasts),
    })
}

/// Fetch every MLB game in the inclusive date range (UTC days), normalized.
pub async fn fetch_schedule(
    client: &reqwest::Client,
    start: NaiveDate,
    end: NaiveDate,
) -> Result<Vec<NormalizedMatch>, reqwest::Error> {
    let url = format!(
        "{BASE}/schedule?sportId=1&startDate={}&endDate={}&hydrate=team,broadcasts(all),venue(timezone)",
        start.format("%Y-%m-%d"),
        end.format("%Y-%m-%d"),
    );
    let resp: ScheduleResp = client.get(&url).send().await?.error_for_status()?.json().await?;
    Ok(resp.dates.into_iter().flat_map(|d| d.games).filter_map(to_match).collect())
}

// ----- Standings -----------------------------------------------------------

#[derive(Deserialize, Default)]
struct StandingsResp {
    #[serde(default)]
    records: Vec<DivisionRecord>,
}

#[derive(Deserialize, Default)]
struct DivisionRecord {
    #[serde(default)]
    division: DivisionRef,
    #[serde(rename = "teamRecords", default)]
    team_records: Vec<TeamRecord>,
}

#[derive(Deserialize, Default)]
struct DivisionRef {
    #[serde(default)]
    id: i64,
}

#[derive(Deserialize, Default)]
struct TeamRecord {
    #[serde(default)]
    team: RawTeamRef,
    #[serde(default)]
    wins: i32,
    #[serde(default)]
    losses: i32,
    #[serde(rename = "divisionRank", default)]
    division_rank: String,
    #[serde(rename = "gamesBack", default)]
    games_back: String,
}

/// Short name for an MLB division id (200–205 are fixed).
fn division_name(id: i64) -> &'static str {
    match id {
        200 => "AL West",
        201 => "AL East",
        202 => "AL Central",
        203 => "NL West",
        204 => "NL East",
        205 => "NL Central",
        _ => "",
    }
}

/// Display order: AL East/Central/West, then NL East/Central/West.
fn division_order(id: i64) -> i32 {
    match id {
        201 => 0,
        202 => 1,
        200 => 2,
        204 => 3,
        205 => 4,
        203 => 5,
        _ => 9,
    }
}

/// Build one [`EventInfo`] (a standings table) per division from a standings
/// response, ordered AL-then-NL. The division id doubles as the table's
/// `tournament_id` so its reveal state is stable and per-division.
fn divisions_from(resp: StandingsResp) -> Vec<EventInfo> {
    let mut divisions: Vec<(i32, EventInfo)> = resp
        .records
        .into_iter()
        .filter_map(|rec| {
            let name = division_name(rec.division.id);
            if name.is_empty() {
                return None;
            }
            let mut rows: Vec<StandingRow> = rec
                .team_records
                .into_iter()
                .map(|t| StandingRow {
                    rank: t.division_rank.parse().unwrap_or(0),
                    team: t.team.label(),
                    wins: t.wins,
                    losses: t.losses,
                    ties: 0,
                    game_wins: 0,
                    game_losses: 0,
                    gb: if t.games_back == "-" { "—".to_string() } else { t.games_back },
                })
                .collect();
            rows.sort_by_key(|r| r.rank);
            Some((
                division_order(rec.division.id),
                EventInfo {
                    event: "MLB".to_string(),
                    tournament_id: rec.division.id,
                    stage: name.to_string(),
                    game: Game::Mlb,
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

/// Fetch the current MLB division standings (both leagues), one table per
/// division. Keyless, like the schedule feed.
pub async fn fetch_standings(
    client: &reqwest::Client,
    season: i32,
) -> Result<Vec<EventInfo>, reqwest::Error> {
    let url = format!(
        "{BASE}/standings?leagueId=103,104&season={season}&standingsTypes=regularSeason&hydrate=team",
    );
    let resp: StandingsResp = client.get(&url).send().await?.error_for_status()?.json().await?;
    Ok(divisions_from(resp))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_scheduled_and_a_final_game() {
        let json = r#"{"dates":[{"games":[
          {"gamePk":1,"gameDate":"2026-06-24T16:10:00Z","seriesDescription":"Regular Season",
           "status":{"abstractGameState":"Preview","detailedState":"Scheduled"},
           "teams":{"away":{"team":{"name":"Texas Rangers","teamName":"Rangers","abbreviation":"TEX"}},
                    "home":{"team":{"name":"Miami Marlins","teamName":"Marlins","abbreviation":"MIA"}}}},
          {"gamePk":2,"gameDate":"2026-06-24T23:10:00Z","seriesDescription":"Regular Season",
           "status":{"abstractGameState":"Final","detailedState":"Final"},
           "teams":{"away":{"score":5,"team":{"name":"Chicago Cubs","teamName":"Cubs","abbreviation":"CHC"}},
                    "home":{"score":3,"team":{"name":"New York Mets","teamName":"Mets","abbreviation":"NYM"}}}}
        ]}]}"#;
        let resp: ScheduleResp = serde_json::from_str(json).unwrap();
        let games: Vec<NormalizedMatch> =
            resp.dates.into_iter().flat_map(|d| d.games).filter_map(to_match).collect();
        assert_eq!(games.len(), 2);
        assert_eq!(games[0].game, Game::Mlb);
        assert_eq!(games[0].team_a.label, "Rangers");
        assert_eq!(games[0].team_b.label, "Marlins");
        assert_eq!(games[0].status, MatchStatus::Upcoming);
        assert_eq!(games[0].team_a.score, None);
        assert_eq!(games[1].status, MatchStatus::Finished);
        assert_eq!(games[1].team_a.score, Some(5));
        assert_eq!(games[1].team_b.score, Some(3));
    }

    #[test]
    fn broadcasts_strip_sponsor_dedup_and_order() {
        let raw = vec![
            RawBroadcast {
                name: "560AM WQAM".into(),
                kind: "AM".into(),
                home_away: "home".into(),
                is_national: false,
            },
            RawBroadcast {
                name: "Rangers Sports Network, presented by Progressive".into(),
                kind: "TV".into(),
                home_away: "away".into(),
                is_national: false,
            },
            RawBroadcast {
                name: "FOX".into(),
                kind: "TV".into(),
                home_away: "national".into(),
                is_national: true,
            },
            // Duplicate of the away TV (same network + medium) → dropped.
            RawBroadcast {
                name: "Rangers Sports Network presented by Progressive".into(),
                kind: "TV".into(),
                home_away: "away".into(),
                is_national: false,
            },
        ];
        let out = broadcasts(&raw);
        // National TV first, then away TV, then the radio entry.
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].name, "FOX");
        assert_eq!(out[0].tag, "national · TV");
        assert_eq!(out[1].name, "Rangers Sports Network");
        assert_eq!(out[1].tag, "away · TV");
        assert_eq!(out[2].name, "560AM WQAM");
        assert_eq!(out[2].tag, "home · radio");
        // Broadcasts are link-less.
        assert!(out.iter().all(|s| s.url.is_empty()));
    }

    #[test]
    fn standings_group_by_division_and_order() {
        let json = r#"{"records":[
          {"division":{"id":204},"teamRecords":[
            {"team":{"teamName":"Mets"},"wins":50,"losses":30,"divisionRank":"1","gamesBack":"-"},
            {"team":{"teamName":"Phillies"},"wins":45,"losses":35,"divisionRank":"2","gamesBack":"5.0"}]},
          {"division":{"id":201},"teamRecords":[
            {"team":{"teamName":"Yankees"},"wins":47,"losses":31,"divisionRank":"1","gamesBack":"-"}]}
        ]}"#;
        let resp: StandingsResp = serde_json::from_str(json).unwrap();
        let divs = divisions_from(resp);
        assert_eq!(divs.len(), 2);
        // AL East (201) sorts before NL East (204).
        assert_eq!(divs[0].stage, "AL East");
        assert_eq!(divs[0].tournament_id, 201);
        assert_eq!(divs[0].standings[0].team, "Yankees");
        assert_eq!(divs[0].standings[0].gb, "—"); // "-" leader → em dash
        assert_eq!(divs[1].stage, "NL East");
        assert_eq!(divs[1].standings.len(), 2);
        assert_eq!(divs[1].standings[0].team, "Mets");
        assert_eq!(divs[1].standings[1].gb, "5.0");
    }
}
