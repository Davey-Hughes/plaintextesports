//! Wire/view types shared between the server and the browser (WASM).
//!
//! These are plain `serde` structs with no server-only dependencies (no
//! `chrono`, no `reqwest`) so they compile cheaply into the WASM bundle. All
//! display formatting (times, day labels) is done server-side, so the client
//! only ever renders ready-to-display strings.

use serde::{Deserialize, Serialize};

/// Default forward window (days from today) for a traditional-sports schedule.
/// They play daily, so the homepage shows a short window by default and a
/// "show later days" control extends it up to [`TRAD_FORWARD_MAX`].
pub const TRAD_FORWARD_DAYS: i64 = 2;
/// Never show traditional-sports games more than this many days ahead.
pub const TRAD_FORWARD_MAX: i64 = 7;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Sport {
    #[default]
    Cs2,
    Lol,
    /// Major League Baseball — a traditional sport, fetched from the MLB Stats
    /// API rather than PandaScore. Lives behind the esports/traditional toggle.
    Mlb,
    /// National Hockey League — a traditional sport, fetched from the keyless
    /// NHL Web API (`api-web.nhle.com`). Like MLB: two teams per sport.
    Nhl,
    /// National Basketball Association — a traditional sport, fetched from the
    /// keyless ESPN API. Two teams per sport.
    Nba,
    /// National Football League — a traditional sport, fetched from the keyless
    /// ESPN API. Two teams per sport.
    Nfl,
    /// Soccer — a traditional sport, fetched from the keyless ESPN API. Covers
    /// more than one competition (the Premier League and the World Cup), each its
    /// own `league`. Two teams per sport; matches can draw.
    Soccer,
    /// Motorsport — one category covering several series via the `league` field:
    /// F1 (Jolpica/Ergast), WRC + WEC (Orange Cat Blacktop). A "match" is a single
    /// session/round (an F1 session, a WEC session, or a WRC rally); the event is
    /// the Grand Prix / race meeting / rally. Single-entity (no opponent). The
    /// series are the sub-league chips, like soccer's competitions.
    Motorsport,
}

