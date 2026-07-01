//! NFL + NBA schedule and standings from ESPN's keyless public API
//! (`site.api.espn.com`). Both leagues share one shape — a date-range scoreboard
//! for the schedule and conference-grouped standings — so this one module serves
//! both, parameterized by [`EspnLeague`]. Normalized into the same
//! [`NormalizedMatch`] model as MLB/NHL (two teams, away at home).
//!
//! ESPN doesn't expose a venue IANA timezone, so these games carry no venue-time
//! toggle (like esports); the start time still shows in the viewer's zone.

use crate::feed::{NormalizedMatch, NormalizedTeam};
use crate::types::{EventInfo, MatchStatus, Sport, StandingRow, StreamView};
use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, Utc};
use serde::Deserialize;

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
}

#[derive(Deserialize, Default)]
struct TeamRef {
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
    let has_prime = streams.iter().any(|s| {
        let n = s.name.to_ascii_uppercase();
        n.contains("PRIME") || n.contains("AMAZON")
    });
    let has_twitch = streams
        .iter()
        .any(|s| s.name.eq_ignore_ascii_case("Twitch"));
    if sport == Sport::Nfl && has_prime && !has_twitch {
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
