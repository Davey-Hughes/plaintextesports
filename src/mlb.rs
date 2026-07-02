//! MLB schedule from the official, free MLB Stats API (statsapi.mlb.com — no key
//! required), normalized into the same [`NormalizedMatch`] model as the esports
//! feeds so it flows through the existing schedule UI unchanged.

use crate::feed::{NormalizedMatch, NormalizedTeam};
use crate::types::{
    stat_share, BoxScore, EventInfo, LeaderCard, LineRow, LineScore, MatchStatus, PlayerRow,
    PlayerTable, Sport, StandingRow, StatPair, StreamView,
};
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
    /// This game's 1-based position within its series, and the series length.
    #[serde(rename = "seriesGameNumber", default)]
    series_game_number: i32,
    #[serde(rename = "gamesInSeries", default)]
    games_in_series: i32,
    #[serde(default)]
    broadcasts: Vec<RawBroadcast>,
    #[serde(default)]
    venue: RawVenue,
}

#[derive(Deserialize, Default)]
struct RawVenue {
    #[serde(default)]
    name: String,
    #[serde(rename = "timeZone", default)]
    time_zone: RawTimeZone,
    #[serde(default)]
    location: RawVenueLocation,
}

#[derive(Deserialize, Default)]
struct RawTimeZone {
    /// IANA id, e.g. "America/Detroit" (from `hydrate=venue(timezone)`).
    #[serde(default)]
    id: String,
}

#[derive(Deserialize, Default)]
struct RawVenueLocation {
    #[serde(default)]
    city: String,
    #[serde(rename = "stateAbbrev", default)]
    state_abbrev: String,
    #[serde(default)]
    country: String,
}