impl Sport {
    /// Every sport, in canonical (display) order. The single source of truth for
    /// "which sport slugs are valid" — drives the DB cleanup migration (purging
    /// rows written under a since-renamed slug) and exhaustiveness tests.
    pub const ALL: &'static [Sport] = &[
        Self::Cs2,
        Self::Lol,
        Self::Mlb,
        Self::Nhl,
        Self::Nba,
        Self::Nfl,
        Self::Soccer,
        Self::Motorsport,
    ];

    /// Display name. Traditional sports use the sport (not the league), so the
    /// top-level filter reads "Baseball/Hockey/…" rather than "MLB/NHL/…"; the
    /// league name itself comes from the match data. American football is
    /// "Football"; the other code uses "Soccer" for the international sport.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Cs2 => "CS2",
            Self::Lol => "LoL",
            Self::Mlb => "Baseball",
            Self::Nhl => "Hockey",
            Self::Nba => "Basketball",
            Self::Nfl => "Football",
            Self::Soccer => "Soccer",
            // The category is motorsport; the series (F1, …) is its sub-filter.
            Self::Motorsport => "Motorsport",
        }
    }

    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Cs2 => "cs2",
            Self::Lol => "lol",
            Self::Mlb => "mlb",
            Self::Nhl => "nhl",
            Self::Nba => "nba",
            Self::Nfl => "nfl",
            Self::Soccer => "football",
            Self::Motorsport => "motorsport",
        }
    }

    /// A traditional sport (vs an esports title) — drives the top-bar mode toggle.
    #[must_use]
    pub const fn traditional(self) -> bool {
        matches!(
            self,
            Self::Mlb | Self::Nhl | Self::Nba | Self::Nfl | Self::Soccer | Self::Motorsport
        )
    }

    /// A single-entity sport (a race/session, not two opposing teams) — the row
    /// shows one competitor label rather than "A vs B".
    #[must_use]
    pub const fn single_entity(self) -> bool {
        matches!(self, Self::Motorsport)
    }

    /// Whether this sport's leagues are a genuine sub-categorisation worth a
    /// filter chip even when only one is present — soccer's competitions (Premier
    /// League / World Cup) and motorsport's series (F1, …). For the other sports
    /// the sole league just repeats the tab, so no chip.
    #[must_use]
    pub const fn has_sub_leagues(self) -> bool {
        matches!(self, Self::Soccer | Self::Motorsport)
    }

    /// The traditional sports, in display order — drives the in-mode sport tabs.
    #[must_use]
    pub fn traditional_sports() -> &'static [Sport] {
        &[Self::Mlb, Self::Nhl, Self::Nba, Self::Nfl, Self::Soccer, Self::Motorsport]
    }

    /// The label for the standings table's last column: a map/game record for
    /// esports, or the team sports' single ranking figure (games-back / points /
    /// win%). Empty for F1 (it has no standings table).
    #[must_use]
    pub const fn standings_last_label(self) -> &'static str {
        match self {
            Self::Lol => "Games",
            Self::Cs2 => "Maps",
            Self::Mlb | Self::Nba => "GB",
            Self::Nhl | Self::Soccer => "PTS",
            Self::Nfl => "PCT",
            Self::Motorsport => "",
        }
    }

    /// Whether the standings' last column holds a single ranking value (games-back
    /// / points / win%) rather than a map/game win-loss record. True for the team
    /// sports, false for esports.
    #[must_use]
    pub const fn standings_single_value(self) -> bool {
        matches!(
            self,
            Self::Mlb | Self::Nhl | Self::Nba | Self::Nfl | Self::Soccer
        )
    }

    /// Parse a UI filter slug. Unknown values (incl. "all") map to `None`.
    #[must_use]
    pub fn from_filter(s: &str) -> Option<Self> {
        match s {
            "cs2" => Some(Self::Cs2),
            "lol" => Some(Self::Lol),
            "mlb" => Some(Self::Mlb),
            "nhl" => Some(Self::Nhl),
            "nba" => Some(Self::Nba),
            "nfl" => Some(Self::Nfl),
            "football" => Some(Self::Soccer),
            // "f1" kept for back-compat with saved filters/links from when the
            // motorsport sport's slug was "f1" (now "motorsport").
            "motorsport" | "f1" => Some(Self::Motorsport),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatchStatus {
    /// Default — also the `from_db` fallback for an unknown value.
    #[default]
    Upcoming,
    Live,
    Finished,
    Canceled,
}

impl MatchStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Upcoming => "upcoming",
            Self::Live => "live",
            Self::Finished => "finished",
            Self::Canceled => "canceled",
        }
    }

    /// Short status badge shown on a schedule/series row ("" while upcoming, so
    /// no badge is drawn).
    #[must_use]
    pub const fn badge(self) -> &'static str {
        match self {
            Self::Live => "LIVE",
            Self::Finished => "Final",
            Self::Canceled => "Canc.",
            Self::Upcoming => "",
        }
    }

    /// CSS class for a schedule/series row in this status (styles the badge + row
    /// emphasis). Matches the `MatchStatus` variants by intent.
    #[must_use]
    pub const fn row_class(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Finished => "final",
            Self::Canceled => "canceled",
            Self::Upcoming => "upcoming",
        }
    }

    /// Parse the string written by [`as_str`](Self::as_str) back (used when
    /// loading from the cache DB). Unknown values default to `Upcoming`. Named
    /// `from_db` rather than `from_str` to avoid shadowing the `FromStr` trait.
    #[must_use]
    pub fn from_db(s: &str) -> Self {
        match s {
            "live" => Self::Live,
            "finished" => Self::Finished,
            "canceled" => Self::Canceled,
            _ => Self::Upcoming,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamView {
    /// Short label shown in the box (acronym if available, else name).
    pub label: String,
    /// Full team name — keys the team page + per-team subscription (empty/"TBD"
    /// when the opponent isn't decided). `#[serde(default)]` so older cached
    /// payloads without it still deserialize.
    #[serde(default)]
    pub name: String,
    pub score: Option<i64>,
    /// True when this team won a finished match (for emphasis).
    pub winner: bool,
    /// Vector (SVG) logo URL, when a vector source exists (NHL/MLB); empty
    /// otherwise. Shown on the match/team pages. `#[serde(default)]` for older
    /// cached payloads.
    #[serde(default)]
    pub logo: String,
    /// Official 2-3 letter abbreviation, shown in place of the label in compact
    /// contexts (narrow schedule rows). Empty when the source has none.
    #[serde(default)]
    pub abbrev: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchView {
    pub id: i64,
    pub sport: Sport,
    /// Short league name, e.g. "LCK" or "IEM".
    pub league: String,
    /// The series/edition name within the league (e.g. "Cologne Major"), empty
    /// when the league name is already the full event name. Combined with
    /// `league` to title the event/match pages (e.g. "IEM Cologne Major").
    #[serde(default)]
    pub series_name: String,
    pub tier: String,
    pub status: MatchStatus,
    /// Start time in the display tz, e.g. "6:00 PM" (always the clock time; the
    /// live/final state is conveyed separately by `status`).
    pub clock_label: String,
    /// Start date in the display tz, e.g. "Wednesday, June 24" — shown on the
    /// detail page, where there's no day heading to carry it. `#[serde(default)]`
    /// so older cached payloads still load.
    #[serde(default)]
    pub date_label: String,
    /// The same start time at the event's venue (e.g. "6:00 PM EDT"), shown when
    /// the user clicks the time. Empty when the venue timezone is unknown
    /// (esports). `#[serde(default)]` so older cached payloads still load.
    #[serde(default)]
    pub venue_label: String,
    /// The venue's name (e.g. "loanDepot park") and its city/country (e.g.
    /// "Miami, FL"), shown on the match page. Empty for esports (no venue).
    #[serde(default)]
    pub venue_name: String,
    #[serde(default)]
    pub venue_location: String,
    /// e.g. "Bo3"; empty when unknown.
    pub best_of: String,
    pub team_a: TeamView,
    pub team_b: TeamView,
    pub stream_url: Option<String>,
    /// Link to the event page: official site when known, else a Liquipedia search.
    pub event_url: String,
    pub begin_at_ms: i64,
    /// When set, the whole row links here instead of its own `/match` page. The
    /// WRC schedule's collapsed "Day N" rows use it to jump to that day on the
    /// rally's event page, where the day's individual stages are listed.
    /// `#[serde(default)]` so older cached payloads still load.
    #[serde(default)]
    pub row_href: Option<String>,
}

impl MatchView {
    /// This match's globally-unique id (see [`match_uid`]).
    #[must_use]
    pub fn uid(&self) -> String {
        match_uid(self.sport, self.id)
    }
}

/// A globally-unique match identifier, `"{slug}-{id}"` (e.g. `"cs2-1494586"`,
/// `"mlb-822869"`). Source match ids aren't unique across games/sources (an ESPN
/// event id can equal an MLB gamePk), so identity is keyed by `(sport, id)`. Sport
/// slugs contain no `-` and ids are numeric, so the uid splits cleanly. Used in
/// the `/match/{uid}` route, the client reveal/star sets + DOM ids, the
/// notifications round-trip, and reminder keys.
#[must_use]
pub fn match_uid(sport: Sport, id: i64) -> String {
    format!("{}-{}", sport.slug(), id)
}

/// Parse a [`match_uid`] back into its sport + source id; `None` if malformed or
/// the slug is unknown.
#[must_use]
pub fn parse_match_uid(uid: &str) -> Option<(Sport, i64)> {
    let (slug, id) = uid.split_once('-')?;
    Some((Sport::from_filter(slug)?, id.parse().ok()?))
}

/// The display name for an event: the league plus its series/edition, e.g.
/// ("IEM", "Cologne Major") -> "IEM Cologne Major". An empty series returns the
/// league alone (league seasons already name themselves). Also the key for the
/// `/event` page, so each edition is addressed distinctly.
#[must_use]
pub fn full_event_name(league: &str, series: &str) -> String {
    if series.is_empty() {
        league.to_string()
    } else {
        format!("{league} {series}")
    }
}

/// Whether `full_event_name(league, series) == target`, without allocating — used
/// to filter the snapshot by edition on every event request.
#[must_use]
pub fn event_name_eq(league: &str, series: &str, target: &str) -> bool {
    if series.is_empty() {
        target == league
    } else {
        target
            .strip_prefix(league)
            .and_then(|rest| rest.strip_prefix(' '))
            .is_some_and(|rest| rest == series)
    }
}

/// The best-fit noun for a tier-2 competition, for display (the subscription tag).
/// The `league` field is a mix of true leagues (MLB, LCK, "… Pro League"),
/// knockout tournaments (World Cup, IEM, the invitationals/majors), and racing
/// championships (F1, WRC, WEC) — "league" alone mislabels the latter two. We only
/// have the name here (not the `Sport`), so this is name-based: exact for the
/// fixed sports/motorsport names, keyword heuristics for the open-ended esports
/// ones (where an occasional miss is acceptable). Returns "league" by default.
#[must_use]
pub fn competition_kind(league: &str) -> &'static str {
    // Racing championships go by "series" (and "WEC"/"WRC" must match before the
    // "championship" keyword below, which their full names would otherwise hit).
    if matches!(league, "F1" | "WRC" | "WEC") {
        return "series";
    }
    // Knockout/standalone competitions: cups, invitationals, majors, masters,
    // championships, and Intel Extreme Masters ("IEM"). A domestic "… League"
    // (Premier League, XSE Pro League) has none of these, so it stays a league.
    const TOURNAMENT_WORDS: &[&str] =
        &["cup", "invitational", "major", "masters", "championship", "iem"];
    let lower = league.to_ascii_lowercase();
    if TOURNAMENT_WORDS.iter().any(|w| lower.contains(w)) {
        return "tournament";
    }
    "league"
}

/// Matches for one league/event within a day.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeagueGroup {
    pub league: String,
    /// The series/edition name within the league (e.g. "Cologne Major"); empty
    /// for league seasons. Combined with `league` for the header + event link.
    #[serde(default)]
    pub series_name: String,
    /// Event-page link shown on the league header (see `MatchView::event_url`).
    pub event_url: String,
    /// Best-of label shown in the header when uniform across the group (e.g.
    /// "Bo3"); `None` when matches mix formats (then shown per-row).
    pub bo: Option<String>,
    pub matches: Vec<MatchView>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DayGroup {
    /// ISO date in the display timezone, e.g. "2026-06-21".
    pub day_key: String,
    /// Human label, e.g. "Sunday, June 21".
    pub day_label: String,
    pub leagues: Vec<LeagueGroup>,
}

/// A footer link (e.g. GitHub, social media).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteLink {
    pub label: String,
    pub url: String,
}

/// Footer/site info pulled from config.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteInfo {
    pub copyright: Option<String>,
    /// When set, the copyright line links to this URL.
    #[serde(default)]
    pub copyright_url: Option<String>,
    pub links: Vec<SiteLink>,
}

