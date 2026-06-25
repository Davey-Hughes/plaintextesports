//! NFL + NBA schedule and standings from ESPN's keyless public API
//! (`site.api.espn.com`). Both leagues share one shape — a date-range scoreboard
//! for the schedule and conference-grouped standings — so this one module serves
//! both, parameterized by [`EspnLeague`]. Normalized into the same
//! [`NormalizedMatch`] model as MLB/NHL (two teams, away at home).
//!
//! ESPN doesn't expose a venue IANA timezone, so these games carry no venue-time
//! toggle (like esports); the start time still shows in the viewer's zone.

use crate::pandascore::{NormTeam, NormalizedMatch};
use crate::types::{EventInfo, Game, MatchStatus, StandingRow};
use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, Utc};
use serde::Deserialize;

const SITE: &str = "https://site.api.espn.com/apis/site/v2/sports";
const CORE: &str = "https://site.api.espn.com/apis/v2/sports";

/// One ESPN-backed league (NFL or NBA) and the bits that differ between them.
pub struct EspnLeague {
    pub game: Game,
    pub name: &'static str,
    /// ESPN URL path parts, e.g. ("football", "nfl").
    sport: &'static str,
    league: &'static str,
    /// The standings stat shown in the last table column (its `displayValue`).
    last_stat: &'static str,
    /// The numeric stat the table is ordered by, descending (wins for the North
    /// American leagues, points for soccer).
    rank_stat: &'static str,
    /// Base id for the conference standings tables (id = base + conference index).
    standings_base: i64,
}

pub const NFL: EspnLeague = EspnLeague {
    game: Game::Nfl,
    name: "NFL",
    sport: "football",
    league: "nfl",
    last_stat: "winPercent",
    rank_stat: "wins",
    standings_base: 500,
};

pub const NBA: EspnLeague = EspnLeague {
    game: Game::Nba,
    name: "NBA",
    sport: "basketball",
    league: "nba",
    last_stat: "gamesBehind",
    rank_stat: "wins",
    standings_base: 400,
};

pub const EPL: EspnLeague = EspnLeague {
    game: Game::Soccer,
    name: "Premier League",
    sport: "soccer",
    league: "eng.1",
    // Soccer ranks by points; the table shows W-D-L (draws ride the ties slot).
    last_stat: "points",
    rank_stat: "points",
    standings_base: 700,
};

pub const WORLD_CUP: EspnLeague = EspnLeague {
    game: Game::Soccer,
    name: "World Cup",
    sport: "soccer",
    league: "fifa.world",
    last_stat: "points",
    rank_stat: "points",
    standings_base: 600,
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
pub fn season_year(game: Game, now: DateTime<Utc>) -> i32 {
    let (y, m) = (now.year(), now.month());
    match game {
        Game::Nfl => {
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
        let region = if a.state.is_empty() { &a.country } else { &a.state };
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
}

#[derive(Deserialize, Default)]
struct TeamRef {
    #[serde(rename = "shortDisplayName", default)]
    short_display_name: String,
    #[serde(rename = "displayName", default)]
    display_name: String,
    #[serde(default)]
    abbreviation: String,
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

/// Parse an ESPN timestamp, which omits seconds ("2026-01-15T19:00Z").
fn parse_date(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .ok()
        .or_else(|| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%MZ").ok().map(|n| n.and_utc()))
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
        lg.game,
        lg.name,
        begin_at,
        status_of(&e.status.r#type.state, &e.status.r#type.name),
        NormTeam {
            label: away.team.label(),
            name: away.team.full_name(),
            score: score(away),
        },
        NormTeam {
            label: home.team.label(),
            name: home.team.full_name(),
            score: score(home),
        },
    );
    m.venue_name = comp.venue.full_name.clone();
    m.venue_location = comp.venue.location();
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
        lg.sport,
        lg.league,
        start.format("%Y%m%d"),
        end.format("%Y%m%d"),
    );
    let resp: Scoreboard = client.get(&url).send().await?.error_for_status()?.json().await?;
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
        self.stats.iter().find(|s| s.name == name).map(|s| s.display_value.clone()).unwrap_or_default()
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
                game: lg.game,
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
        Some(s) => format!("{CORE}/{}/{}/standings?season={s}", lg.sport, lg.league),
        None => format!("{CORE}/{}/{}/standings", lg.sport, lg.league),
    };
    let resp: StandingsResp = client.get(&url).send().await?.error_for_status()?.json().await?;
    Ok(conferences_from(resp, lg))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let games: Vec<NormalizedMatch> =
            resp.events.into_iter().filter_map(|e| to_match(e, &NBA)).collect();
        assert_eq!(games.len(), 2);
        assert_eq!(games[0].game, Game::Nba);
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
        assert_eq!(season_year(Game::Nfl, jun_2026), 2025);
        assert_eq!(season_year(Game::Nba, jun_2026), 2026);
        let nov_2026 = "2026-11-15T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        assert_eq!(season_year(Game::Nfl, nov_2026), 2026);
        assert_eq!(season_year(Game::Nba, nov_2026), 2027);
    }
}
