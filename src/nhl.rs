//! NHL schedule + standings from the keyless NHL Web API (`api-web.nhle.com`),
//! normalized into the same [`NormalizedMatch`] model as MLB and the esports
//! feeds. Two teams per sport (away at home), behind the esports/traditional
//! toggle. The schedule endpoint returns a week at a time, so a date-range fetch
//! walks it a week at a time and de-dupes.

use crate::feed::{NormalizedMatch, NormalizedTeam};
use crate::types::{EventInfo, MatchStatus, Sport, StandingRow};
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
                if let Some(m) = to_match(g) {
                    let d = m.begin_at.date_naive();
                    if d >= start && d <= end && seen.insert(m.id) {
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

#[cfg(test)]
mod tests {
    use super::*;

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