/// A browser Web Push subscription (from `PushSubscription.toJSON()`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushSub {
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
}

/// A request to be reminded about a match (sent from the browser when starred).
/// Only the push subscription + match id are sent; the server derives the
/// notification's time/title/body/url from its snapshot (it doesn't trust the
/// client for those).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReminderReq {
    pub sub: PushSub,
    pub match_id: i64,
    /// The match's sport slug, so the server resolves the right match (ids aren't
    /// unique across games) and keys the reminder by `(match_id, game)`.
    #[serde(default)]
    pub sport: String,
}

/// Subscribe/unsubscribe to a whole sport or event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscribeReq {
    pub sub: PushSub,
    /// "sport" or "league".
    pub kind: String,
    /// "cs2"/"lol" for a sport, else the league name.
    pub value: String,
}

/// One broadcast/stream for a match. Esports entries (from PandaScore
/// `streams_list`) carry a clickable `url`; MLB TV/radio broadcasts carry no
/// link, just a network `name` and a pre-formatted `tag` (e.g. "national · TV").
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamView {
    pub url: String,
    /// ISO-639-1 language code (e.g. "en"), or empty if unknown.
    pub language: String,
    pub official: bool,
    pub main: bool,
    /// Display label for a link-less broadcast (the network name). Empty for
    /// esports streams, whose label is derived from the `url`.
    #[serde(default)]
    pub name: String,
    /// Pre-formatted tag for a link-less broadcast (e.g. "national · TV"). Empty
    /// for esports streams, whose tags are derived from language/main/official.
    #[serde(default)]
    pub tag: String,
    /// Grouping key for the broadcast list, so kinds are visually separated:
    /// "official"/"costream" for esports, "tv"/"streaming"/"radio" for MLB. The
    /// producers emit entries already ordered by group.
    #[serde(default)]
    pub group: String,
}