impl RawVenueLocation {
    /// "City, ST" for US venues, else "City, Country" (e.g. "Toronto, Canada").
    fn label(&self) -> String {
        let region = if self.country == "USA" && !self.state_abbrev.is_empty() {
            &self.state_abbrev
        } else {
            &self.country
        };
        match (self.city.is_empty(), region.is_empty()) {
            (false, false) => format!("{}, {region}", self.city),
            (false, true) => self.city.clone(),
            _ => String::new(),
        }
    }
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
    id: i64,
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

/// The MLB team's SVG logo URL (mlbstatic serves one per team id), or empty
/// when the id is unknown.
fn team_logo(team_id: i64) -> String {
    if team_id > 0 {
        format!("https://www.mlbstatic.com/team-logos/{team_id}.svg")
    } else {
        String::new()
    }
}

fn status_of(s: &RawStatus) -> MatchStatus {
    let d = s.detailed_state.to_lowercase();
    if d.contains("postpone")
        || d.contains("cancel")
        || d.contains("suspend")
        || d.contains("forfeit")
    {
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
    let tv = if b.kind.eq_ignore_ascii_case("TV") {
        0
    } else {
        1
    };
    let nat = if b.is_national || b.home_away == "national" {
        0
    } else {
        1
    };
    let side = match b.home_away.as_str() {
        "national" => 0,
        "home" => 1,
        "away" => 2,
        _ => 3,
    };
    (tv, nat, side)
}

/// Turn the API's broadcast list into [`StreamView`]s, plus MLB.tv (every game
/// streams there). Deduped by name+medium and grouped TV → streaming → radio;
/// national broadcasters carry a watch link, local networks/radio are text
/// (the API provides no per-network URL).
fn broadcasts(raw: &[RawBroadcast]) -> Vec<StreamView> {
    let mut sorted: Vec<&RawBroadcast> = raw.iter().collect();
    sorted.sort_by_key(|b| broadcast_rank(b));
    let mut seen: HashSet<String> = HashSet::new();
    let (mut tv, mut radio) = (Vec::new(), Vec::new());
    for b in sorted {
        let name = clean_network(&b.name);
        if name.is_empty() {
            continue;
        }
        let is_tv = b.kind.eq_ignore_ascii_case("TV");
        let medium = if is_tv { "TV" } else { "radio" };
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
        let url = if b.is_national {
            crate::watch::national_watch_url(&name)
                .unwrap_or_default()
                .to_string()
        } else {
            String::new()
        };
        let sv = StreamView {
            url,
            language: String::new(),
            official: b.is_national,
            main: is_tv,
            name,
            tag,
            group: if is_tv { "tv" } else { "radio" }.to_string(),
            ..Default::default()
        };
        if is_tv {
            tv.push(sv);
        } else {
            radio.push(sv);
        }
    }
    // Every MLB game is on MLB.tv — the universal "where to watch", between the
    // broadcast-TV networks and the radio listings.
    let mlb_tv = StreamView {
        url: "https://www.mlb.com/tv".to_string(),
        language: String::new(),
        official: true,
        main: true,
        name: "MLB.tv".to_string(),
        tag: "streaming".to_string(),
        group: "streaming".to_string(),
        ..Default::default()
    };
    let mut out = tv;
    out.push(mlb_tv);
    out.extend(radio);
    out
}

fn to_match(g: RawGame) -> Option<NormalizedMatch> {
    let begin_at = DateTime::parse_from_rfc3339(&g.game_date)
        .ok()?
        .with_timezone(&Utc);
    let mut m = NormalizedMatch::team_sport(
        g.game_pk,
        Sport::Mlb,
        "MLB",
        begin_at,
        status_of(&g.status),
        NormalizedTeam {
            label: g.teams.away.team.label(),
            name: g.teams.away.team.full_name(),
            abbrev: g.teams.away.team.abbreviation.clone(),
            score: g.teams.away.score,
        },
        NormalizedTeam {
            label: g.teams.home.team.label(),
            name: g.teams.home.team.full_name(),
            abbrev: g.teams.home.team.abbreviation.clone(),
            score: g.teams.home.score,
        },
    );
    // A regular-season game is just "MLB"; the postseason/all-star names its
    // round. Strip a redundant leading "MLB " (the API's All-Star series is
    // "MLB All-Star Game") so the league+series heading doesn't read "MLB MLB …".
    m.series_name = if g.series.eq_ignore_ascii_case("Regular Season") {
        String::new()
    } else {
        g.series
            .strip_prefix("MLB ")
            .unwrap_or(&g.series)
            .to_string()
    };
    // The stadium's IANA timezone, so the UI can show the local time at the
    // ballpark (handles cross-country and neutral-site games).
    m.venue_tz = Some(g.venue.time_zone.id).filter(|s| !s.is_empty());
    m.venue_name = g.venue.name;
    m.venue_location = g.venue.location.label();
    // MLB serves an SVG logo per team id at this stable path.
    m.team_a_logo = team_logo(g.teams.away.team.id);
    m.team_b_logo = team_logo(g.teams.home.team.id);
    m.streams = broadcasts(&g.broadcasts);
    // Carry the two teams' ids + this game's series position so the detail page
    // can fetch the whole series between them. Only when both ids and a sane
    // series length are present.
    m.mlb_series = (g.teams.away.team.id > 0 && g.teams.home.team.id > 0 && g.games_in_series > 0)
        .then_some(crate::types::MlbSeriesRef {
            team_id_a: g.teams.away.team.id,
            team_id_b: g.teams.home.team.id,
            game_number: g.series_game_number,
            games_in_series: g.games_in_series,
        });
    Some(m)
}

/// Fetch every MLB game in the inclusive date range (UTC days), normalized.
pub async fn fetch_schedule(
    client: &reqwest::Client,
    start: NaiveDate,
    end: NaiveDate,
) -> Result<Vec<NormalizedMatch>, reqwest::Error> {
    let url = format!(
        "{BASE}/schedule?sportId=1&startDate={}&endDate={}&hydrate=team,broadcasts(all),venue(timezone,location)",
        start.format("%Y-%m-%d"),
        end.format("%Y-%m-%d"),
    );
    let resp: ScheduleResp = client
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(resp
        .dates
        .into_iter()
        .flat_map(|d| d.games)
        .filter_map(to_match)
        .collect())
}

// ----- Series (the multi-game set between two teams) ------------------------

/// The display labels for one series game's start: the date and time in the
/// viewer's tz (always mutually consistent), plus the same instant in the
/// ballpark's local tz as a full "date · time tz" string (empty when the venue tz
/// is unknown or identical to the viewer's). Built by the caller, which owns the
/// tz preference; see [`format_series`].
#[derive(Default)]
pub struct SeriesLabels {
    pub day: String,
    pub clock: String,
    pub venue: String,
}

/// A tz-agnostic series (cacheable, keyed by match id) — the raw data
/// `format_series` turns into a display [`crate::types::Series`] per request tz.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct RawSeries {
    pub games: Vec<RawSeriesGame>,
    pub game_label: String,
    pub record_label: String,
}

/// One game of a [`RawSeries`], with the UTC start + venue tz kept raw.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct RawSeriesGame {
    pub match_id: i64,
    pub begin_at: Option<DateTime<Utc>>,
    pub venue_tz: Option<String>,
    pub team_a: String,
    pub team_b: String,
    pub score_a: Option<i64>,
    pub score_b: Option<i64>,
    pub winner: String,
    pub status: MatchStatus,
    pub current: bool,
}

