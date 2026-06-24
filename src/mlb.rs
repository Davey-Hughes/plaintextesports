//! MLB schedule from the official, free MLB Stats API (statsapi.mlb.com — no key
//! required), normalized into the same [`NormalizedMatch`] model as the esports
//! feeds so it flows through the existing schedule UI unchanged.

use crate::pandascore::{NormTeam, NormalizedMatch};
use crate::types::{Game, MatchStatus};
use chrono::{DateTime, NaiveDate, Utc};
use serde::Deserialize;

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
        team_a: NormTeam { label: g.teams.away.team.label(), score: g.teams.away.score },
        team_b: NormTeam { label: g.teams.home.team.label(), score: g.teams.home.score },
        stream_url: None,
        tournament_id: None,
        streams: Vec::new(),
    })
}

/// Fetch every MLB game in the inclusive date range (UTC days), normalized.
pub async fn fetch_schedule(
    client: &reqwest::Client,
    start: NaiveDate,
    end: NaiveDate,
) -> Result<Vec<NormalizedMatch>, reqwest::Error> {
    let url = format!(
        "{BASE}/schedule?sportId=1&startDate={}&endDate={}&hydrate=team",
        start.format("%Y-%m-%d"),
        end.format("%Y-%m-%d"),
    );
    let resp: ScheduleResp = client.get(&url).send().await?.error_for_status()?.json().await?;
    Ok(resp.dates.into_iter().flat_map(|d| d.games).filter_map(to_match).collect())
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
}