/// One row of a group-stage / Swiss standings table.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StandingRow {
    pub rank: i32,
    pub team: String,
    pub wins: i32,
    pub losses: i32,
    pub ties: i32,
    /// Maps/games won and lost (the "map diff"); zeroed when unavailable.
    pub game_wins: i32,
    pub game_losses: i32,
    /// Games back from the division leader (MLB), e.g. "3.5" or "-" for the
    /// leader. Empty for esports standings, which don't use it.
    #[serde(default)]
    pub gb: String,
}

/// One match within a bracket round.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BracketMatch {
    pub team_a: String,
    pub team_b: String,
    pub score_a: Option<i64>,
    pub score_b: Option<i64>,
    /// Which side won, if decided: "a", "b", or empty.
    pub winner: String,
    /// PandaScore match id, for linking to the match detail page. 0 when unknown
    /// (e.g. demo data); the bracket suppresses the link in that case.
    #[serde(default)]
    pub match_id: i64,
    /// The `(round_index, match_index)` of the matches whose winners/losers feed
    /// this one (0 entries = a first-round match). Used to auto-reveal a series'
    /// lineup once its feeders' scores are revealed — works for single- and
    /// double-elimination alike. Each feeder is in a strictly earlier round.
    #[serde(default)]
    pub feeders: Vec<(usize, usize)>,
}