/// Build the [`RawSeries`] for the headline game `game_pk`, given the raw games
/// of the series (already fetched, in any order) and the headline orientation:
/// `(team_a_id, team_a_label)` is the headline's left side, `team_b_label` the
/// right (any game side that isn't `team_a_id` is team B). Games are oriented to
/// the headline (so each row reads `team_a` vs `team_b` consistently), ordered by
/// series game number, the current game flagged, and the spoiler-bearing record
/// computed from finished games. The UTC start + venue tz are kept raw so the
/// series can be cached once and re-formatted per request via [`format_series`].
fn build_raw_series(
    games: Vec<RawGame>,
    game_pk: i64,
    team_a_id: i64,
    team_a_label: &str,
    team_b_label: &str,
) -> RawSeries {
    // Keep only games of *this* series (same length + a real series number), so a
    // padded window can't pull in an adjacent series between the same two teams.
    let anchor_len = games
        .iter()
        .find(|g| g.game_pk == game_pk)
        .map(|g| g.games_in_series)
        .unwrap_or(0);
    let mut raw: Vec<RawGame> = games
        .into_iter()
        .filter(|g| {
            g.series_game_number > 0 && (anchor_len == 0 || g.games_in_series == anchor_len)
        })
        .collect();
    // Order by series game number, then start time (doubleheaders share a number
    // only across series, but keep a stable tiebreak by date/pk).
    raw.sort_by(|x, y| {
        x.series_game_number
            .cmp(&y.series_game_number)
            .then_with(|| x.game_date.cmp(&y.game_date))
            .then_with(|| x.game_pk.cmp(&y.game_pk))
    });

    let mut games_out: Vec<RawSeriesGame> = Vec::with_capacity(raw.len());
    // Series record from the headline-A side's perspective, counting only final
    // games (ties don't move the count).
    let (mut wins_a, mut wins_b) = (0i32, 0i32);
    for g in &raw {
        // Align the API's home/away sides to the headline orientation by team id,
        // so `team_a`/`score_a` always refer to the headline's left team.
        let away_is_a = g.teams.away.team.id == team_a_id;
        let (sa, sb) = if away_is_a {
            (g.teams.away.score, g.teams.home.score)
        } else {
            (g.teams.home.score, g.teams.away.score)
        };
        let status = status_of(&g.status);
        let mut winner = String::new();
        if status == MatchStatus::Finished {
            if let (Some(a), Some(b)) = (sa, sb) {
                if a > b {
                    wins_a += 1;
                    winner = "a".to_string();
                } else if b > a {
                    wins_b += 1;
                    winner = "b".to_string();
                }
            }
        }
        let venue_tz = Some(g.venue.time_zone.id.clone()).filter(|s| !s.is_empty());
        let begin_at = DateTime::parse_from_rfc3339(&g.game_date)
            .ok()
            .map(|d| d.with_timezone(&Utc));
        games_out.push(RawSeriesGame {
            match_id: g.game_pk,
            begin_at,
            venue_tz,
            team_a: team_a_label.to_string(),
            team_b: team_b_label.to_string(),
            score_a: sa,
            score_b: sb,
            winner,
            status,
            current: g.game_pk == game_pk,
        });
    }

    // "Game 2 of 3" — the current game's place. Prefer the anchor's own series
    // number; fall back to its index among the ordered games.
    let game_label = raw
        .iter()
        .find(|g| g.game_pk == game_pk)
        .map(|g| (g.series_game_number, g.games_in_series))
        .filter(|&(n, total)| n > 0 && total > 0)
        .map(|(n, total)| format!("Game {n} of {total}"))
        .unwrap_or_default();

    RawSeries {
        games: games_out,
        game_label,
        record_label: series_record_label(wins_a, wins_b, team_a_label, team_b_label, anchor_len),
    }
}

/// Turn a [`RawSeries`] into the display [`crate::types::Series`], formatting each
/// game's date/time labels with `fmt_labels` (the caller owns the display-tz /
/// 12h-24h preference). Pure — no I/O — so the raw series can be cached once and
/// re-formatted per request.
pub fn format_series(
    raw: &RawSeries,
    fmt_labels: impl Fn(DateTime<Utc>, Option<&str>) -> SeriesLabels,
) -> crate::types::Series {
    use crate::types::SeriesGame;
    let games = raw
        .games
        .iter()
        .map(|g| {
            let labels = g
                .begin_at
                .map(|utc| fmt_labels(utc, g.venue_tz.as_deref()))
                .unwrap_or_default();
            SeriesGame {
                day_label: labels.day,
                clock_label: labels.clock,
                venue_label: labels.venue,
                team_a: g.team_a.clone(),
                team_b: g.team_b.clone(),
                score_a: g.score_a,
                score_b: g.score_b,
                winner: g.winner.clone(),
                status: g.status,
                current: g.current,
                match_id: g.match_id,
            }
        })
        .collect();
    crate::types::Series {
        games,
        game_label: raw.game_label.clone(),
        record_label: raw.record_label.clone(),
    }
}

/// The spoiler-bearing series standing from the headline-A side's win counts:
/// "Astros win series 2–1" once one side clinches, else "Blue Jays lead 2–1",
/// "Series tied 1–1", or "" when no game is final yet. A side has clinched once
/// its wins exceed half the series length.
fn series_record_label(
    wins_a: i32,
    wins_b: i32,
    team_a: &str,
    team_b: &str,
    games_in_series: i32,
) -> String {
    if wins_a == 0 && wins_b == 0 {
        return String::new();
    }
    let (hi, lo) = (wins_a.max(wins_b), wins_a.min(wins_b));
    let leader = if wins_a >= wins_b { team_a } else { team_b };
    // Majority of the series clinches it (e.g. 2 of 3, 3 of 5, but never on a
    // 2-game set where 2–0 still "wins").
    let clinched = games_in_series > 0 && hi * 2 > games_in_series;
    if wins_a == wins_b {
        format!("Series tied {hi}\u{2013}{lo}")
    } else if clinched {
        format!("{leader} win series {hi}\u{2013}{lo}")
    } else {
        format!("{leader} lead {hi}\u{2013}{lo}")
    }
}

