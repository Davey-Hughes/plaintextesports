//! PandaScore REST client + normalization (server-only).
//!
//! Fetches upcoming/running/recent matches for each game, deserializes the
//! relevant fields, and normalizes them into [`NormalizedMatch`] after applying
//! the tier-1 filter. CS2 lives under the `/csgo/` path prefix; LoL under
//! `/lol/`. Docs: https://developers.pandascore.co

use crate::tiering::{is_tier_one, TierInput};
use crate::types::{Game, MatchStatus};
use chrono::{DateTime, Utc};
use serde::Deserialize;

const BASE_URL: &str = "https://api.pandascore.co";

type DynError = Box<dyn std::error::Error + Send + Sync>;

/// Result of fetching one game's tier-1 matches.
pub type FetchResult = Result<Vec<NormalizedMatch>, DynError>;

/// A match after deserialization, normalization, and tier-1 filtering.
#[derive(Debug, Clone)]
pub struct NormalizedMatch {
    pub id: i64,
    pub game: Game,
    pub league: String,
    pub tier: String,
    pub begin_at: DateTime<Utc>,
    pub status: MatchStatus,
    pub best_of: Option<i64>,
    pub team_a: NormTeam,
    pub team_b: NormTeam,
    pub stream_url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NormTeam {
    pub label: String,
    pub score: Option<i64>,
}

// ----- Raw PandaScore JSON -------------------------------------------------

#[derive(Debug, Deserialize)]
struct RawMatch {
    id: i64,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    begin_at: Option<String>,
    #[serde(default)]
    scheduled_at: Option<String>,
    #[serde(default)]
    number_of_games: Option<i64>,
    #[serde(default)]
    league: Option<RawLeague>,
    #[serde(default)]
    serie: Option<RawSerie>,
    #[serde(default)]
    tournament: Option<RawTournament>,
    #[serde(default)]
    results: Vec<RawResult>,
    #[serde(default)]
    opponents: Vec<RawOpponent>,
    #[serde(default)]
    streams_list: Vec<RawStream>,
}

#[derive(Debug, Deserialize)]
struct RawLeague {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    slug: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawSerie {
    #[serde(default)]
    slug: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawTournament {
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    tier: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawResult {
    #[serde(default)]
    team_id: Option<i64>,
    #[serde(default)]
    score: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct RawOpponent {
    #[serde(default)]
    opponent: Option<RawTeam>,
}

#[derive(Debug, Deserialize)]
struct RawTeam {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    acronym: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawStream {
    #[serde(default)]
    main: Option<bool>,
    #[serde(default)]
    raw_url: Option<String>,
}

// ----- Parsing / normalization ---------------------------------------------

fn map_status(raw: Option<&str>) -> MatchStatus {
    match raw {
        Some("running") => MatchStatus::Live,
        Some("finished") => MatchStatus::Finished,
        Some("canceled" | "postponed") => MatchStatus::Canceled,
        _ => MatchStatus::Upcoming,
    }
}

fn team_label(team: Option<&RawTeam>) -> (String, Option<i64>) {
    match team {
        Some(t) => {
            let label = t
                .acronym
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .or(t.name.as_deref())
                .filter(|s| !s.trim().is_empty())
                .unwrap_or("TBD")
                .to_string();
            (label, t.id)
        }
        None => ("TBD".to_string(), None),
    }
}

fn score_for(results: &[RawResult], team_id: Option<i64>) -> Option<i64> {
    let id = team_id?;
    results
        .iter()
        .find(|r| r.team_id == Some(id))
        .and_then(|r| r.score)
}

fn parse_time(raw: &RawMatch) -> Option<DateTime<Utc>> {
    let s = raw.begin_at.as_deref().or(raw.scheduled_at.as_deref())?;
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Normalize a single raw match. Returns `None` when essential fields (time,
/// league) are missing.
fn normalize(game: Game, raw: &RawMatch) -> Option<NormalizedMatch> {
    let begin_at = parse_time(raw)?;
    let league = raw.league.as_ref().and_then(|l| l.name.clone())?;

    let (label_a, id_a) = team_label(raw.opponents.first().and_then(|o| o.opponent.as_ref()));
    let (label_b, id_b) = team_label(raw.opponents.get(1).and_then(|o| o.opponent.as_ref()));

    let tier = raw
        .tournament
        .as_ref()
        .and_then(|t| t.tier.as_deref())
        .unwrap_or("")
        .to_ascii_uppercase();

    let stream_url = raw
        .streams_list
        .iter()
        .find(|s| s.main == Some(true))
        .or_else(|| raw.streams_list.first())
        .and_then(|s| s.raw_url.clone())
        .filter(|u| !u.is_empty());

    Some(NormalizedMatch {
        id: raw.id,
        game,
        league,
        tier,
        begin_at,
        status: map_status(raw.status.as_deref()),
        best_of: raw.number_of_games,
        team_a: NormTeam {
            label: label_a,
            score: score_for(&raw.results, id_a),
        },
        team_b: NormTeam {
            label: label_b,
            score: score_for(&raw.results, id_b),
        },
        stream_url,
    })
}

fn tier_input(raw: &RawMatch) -> TierInput<'_> {
    TierInput {
        league_slug: raw.league.as_ref().and_then(|l| l.slug.as_deref()),
        serie_slug: raw.serie.as_ref().and_then(|s| s.slug.as_deref()),
        tournament_slug: raw.tournament.as_ref().and_then(|t| t.slug.as_deref()),
        tier: raw.tournament.as_ref().and_then(|t| t.tier.as_deref()),
    }
}

/// Deserialize a PandaScore matches array, normalize, and keep only tier-1.
pub fn parse_and_filter(game: Game, json: &str) -> Result<Vec<NormalizedMatch>, serde_json::Error> {
    let raw: Vec<RawMatch> = serde_json::from_str(json)?;
    Ok(raw
        .iter()
        .filter(|m| is_tier_one(game, &tier_input(m)))
        .filter_map(|m| normalize(game, m))
        .collect())
}

// ----- HTTP ----------------------------------------------------------------

const fn game_path(game: Game) -> &'static str {
    match game {
        Game::Cs2 => "csgo",
        Game::Lol => "lol",
    }
}

async fn get_segment(
    client: &reqwest::Client,
    token: &str,
    game: Game,
    segment: &str,
    query: &[(&str, &str)],
) -> Result<Vec<NormalizedMatch>, DynError> {
    let url = format!("{BASE_URL}/{}/matches/{segment}", game_path(game));
    let body = client
        .get(&url)
        .bearer_auth(token)
        .query(query)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    Ok(parse_and_filter(game, &body)?)
}

/// Fetch tier-1 matches for one game: upcoming (required) plus best-effort
/// running and recent finished. Errors on the optional segments are logged and
/// swallowed so a free-tier 403 on `/running` or `/past` doesn't sink the poll.
pub async fn fetch_game(client: &reqwest::Client, token: &str, game: Game) -> FetchResult {
    use std::collections::HashMap;

    let mut by_id: HashMap<i64, NormalizedMatch> = HashMap::new();

    // Upcoming is the core feed; a failure here fails the whole game fetch.
    for m in get_segment(
        client,
        token,
        game,
        "upcoming",
        &[("per_page", "100"), ("sort", "begin_at")],
    )
    .await?
    {
        by_id.insert(m.id, m);
    }

    // Running + recent past are best-effort (often unavailable on free tier).
    for segment in ["running", "past"] {
        let query: &[(&str, &str)] = if segment == "past" {
            &[("per_page", "30"), ("sort", "-begin_at")]
        } else {
            &[("per_page", "50")]
        };
        match get_segment(client, token, game, segment, query).await {
            Ok(matches) => {
                for m in matches {
                    by_id.insert(m.id, m);
                }
            }
            Err(e) => {
                leptos::logging::log!("pandascore {}/{segment} unavailable: {e}", game.slug());
            }
        }
    }

    Ok(by_id.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = include_str!("../fixtures/pandascore_sample.json");

    #[test]
    fn deserializes_and_normalizes_sample() {
        let matches = parse_and_filter(Game::Lol, SAMPLE).expect("valid sample json");
        // The sample has one tier-A LCK match and one denylisted challenger
        // match; only the LCK one survives the filter.
        assert_eq!(matches.len(), 1);
        let m = &matches[0];
        assert_eq!(m.league, "LCK");
        assert_eq!(m.tier, "A");
        assert_eq!(m.best_of, Some(3));
        assert_eq!(m.team_a.label, "T1");
        assert_eq!(m.team_b.label, "GEN");
        // Scores mapped from results by team_id.
        assert_eq!(m.team_a.score, Some(2));
        assert_eq!(m.team_b.score, Some(1));
        assert_eq!(m.status, MatchStatus::Finished);
        assert!(m.stream_url.as_deref().unwrap().contains("twitch"));
    }

    #[test]
    fn handles_empty_array() {
        let matches = parse_and_filter(Game::Cs2, "[]").expect("empty array");
        assert!(matches.is_empty());
    }
}