/// One match within a Swiss-stage bracket (a "first to N" group stage rendered
/// as a buchholz grid rather than a tree).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwissMatch {
    pub team_a: String,
    pub team_b: String,
    pub score_a: Option<i64>,
    pub score_b: Option<i64>,
    /// Which side won, if decided: "a", "b", or empty.
    pub winner: String,
    pub match_id: i64,
    /// The final stage record (e.g. "3-0" advanced, "0-3" eliminated) for a side
    /// that finished the Swiss in this match; empty while the team is still in.
    #[serde(default)]
    pub a_record: String,
    #[serde(default)]
    pub b_record: String,
    /// The `match_id` of each side's previous-round match (the "feeder" that put
    /// it at this record), or `None` in round 1. Progressive reveal gates a
    /// side's name on its feeder's score being shown — exactly like the playoff
    /// bracket — so you can't peek ahead without revealing the earlier rounds.
    #[serde(default)]
    pub a_feeder: Option<i64>,
    #[serde(default)]
    pub b_feeder: Option<i64>,
}

/// A record bucket within a Swiss round — the teams that entered with the same
/// W-L (e.g. all the 2-0 teams), playing each other.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwissBucket {
    /// The shared entering record, e.g. "2-0".
    pub record: String,
    pub matches: Vec<SwissMatch>,
}

/// One round (column) of a Swiss stage, its buckets ordered most-wins first.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwissRound {
    pub number: u32,
    pub buckets: Vec<SwissBucket>,
}

/// A column of the elimination bracket (e.g. "Quarterfinals").
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BracketRound {
    pub title: String,
    pub matches: Vec<BracketMatch>,
    /// Which half of a double-elimination bracket this column belongs to:
    /// "upper", "lower", "final" (grand final), or "" for a single-elimination
    /// bracket. Drives the upper/lower visual grouping.
    #[serde(default)]
    pub section: String,
}

/// Standings + bracket for one event/tournament.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventInfo {
    pub event: String,
    /// PandaScore tournament id — the stable key for sharing per-round reveal
    /// state across the pages that show this same bracket.
    pub tournament_id: i64,
    /// The stage/tournament name within the event (e.g. "Stage 1", "Playoffs"),
    /// so a multi-stage event can label each stage. Empty for single-stage events.
    #[serde(default)]
    pub stage: String,
    /// Which sport this event is, so standings can use sport-appropriate labels.
    pub sport: Sport,
    pub standings: Vec<StandingRow>,
    pub rounds: Vec<BracketRound>,
    /// Swiss-stage rounds (buchholz grid), when this stage is a Swiss/group
    /// stage; empty otherwise. Paired with `standings` (the same stage), so the
    /// UI can toggle between the grid and the ranking table.
    #[serde(default)]
    pub swiss: Vec<SwissRound>,
}

impl EventInfo {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.standings.is_empty() && self.rounds.is_empty() && self.swiss.is_empty()
    }
}

/// MLB-only handle for fetching the whole series between the two teams: the two
/// teams' MLB-Stats-API ids and this game's 1-based position within its series.
/// Kept on the in-memory match (not serialized to the client, not persisted);
/// the detail server fn uses it to fetch the series at request time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MlbSeriesRef {
    /// MLB Stats API team ids for the home/away sides (order doesn't matter — the
    /// fetch uses one as `teamId` and the other as `opponentId`).
    pub team_id_a: i64,
    pub team_id_b: i64,
    /// This game's 1-based number within its series, and the series length, so the
    /// fetch can bound a tight date window around just this one series.
    pub game_number: i32,
    pub games_in_series: i32,
}