/// Fetch the whole series between the two teams for the headline game and return
/// the tz-agnostic [`RawSeries`]. `begin_at` anchors a tight date window (the
/// series can't span more than `games_in_series` days plus a doubleheader, so pad
/// generously). `team_a`/`team_b` are the headline's away/home labels; `series`
/// carries the team ids + this game's series position. Call [`format_series`] on
/// the result to produce per-request display labels.
pub async fn fetch_series(
    client: &reqwest::Client,
    game_pk: i64,
    begin_at: DateTime<Utc>,
    team_a_label: &str,
    team_b_label: &str,
    series: crate::types::MlbSeriesRef,
) -> Result<RawSeries, reqwest::Error> {
    let date = begin_at.date_naive();
    // The series spans from (this game - (n-1)) to (this game + (total-n)) days.
    // Pad ±1 day for late-night/cross-midnight starts; build_raw_series then
    // filters to the contiguous series of the right length.
    let before = i64::from((series.game_number - 1).max(0)) + 1;
    let after = i64::from((series.games_in_series - series.game_number).max(0)) + 1;
    let start = date - chrono::Duration::days(before);
    let end = date + chrono::Duration::days(after);
    let url = format!(
        "{BASE}/schedule?sportId=1&teamId={}&opponentId={}&startDate={}&endDate={}&hydrate=team,venue(timezone)",
        series.team_id_a,
        series.team_id_b,
        start.format("%Y-%m-%d"),
        end.format("%Y-%m-%d"),
    );
    let resp: ScheduleResp = client
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let games: Vec<RawGame> = resp.dates.into_iter().flat_map(|d| d.games).collect();
    Ok(build_raw_series(
        games,
        game_pk,
        series.team_id_a,
        team_a_label,
        team_b_label,
    ))
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
                    gb: if t.games_back == "-" {
                        "—".to_string()
                    } else {
                        t.games_back
                    },
                })
                .collect();
            rows.sort_by_key(|r| r.rank);
            Some((
                division_order(rec.division.id),
                EventInfo {
                    event: "MLB".to_string(),
                    tournament_id: rec.division.id,
                    stage: name.to_string(),
                    sport: Sport::Mlb,
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
    let resp: StandingsResp = client
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(divisions_from(resp))
}

// ---- Linescore raw structs ------------------------------------------------

#[derive(Deserialize, Default)]
pub struct RawLinescore {
    #[serde(default)]
    innings: Vec<RawInning>,
    #[serde(default)]
    teams: RawLsTeams,
}

#[derive(Deserialize, Default)]
struct RawLsTeams {
    #[serde(default)]
    home: RawLsSide,
    #[serde(default)]
    away: RawLsSide,
}

#[derive(Deserialize, Default)]
struct RawLsSide {
    #[serde(default)]
    runs: Option<i64>,
    #[serde(default)]
    hits: i64,
    #[serde(default)]
    errors: i64,
}

#[derive(Deserialize, Default)]
struct RawInning {
    #[serde(default)]
    home: RawLsSide,
    #[serde(default)]
    away: RawLsSide,
}

// ---- Boxscore raw structs -------------------------------------------------

#[derive(Deserialize, Default)]
pub struct RawBoxscore {
    #[serde(default)]
    teams: RawBsTeams,
    #[serde(default, rename = "topPerformers")]
    top_performers: Vec<RawPerformer>,
}

#[derive(Deserialize, Default)]
struct RawBsTeams {
    #[serde(default)]
    home: RawBsTeam,
    #[serde(default)]
    away: RawBsTeam,
}

#[derive(Deserialize, Default)]
struct RawBsTeam {
    #[serde(default)]
    team: RawBsTeamName,
    #[serde(default, rename = "teamStats")]
    team_stats: RawTeamStats,
    #[serde(default)]
    players: std::collections::HashMap<String, RawPlayer>,
    #[serde(default)]
    batters: Vec<i64>,
    #[serde(default)]
    pitchers: Vec<i64>,
}

#[derive(Deserialize, Default)]
struct RawBsTeamName {
    #[serde(default)]
    name: String,
    #[serde(default, rename = "abbreviation")]
    abbrev: String,
}

#[derive(Deserialize, Default)]
struct RawTeamStats {
    #[serde(default)]
    batting: RawBatTotals,
    #[serde(default)]
    pitching: RawPitchTotals,
}

#[derive(Deserialize, Default)]
struct RawBatTotals {
    #[serde(default)]
    hits: i64,
    #[serde(default, rename = "homeRuns")]
    home_runs: i64,
    #[serde(default, rename = "leftOnBase")]
    lob: i64,
    #[serde(default, rename = "strikeOuts")]
    strike_outs: i64,
    #[serde(default, rename = "baseOnBalls")]
    walks: i64,
    #[serde(default)]
    avg: String,
}

#[derive(Deserialize, Default)]
struct RawPitchTotals {
    #[serde(default, rename = "strikeOuts")]
    strike_outs: i64,
    #[serde(default, rename = "baseOnBalls")]
    walks: i64,
    #[serde(default)]
    era: String,
}

#[derive(Deserialize, Default)]
struct RawPlayer {
    #[serde(default)]
    person: RawPerson,
    #[serde(default)]
    position: RawPos,
    #[serde(default)]
    stats: RawSplit,
    #[serde(default, rename = "seasonStats")]
    season_stats: RawSplit,
}

#[derive(Deserialize, Default)]
struct RawPerson {
    #[serde(default, rename = "fullName")]
    full_name: String,
}

#[derive(Deserialize, Default)]
struct RawPos {
    #[serde(default)]
    abbreviation: String,
}

#[derive(Deserialize, Default)]
struct RawSplit {
    #[serde(default)]
    batting: RawBatLine,
    #[serde(default)]
    pitching: RawPitchLine,
}

#[derive(Deserialize, Default)]
struct RawBatLine {
    #[serde(default, rename = "atBats")]
    at_bats: Option<i64>,
    #[serde(default)]
    runs: Option<i64>,
    #[serde(default)]
    hits: Option<i64>,
    #[serde(default)]
    rbi: Option<i64>,
    #[serde(default, rename = "baseOnBalls")]
    walks: Option<i64>,
    #[serde(default, rename = "strikeOuts")]
    strike_outs: Option<i64>,
    #[serde(default)]
    avg: Option<String>,
}

#[derive(Deserialize, Default)]
struct RawPitchLine {
    #[serde(default, rename = "inningsPitched")]
    innings: Option<String>,
    #[serde(default)]
    hits: Option<i64>,
    #[serde(default)]
    runs: Option<i64>,
    #[serde(default, rename = "earnedRuns")]
    earned_runs: Option<i64>,
    #[serde(default, rename = "baseOnBalls")]
    walks: Option<i64>,
    #[serde(default, rename = "strikeOuts")]
    strike_outs: Option<i64>,
    #[serde(default)]
    era: Option<String>,
}

#[derive(Deserialize, Default)]
struct RawPerformer {
    #[serde(default)]
    player: RawPlayer,
}

// ---- Fetch ----------------------------------------------------------------

/// Fetch a game's linescore + boxscore (two keyless calls).
pub async fn fetch_box_score(
    client: &reqwest::Client,
    game_pk: i64,
) -> Result<(RawLinescore, RawBoxscore), reqwest::Error> {
    let ls = client
        .get(format!("{BASE}/game/{game_pk}/linescore"))
        .send()
        .await?
        .json::<RawLinescore>()
        .await?;
    let bs = client
        .get(format!("{BASE}/game/{game_pk}/boxscore"))
        .send()
        .await?
        .json::<RawBoxscore>()
        .await?;
    Ok((ls, bs))
}

// ---- Normalizer helpers ---------------------------------------------------

fn i(n: Option<i64>) -> String {
    n.map(|v| v.to_string()).unwrap_or_default()
}

fn batting_table(t: &RawBsTeam) -> PlayerTable {
    let rows = t
        .batters
        .iter()
        .filter_map(|id| t.players.get(&format!("ID{id}")))
        .map(|p| PlayerRow {
            name: p.person.full_name.clone(),
            note: p.position.abbreviation.clone(),
            values: vec![
                i(p.stats.batting.at_bats),
                i(p.stats.batting.runs),
                i(p.stats.batting.hits),
                i(p.stats.batting.rbi),
                i(p.stats.batting.walks),
                i(p.stats.batting.strike_outs),
                p.season_stats.batting.avg.clone().unwrap_or_default(),
            ],
        })
        .collect();
    PlayerTable {
        title: format!("Batting \u{2014} {}", t.team.abbrev),
        team: t.team.abbrev.clone(),
        columns: ["AB", "R", "H", "RBI", "BB", "K", "AVG"]
            .map(String::from)
            .to_vec(),
        rows,
    }
}

fn pitching_table(t: &RawBsTeam) -> PlayerTable {
    let rows = t
        .pitchers
        .iter()
        .filter_map(|id| t.players.get(&format!("ID{id}")))
        .map(|p| PlayerRow {
            name: p.person.full_name.clone(),
            note: p.position.abbreviation.clone(),
            values: vec![
                p.stats.pitching.innings.clone().unwrap_or_default(),
                i(p.stats.pitching.hits),
                i(p.stats.pitching.runs),
                i(p.stats.pitching.earned_runs),
                i(p.stats.pitching.walks),
                i(p.stats.pitching.strike_outs),
                p.season_stats.pitching.era.clone().unwrap_or_default(),
            ],
        })
        .collect();
    PlayerTable {
        title: format!("Pitching \u{2014} {}", t.team.abbrev),
        team: t.team.abbrev.clone(),
        columns: ["IP", "H", "R", "ER", "BB", "K", "ERA"]
            .map(String::from)
            .to_vec(),
        rows,
    }
}

/// Normalize a game's linescore + boxscore into the shared `BoxScore`.
pub fn to_box_score(ls: &RawLinescore, bs: &RawBoxscore) -> BoxScore {
    let (away, home) = (&bs.teams.away, &bs.teams.home);

    // Line score: runs per inning + R/H/E totals.
    let segments: Vec<String> = (1..=ls.innings.len()).map(|n| n.to_string()).collect();
    let seg_vals = |pick: fn(&RawInning) -> &RawLsSide| -> Vec<String> {
        ls.innings.iter().map(|inn| i(pick(inn).runs)).collect()
    };
    let totals = vec![
        StatPair {
            label: "R".into(),
            away: ls.teams.away.runs.unwrap_or(0).to_string(),
            home: ls.teams.home.runs.unwrap_or(0).to_string(),
            away_share: None,
        },
        StatPair {
            label: "H".into(),
            away: ls.teams.away.hits.to_string(),
            home: ls.teams.home.hits.to_string(),
            away_share: None,
        },
        StatPair {
            label: "E".into(),
            away: ls.teams.away.errors.to_string(),
            home: ls.teams.home.errors.to_string(),
            away_share: None,
        },
    ];
    let line = LineScore {
        segments,
        away: LineRow {
            team: away.team.name.clone(),
            abbrev: away.team.abbrev.clone(),
            segment_values: seg_vals(|inn| &inn.away),
            total: ls.teams.away.runs.unwrap_or(0).to_string(),
        },
        home: LineRow {
            team: home.team.name.clone(),
            abbrev: home.team.abbrev.clone(),
            segment_values: seg_vals(|inn| &inn.home),
            total: ls.teams.home.runs.unwrap_or(0).to_string(),
        },
        totals,
    };

    // Team-stat comparison (lead bars on the counting stats).
    let cmp = |label: &str, a: String, h: String, bar: bool| StatPair {
        away_share: if bar { stat_share(&a, &h) } else { None },
        label: label.into(),
        away: a,
        home: h,
    };
    let (ab, hb) = (&away.team_stats.batting, &home.team_stats.batting);
    let (ap, hp) = (&away.team_stats.pitching, &home.team_stats.pitching);
    let team_stats = vec![
        cmp("Hits", ab.hits.to_string(), hb.hits.to_string(), true),
        cmp(
            "Home Runs",
            ab.home_runs.to_string(),
            hb.home_runs.to_string(),
            true,
        ),
        cmp("Left on Base", ab.lob.to_string(), hb.lob.to_string(), true),
        cmp("Walks", ab.walks.to_string(), hb.walks.to_string(), true),
        cmp(
            "Strikeouts (batting)",
            ab.strike_outs.to_string(),
            hb.strike_outs.to_string(),
            true,
        ),
        cmp("Team AVG", ab.avg.clone(), hb.avg.clone(), false),
        cmp(
            "Pitching K",
            ap.strike_outs.to_string(),
            hp.strike_outs.to_string(),
            true,
        ),
        cmp("Team ERA", ap.era.clone(), hp.era.clone(), false),
    ];

    // Leaders from the API's curated topPerformers.
    let leaders: Vec<LeaderCard> = bs
        .top_performers
        .iter()
        .map(|p| {
            let b = &p.player.stats.batting;
            let line = if b.hits.is_some() {
                format!(
                    "{}-{}, {} R, {} RBI",
                    i(b.hits),
                    i(b.at_bats),
                    i(b.runs),
                    i(b.rbi)
                )
            } else {
                let pt = &p.player.stats.pitching;
                format!(
                    "{} IP, {} K, {} ER",
                    pt.innings.clone().unwrap_or_default(),
                    i(pt.strike_outs),
                    i(pt.earned_runs)
                )
            };
            LeaderCard {
                name: p.player.person.full_name.clone(),
                team: String::new(),
                category: "Top performer".into(),
                line,
            }
        })
        .collect();

    let player_tables = vec![
        batting_table(away),
        pitching_table(away),
        batting_table(home),
        pitching_table(home),
    ];

    BoxScore {
        line: Some(line),
        team_stats,
        leaders,
        player_tables,
        timeline: Vec::new(),
        games: Vec::new(),
        unavailable: false,
    }
}

// ---- Tests ----------------------------------------------------------------

#[cfg(all(test, feature = "ssr"))]
mod boxscore_tests {
    use super::*;

    #[test]
    fn mlb_to_box_score_maps_linescore_stats_and_players() {
        let ls: RawLinescore =
            serde_json::from_str(include_str!("testdata/mlb_linescore_824819.json")).unwrap();
        let bs: RawBoxscore =
            serde_json::from_str(include_str!("testdata/mlb_boxscore_824819.json")).unwrap();
        let out = to_box_score(&ls, &bs);

        let line = out.line.expect("line score");
        assert_eq!(line.segments.len(), 9, "nine innings");
        // R/H/E totals present as three total columns.
        let labels: Vec<&str> = line.totals.iter().map(|t| t.label.as_str()).collect();
        assert_eq!(labels, ["R", "H", "E"]);

        // Team-stat comparison has Hits with a computed lead share.
        let hits = out
            .team_stats
            .iter()
            .find(|s| s.label == "Hits")
            .expect("Hits row");
        assert!(hits.away_share.is_some());

        // Two batting + two pitching tables (one per team).
        assert_eq!(out.player_tables.len(), 4);
        assert!(out
            .player_tables
            .iter()
            .any(|t| t.title.starts_with("Batting")));
        assert!(out
            .player_tables
            .iter()
            .any(|t| t.title.starts_with("Pitching")));
        assert!(!out.unavailable);
    }
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
        let games: Vec<NormalizedMatch> = resp
            .dates
            .into_iter()
            .flat_map(|d| d.games)
            .filter_map(to_match)
            .collect();
        assert_eq!(games.len(), 2);
        assert_eq!(games[0].sport, Sport::Mlb);
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
        // TV group (national first, then away), then MLB.tv, then radio.
        assert_eq!(out.len(), 4);
        assert_eq!(
            (
                out[0].name.as_str(),
                out[0].tag.as_str(),
                out[0].group.as_str()
            ),
            ("FOX", "national · TV", "tv")
        );
        // A national broadcaster gets a watch link; a local network stays text.
        assert_eq!(out[0].url, "https://www.foxsports.com/live");
        assert_eq!(
            (
                out[1].name.as_str(),
                out[1].tag.as_str(),
                out[1].group.as_str()
            ),
            ("Rangers Sports Network", "away · TV", "tv")
        );
        assert!(out[1].url.is_empty());
        // MLB.tv is inserted between TV and radio, with a link.
        assert_eq!(
            (out[2].name.as_str(), out[2].group.as_str()),
            ("MLB.tv", "streaming")
        );
        assert_eq!(out[2].url, "https://www.mlb.com/tv");
        assert_eq!(
            (
                out[3].name.as_str(),
                out[3].tag.as_str(),
                out[3].group.as_str()
            ),
            ("560AM WQAM", "home · radio", "radio")
        );
        assert!(out[3].url.is_empty());
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

    /// A 3-game Astros(117, away) @ Blue Jays(141, home) series: G1/G2 final,
    /// G3 upcoming — the headline game is G2. Mirrors the live API shape.
    fn sample_series_json() -> &'static str {
        r#"{"dates":[
          {"games":[{"gamePk":822800,"gameDate":"2026-06-22T20:07:00Z","officialDate":"2026-06-22",
            "seriesGameNumber":1,"gamesInSeries":3,"seriesDescription":"Regular Season",
            "venue":{"timeZone":{"id":"America/Toronto"}},
            "status":{"abstractGameState":"Final","detailedState":"Final"},
            "teams":{"away":{"score":2,"team":{"id":117,"teamName":"Astros"}},
                     "home":{"score":4,"team":{"id":141,"teamName":"Blue Jays"}}}}]},
          {"games":[{"gamePk":822799,"gameDate":"2026-06-23T20:07:00Z","officialDate":"2026-06-23",
            "seriesGameNumber":2,"gamesInSeries":3,"seriesDescription":"Regular Season",
            "status":{"abstractGameState":"Final","detailedState":"Final"},
            "teams":{"away":{"score":9,"team":{"id":117,"teamName":"Astros"}},
                     "home":{"score":7,"team":{"id":141,"teamName":"Blue Jays"}}}}]},
          {"games":[{"gamePk":822798,"gameDate":"2026-06-24T20:07:00Z","officialDate":"2026-06-24",
            "seriesGameNumber":3,"gamesInSeries":3,"seriesDescription":"Regular Season",
            "status":{"abstractGameState":"Preview","detailedState":"Scheduled"},
            "teams":{"away":{"team":{"id":117,"teamName":"Astros"}},
                     "home":{"team":{"id":141,"teamName":"Blue Jays"}}}}]}
        ]}"#
    }

    fn raw_series_games(json: &str) -> Vec<RawGame> {
        let resp: ScheduleResp = serde_json::from_str(json).unwrap();
        resp.dates.into_iter().flat_map(|d| d.games).collect()
    }

    /// Test label formatter: UTC date + "HH:MM" (no tz dependency), echoing any
    /// venue tz id into the venue label so we can assert it's threaded through.
    fn utc_labels(d: DateTime<Utc>, venue_tz: Option<&str>) -> SeriesLabels {
        SeriesLabels {
            day: d.format("%a, %b %-d").to_string(),
            clock: d.format("%H:%M").to_string(),
            venue: venue_tz.map(|tz| format!("@{tz}")).unwrap_or_default(),
        }
    }

    #[test]
    fn series_orients_to_headline_orders_and_flags_current() {
        // Headline is G2 (gamePk 822799); team_a = away Astros(117), team_b = home Jays(141).
        let raw = build_raw_series(
            raw_series_games(sample_series_json()),
            822799,
            117,
            "Astros",
            "Blue Jays",
        );
        let series = format_series(&raw, utc_labels);
        assert_eq!(series.games.len(), 3);
        // Ordered by series game number; each row oriented to the headline (a=Astros).
        assert_eq!(series.games[0].day_label, "Mon, Jun 22");
        // The start date+time are formatted via the supplied formatter (UTC here).
        assert_eq!(series.games[0].clock_label, "20:07");
        // The per-game venue tz is threaded through to the formatter (echoed here).
        assert_eq!(series.games[0].venue_label, "@America/Toronto");
        // A game without a venue tz gets no venue label.
        assert_eq!(series.games[1].venue_label, "");
        assert_eq!(
            (series.games[0].score_a, series.games[0].score_b),
            (Some(2), Some(4))
        );
        assert_eq!(series.games[0].winner, "b"); // Jays won G1
        assert!(!series.games[0].current);
        // G2 is the current game; Astros won it.
        assert!(series.games[1].current);
        assert_eq!(
            (series.games[1].score_a, series.games[1].score_b),
            (Some(9), Some(7))
        );
        assert_eq!(series.games[1].winner, "a");
        // G3 upcoming — no scores, no winner.
        assert_eq!(series.games[2].status, MatchStatus::Upcoming);
        assert_eq!(series.games[2].score_a, None);
        assert_eq!(series.games[2].winner, "");
        // 1 win each so far → tied, and "Game 2 of 3".
        assert_eq!(series.game_label, "Game 2 of 3");
        assert_eq!(series.record_label, "Series tied 1\u{2013}1");
    }

    #[test]
    fn series_orients_when_headline_team_a_is_home() {
        // Flip the headline orientation: team_a = home Jays(141), team_b = away Astros(117).
        let raw = build_raw_series(
            raw_series_games(sample_series_json()),
            822799,
            141,
            "Blue Jays",
            "Astros",
        );
        let series = format_series(&raw, utc_labels);
        // G1: Jays beat Astros 4-2 → a=4, b=2, winner "a".
        assert_eq!(
            (series.games[0].score_a, series.games[0].score_b),
            (Some(4), Some(2))
        );
        assert_eq!(series.games[0].winner, "a");
        // G2 (current): Astros beat Jays 9-7 → a=7, b=9, winner "b".
        assert_eq!(
            (series.games[1].score_a, series.games[1].score_b),
            (Some(7), Some(9))
        );
        assert_eq!(series.games[1].winner, "b");
    }

    #[test]
    fn series_filters_out_an_adjacent_series_in_a_padded_window() {
        // Padded window pulls in a stray game from a *different* (2-game) series
        // between the same teams; it must be dropped (wrong gamesInSeries).
        let mut games = raw_series_games(sample_series_json());
        games.push(RawGame {
            game_pk: 999999,
            game_date: "2026-06-25T20:07:00Z".into(),
            status: RawStatus {
                abstract_state: "Final".into(),
                detailed_state: "Final".into(),
            },
            teams: RawTeams {
                away: RawSide {
                    score: Some(1),
                    team: RawTeamRef {
                        id: 117,
                        team_name: "Astros".into(),
                        ..Default::default()
                    },
                },
                home: RawSide {
                    score: Some(0),
                    team: RawTeamRef {
                        id: 141,
                        team_name: "Blue Jays".into(),
                        ..Default::default()
                    },
                },
            },
            series: "Regular Season".into(),
            series_game_number: 1,
            games_in_series: 2,
            broadcasts: Vec::new(),
            venue: RawVenue::default(),
        });
        let raw = build_raw_series(games, 822799, 117, "Astros", "Blue Jays");
        let series = format_series(&raw, utc_labels);
        // Only the three 3-game-series games survive.
        assert_eq!(series.games.len(), 3);
        assert!(series.games.iter().all(|g| g.match_id != 999999));
    }

    #[test]
    fn record_label_wording() {
        // No final games yet → empty.
        assert_eq!(series_record_label(0, 0, "A", "B", 3), "");
        // Tie.
        assert_eq!(
            series_record_label(1, 1, "A", "B", 3),
            "Series tied 1\u{2013}1"
        );
        // Lead without clinching (1-0 of a 3-game set).
        assert_eq!(series_record_label(1, 0, "A", "B", 3), "A lead 1\u{2013}0");
        // Clinched a 3-game set (2-1).
        assert_eq!(
            series_record_label(2, 1, "A", "B", 3),
            "A win series 2\u{2013}1"
        );
        assert_eq!(
            series_record_label(1, 2, "A", "B", 3),
            "B win series 2\u{2013}1"
        );
        // A 2-game set: at 1-0 there's still a game to play (lead); at 2-0 it's
        // a completed sweep (won).
        assert_eq!(series_record_label(1, 0, "A", "B", 2), "A lead 1\u{2013}0");
        assert_eq!(
            series_record_label(2, 0, "A", "B", 2),
            "A win series 2\u{2013}0"
        );
    }

    #[test]
    fn format_series_applies_labels_and_keeps_raw_fields() {
        let raw = RawSeries {
            game_label: "Game 1 of 3".to_string(),
            record_label: String::new(),
            games: vec![RawSeriesGame {
                match_id: 42,
                begin_at: "2026-06-23T23:10:00Z".parse::<DateTime<Utc>>().ok(),
                venue_tz: Some("America/New_York".to_string()),
                team_a: "NYY".to_string(),
                team_b: "BOS".to_string(),
                score_a: Some(5),
                score_b: Some(3),
                winner: "a".to_string(),
                status: MatchStatus::Finished,
                current: true,
            }],
        };
        // A trivial formatter: prove the closure receives the UTC start + venue tz,
        // and its output lands in the SeriesGame labels.
        let series = format_series(&raw, |utc, tz| SeriesLabels {
            day: utc.format("%Y-%m-%d").to_string(),
            clock: utc.format("%H:%M").to_string(),
            venue: tz.unwrap_or("").to_string(),
        });
        assert_eq!(series.game_label, "Game 1 of 3");
        let g = &series.games[0];
        assert_eq!(g.match_id, 42);
        assert_eq!(g.day_label, "2026-06-23");
        assert_eq!(g.clock_label, "23:10");
        assert_eq!(g.venue_label, "America/New_York");
        assert_eq!(
            (g.score_a, g.score_b, g.winner.as_str(), g.current),
            (Some(5), Some(3), "a", true)
        );
    }
}
