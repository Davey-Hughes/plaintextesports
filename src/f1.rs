//! Formula 1 schedule from the free, keyless Jolpica API (the Ergast successor,
//! `api.jolpi.ca`). A Grand Prix weekend is a set of sessions (practice /
//! qualifying / sprint / race); we emit one [`NormalizedMatch`] per session,
//! grouped under the Grand Prix as the event (`league = "F1"`, `serie = the GP
//! name`). F1 is single-entity — a session has no opposing team — so the row's
//! one label is the session name (e.g. "Race"); results live on the event page.

use crate::pandascore::{NormTeam, NormalizedMatch};
use crate::types::{Game, MatchStatus};
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;

const BASE: &str = "https://api.jolpi.ca/ergast/f1";
const USER_AGENT: &str =
    "plaintextesports/0.1 (https://github.com/ralphpotato/plaintextesports; ralphpotato@gmail.com)";

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
            out.push(Session { ord, label, date: x.date.clone(), time: x.time.clone() });
        }
    };
    push(1, "Practice 1", &r.fp1);
    push(2, "Practice 2", &r.fp2);
    push(3, "Practice 3", &r.fp3);
    push(4, "Sprint Qualifying", &r.sprint_quali);
    push(5, "Sprint", &r.sprint);
    push(6, "Qualifying", &r.qualifying);
    // The race itself lives on the race object's own date/time.
    out.push(Session { ord: 7, label: "Race", date: r.date.clone(), time: r.time.clone() });
    out
}

fn to_matches(r: &RawRace, now: DateTime<Utc>) -> Vec<NormalizedMatch> {
    let season: i64 = r.season.parse().unwrap_or(0);
    let round: i64 = r.round.parse().unwrap_or(0);
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
            Some(NormalizedMatch {
                // Stable, collision-free id from (season, round, session order).
                id: season * 100_000 + round * 100 + s.ord,
                game: Game::F1,
                league: "F1".to_string(),
                league_url: None,
                // The Grand Prix is the event (e.g. "Austrian Grand Prix").
                serie_name: r.race_name.clone(),
                tier: "S".to_string(),
                begin_at,
                status,
                best_of: None,
                // Single-entity: the one label is the session; no opponent.
                team_a: NormTeam {
                    label: s.label.to_string(),
                    name: String::new(),
                    score: None,
                },
                team_b: NormTeam {
                    label: String::new(),
                    name: String::new(),
                    score: None,
                },
                stream_url: None,
                tournament_id: None,
                venue_tz: None,
                streams: Vec::new(),
                // MLB-only (the series ref); F1 has no opposing-team series.
                mlb_series: None,
            })
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
        let ms: Vec<NormalizedMatch> =
            resp.data.race_table.races.iter().flat_map(|r| to_matches(r, now)).collect();
        // FP1, FP2, FP3, Qualifying, Race — no sprint sessions this weekend.
        assert_eq!(ms.len(), 5);
        assert!(ms.iter().all(|m| m.game == Game::F1 && m.league == "F1"));
        assert!(ms.iter().all(|m| m.serie_name == "Austrian Grand Prix"));
        let labels: Vec<&str> = ms.iter().map(|m| m.team_a.label.as_str()).collect();
        assert_eq!(labels, ["Practice 1", "Practice 2", "Practice 3", "Qualifying", "Race"]);
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
        let ms: Vec<NormalizedMatch> =
            resp.data.race_table.races.iter().flat_map(|r| to_matches(r, now)).collect();
        let labels: Vec<&str> = ms.iter().map(|m| m.team_a.label.as_str()).collect();
        assert_eq!(labels, ["Practice 1", "Sprint Qualifying", "Sprint", "Qualifying", "Race"]);
    }
}