/// One game of an MLB series between the two teams. Scores are raw (the same as
/// the headline match score); the UI hides them and the leader until the page's
/// reveal toggle is on, so nothing leaks past the spoiler gate.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeriesGame {
    /// Date label in the viewer's display tz, e.g. "Mon, Jun 23". Derived from the
    /// same tz as `clock_label`, so the date and time always agree (a late game can
    /// fall on a different calendar day in the viewer's tz than at the ballpark).
    pub day_label: String,
    /// Start time in the display tz, e.g. "6:40 PM" (formatted server-side, like
    /// the schedule rows). Empty when the start time can't be parsed.
    #[serde(default)]
    pub clock_label: String,
    /// The same start as `day_label`/`clock_label` but in the ballpark's local tz,
    /// as a full "date · time tz" (e.g. "Mon, Jun 23 · 7:10 PM EDT") — shown when
    /// the viewer clicks the time, like the schedule's venue toggle. Empty when the
    /// venue tz is unknown or matches the display tz (nothing to add).
    #[serde(default)]
    pub venue_label: String,
    /// The two sides' short labels, in the same order as the headline match
    /// (`team_a` is the headline's left team), so each row reads consistently.
    pub team_a: String,
    pub team_b: String,
    pub score_a: Option<i64>,
    pub score_b: Option<i64>,
    /// Which headline side won this game once final: "a", "b", or "" (not final
    /// or tied). Only surfaced by the UI when scores are revealed.
    #[serde(default)]
    pub winner: String,
    pub status: MatchStatus,
    /// True for the game the detail page is about (the "current" game).
    pub current: bool,
    /// gamePk, so each row links to that game's own detail page (0 = this game).
    #[serde(default)]
    pub match_id: i64,
}

/// The multi-game series between the two teams of an MLB match: every game and
/// the overall series standing. Empty `games` ⇒ no series section.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Series {
    pub games: Vec<SeriesGame>,
    /// e.g. "Game 2 of 3" — the current game's place in the series (always safe
    /// to show; reveals nothing about scores).
    #[serde(default)]
    pub game_label: String,
    /// Spoiler-bearing record line computed from the revealed games, e.g.
    /// "Blue Jays lead 2–1", "Series tied 1–1", or "Astros win series 2–1". The
    /// UI shows it only when scores are revealed. Empty when no game is final yet.
    #[serde(default)]
    pub record_label: String,
}

/// One finished F1 session's full classification, for the GP event page.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct F1Result {
    /// The session this classifies: "Race", "Sprint", or "Qualifying".
    pub session: String,
    pub rows: Vec<F1ResultRow>,
}

/// One driver's line in an F1 session result.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct F1ResultRow {
    /// Finishing/grid position, e.g. "1". May be non-numeric for the classified.
    pub pos: String,
    pub driver: String,
    pub constructor: String,
    /// Race/Sprint: finishing time or gap ("+2.974"), else the status ("+1 Lap",
    /// "DNF"). Qualifying: the best lap time (pole time for P1). A spoiler.
    pub detail: String,
    /// The driver's nationality flag (SVG) + their constructor's logo URLs; empty
    /// when unmapped. `#[serde(default)]` for older cached payloads.
    #[serde(default)]
    pub flag: String,
    #[serde(default)]
    pub constructor_logo: String,
    /// The constructor's 2-3 letter abbreviation (e.g. "FER"), shown in place of
    /// the full name where the column is too narrow. Empty when unmapped.
    #[serde(default)]
    pub constructor_abbrev: String,
}

/// The F1 championship standings as of a round, for the GP event page. Both
/// tables are spoilers — they encode every prior result.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct F1Standings {
    /// The round these standings are current as of (e.g. 7).
    pub round: i64,
    pub drivers: Vec<F1StandingRow>,
    pub constructors: Vec<F1StandingRow>,
}

/// One line in an F1 championship table. `detail` carries the driver's
/// constructor in the drivers' table; it's empty in the constructors' table.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct F1StandingRow {
    pub pos: String,
    pub name: String,
    pub detail: String,
    pub points: String,
    pub wins: String,
    /// The driver's nationality flag (drivers' table only) + the constructor's
    /// logo (the driver's team, or the constructor itself). Empty when unmapped.
    #[serde(default)]
    pub flag: String,
    #[serde(default)]
    pub constructor_logo: String,
    /// The constructor's 2-3 letter abbreviation, shown where the name won't fit.
    #[serde(default)]
    pub constructor_abbrev: String,
}

/// Championship standings for WRC/WEC (Orange Cat Blacktop) — a list of titled
/// tables, since each series has several (WRC: drivers / co-drivers /
/// manufacturers; WEC: per class × drivers / teams / manufacturers). Generic
/// enough for both, unlike the F1-specific [`F1Standings`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MotorStandings {
    pub tables: Vec<MotorStandingTable>,
}

/// One championship table within a series' standings.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MotorStandingTable {
    /// The class these tables belong to (WEC: "Hypercar"/"LMGT3"); empty for WRC.
    /// Tables sharing a `group` render together; a new group starts a new row.
    #[serde(default)]
    pub group: String,
    /// e.g. "Drivers", "Teams", "Manufacturers".
    pub title: String,
    pub rows: Vec<MotorStandingRow>,
}

/// One line in a WRC/WEC championship table.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MotorStandingRow {
    pub pos: String,
    pub name: String,
    pub points: String,
    /// Nationality flag (from the API's 2-letter country code); empty when none.
    #[serde(default)]
    pub flag: String,
}

/// Everything the per-match detail page shows.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchDetail {
    pub found: bool,
    pub match_view: Option<MatchView>,
    pub streams: Vec<StreamView>,
    pub event: EventInfo,
    /// Extra standings tables shown below `event` — used by MLB to show each
    /// team's division table. Empty for esports (which use `event`).
    #[serde(default)]
    pub stages: Vec<EventInfo>,
    /// The MLB series between the two teams (every game + the series standing).
    /// Empty for esports and for MLB games with no resolvable series.
    /// `#[serde(default)]` so older cached payloads still load.
    #[serde(default)]
    pub series: Series,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleView {
    pub days: Vec<DayGroup>,
    /// Today's date key (`%Y-%m-%d`) in the display tz, computed server-side so a
    /// day heading can be greyed when past or highlighted when it's today (the
    /// keys sort chronologically as strings). Empty in tests.
    #[serde(default)]
    pub today_key: String,
    /// When the data was last fetched (unix ms).
    pub fetched_at_ms: i64,
    /// Server-formatted "last loaded" time in the display tz, e.g. "10:57 AM".
    pub fetched_label: String,
    /// True when the most recent poll failed and we're serving older data.
    pub stale: bool,
    /// True when no API token is configured and demo data is shown.
    pub using_fixture: bool,
    /// True when demo data was explicitly forced via `DEMO=1`.
    pub demo_forced: bool,
    /// Populated only for the single-day view: ISO dates for prev/next nav.
    pub date_label: Option<String>,
    pub prev_date: Option<String>,
    pub next_date: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_name_eq_agrees_with_full_event_name() {
        for (league, series) in [("IEM", "Cologne Major"), ("LCK", ""), ("A B", "C")] {
            let full = full_event_name(league, series);
            assert!(event_name_eq(league, series, &full));
        }
        assert!(!event_name_eq("IEM", "Cologne Major", "IEM Katowice"));
        assert!(!event_name_eq("IEM", "Cologne", "IEM Cologne Major")); // series is a prefix
        assert!(!event_name_eq("LCK", "", "LCK Spring"));
        assert!(!event_name_eq("IEM", "Cologne", "IEMCologne")); // missing the space
    }

    #[test]
    fn competition_kind_labels_the_real_competitions() {
        // Racing championships → "series".
        for s in ["F1", "WRC", "WEC"] {
            assert_eq!(competition_kind(s), "series", "{s}");
        }
        // Knockout/standalone competitions → "tournament".
        for t in ["World Cup", "IEM", "Mid-Season Invitational"] {
            assert_eq!(competition_kind(t), "tournament", "{t}");
        }
        // Domestic/regional leagues (incl. a "… Pro League") → "league".
        for l in ["MLB", "LCK", "LCS", "LEC", "LPL", "CBLOL", "XSE Pro League"] {
            assert_eq!(competition_kind(l), "league", "{l}");
        }
    }

    #[test]
    fn game_from_filter_maps_known_and_rejects_other() {
        assert_eq!(Sport::from_filter("cs2"), Some(Sport::Cs2));
        assert_eq!(Sport::from_filter("lol"), Some(Sport::Lol));
        assert_eq!(Sport::from_filter("all"), None);
        assert_eq!(Sport::from_filter(""), None);
    }

    #[test]
    fn game_slug_and_label() {
        assert_eq!(Sport::Cs2.slug(), "cs2");
        assert_eq!(Sport::Lol.slug(), "lol");
        assert_eq!(Sport::Cs2.label(), "CS2");
        assert_eq!(Sport::Lol.label(), "LoL");
        // Traditional sports label by the sport, not the league.
        assert_eq!(Sport::Mlb.label(), "Baseball");
        assert_eq!(Sport::Nhl.label(), "Hockey");
        assert_eq!(Sport::Nba.label(), "Basketball");
        // American football is "Football"; the international sport is "Soccer"
        // (its slug stays "football", unchanged).
        assert_eq!(Sport::Nfl.label(), "Football");
        assert_eq!(Sport::Soccer.label(), "Soccer");
        assert_eq!(Sport::Soccer.slug(), "football");
        assert_eq!(Sport::Motorsport.label(), "Motorsport");
    }

    #[test]
    fn match_status_str_roundtrips() {
        for s in [
            MatchStatus::Upcoming,
            MatchStatus::Live,
            MatchStatus::Finished,
            MatchStatus::Canceled,
        ] {
            assert_eq!(MatchStatus::from_db(s.as_str()), s);
        }
        assert_eq!(MatchStatus::from_db("nonsense"), MatchStatus::Upcoming);
    }

    #[test]
    fn every_game_slug_round_trips_through_from_filter() {
        // `slug` and `from_filter` must be inverse over every sport — store.rs
        // persists `slug()` and reads it back with `from_filter`, so a mismatch
        // would make a row load as a different (default) sport. Also asserts `ALL`
        // is exhaustive (a new variant trips the count).
        assert_eq!(Sport::ALL.len(), 8);
        for &g in Sport::ALL {
            assert_eq!(Sport::from_filter(g.slug()), Some(g), "slug {:?}", g.slug());
        }
    }

    #[test]
    fn soccer_slug_is_football_not_legacy_soccer() {
        // Regression: soccer's slug was renamed "soccer" → "football". The old
        // slug must not parse, so legacy rows are dropped rather than loaded as
        // the default sport (which surfaced the World Cup on the esports page).
        assert_eq!(Sport::Soccer.slug(), "football");
        assert_eq!(Sport::from_filter("football"), Some(Sport::Soccer));
        assert_eq!(Sport::from_filter("soccer"), None);
    }

    #[test]
    fn standings_columns_match_the_sport() {
        // Esports show a map/game record (not a single value); the team sports a
        // single ranking figure with a sport-specific label.
        assert_eq!(Sport::Cs2.standings_last_label(), "Maps");
        assert_eq!(Sport::Lol.standings_last_label(), "Games");
        assert!(!Sport::Cs2.standings_single_value());
        assert!(!Sport::Lol.standings_single_value());
        for (g, label) in [
            (Sport::Mlb, "GB"),
            (Sport::Nhl, "PTS"),
            (Sport::Nba, "GB"),
            (Sport::Nfl, "PCT"),
            (Sport::Soccer, "PTS"),
        ] {
            assert_eq!(g.standings_last_label(), label, "{g:?}");
            assert!(g.standings_single_value(), "{g:?}");
        }
        // F1 has no standings table.
        assert_eq!(Sport::Motorsport.standings_last_label(), "");
        assert!(!Sport::Motorsport.standings_single_value());
    }

    #[test]
    fn match_uid_round_trips_for_every_game() {
        for &g in Sport::ALL {
            let uid = match_uid(g, 1494586);
            assert_eq!(parse_match_uid(&uid), Some((g, 1494586)), "uid {uid}");
            // The slug has no '-', so the split is unambiguous.
            assert_eq!(uid, format!("{}-1494586", g.slug()));
        }
        assert_eq!(parse_match_uid("cs2-1494586"), Some((Sport::Cs2, 1494586)));
        assert_eq!(parse_match_uid("mlb-822869"), Some((Sport::Mlb, 822869)));
        // Malformed / unknown slug → None.
        assert_eq!(parse_match_uid("1494586"), None); // no separator
        assert_eq!(parse_match_uid("bogus-1"), None); // unknown sport
        assert_eq!(parse_match_uid("cs2-abc"), None); // non-numeric id
    }

    #[test]
    fn match_status_badge_and_row_class() {
        assert_eq!(MatchStatus::Live.badge(), "LIVE");
        assert_eq!(MatchStatus::Finished.badge(), "Final");
        assert_eq!(MatchStatus::Canceled.badge(), "Canc.");
        assert_eq!(MatchStatus::Upcoming.badge(), ""); // no badge while upcoming
        // Row classes the SCSS keys off (note "final", not the wire "finished").
        assert_eq!(MatchStatus::Live.row_class(), "live");
        assert_eq!(MatchStatus::Finished.row_class(), "final");
        assert_eq!(MatchStatus::Canceled.row_class(), "canceled");
        assert_eq!(MatchStatus::Upcoming.row_class(), "upcoming");
    }
}
