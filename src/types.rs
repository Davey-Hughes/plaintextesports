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
pub enum Game {
    #[default]
    Cs2,
    Lol,
    /// Major League Baseball — a traditional sport, fetched from the MLB Stats
    /// API rather than PandaScore. Lives behind the esports/traditional toggle.
    Mlb,
    /// Formula 1 — a traditional sport, fetched from the Jolpica (Ergast) API.
    /// A "match" is a single session of a Grand Prix weekend (practice/quali/
    /// sprint/race); the Grand Prix is the event. Single-entity (no opponent).
    F1,
}

impl Game {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Cs2 => "CS2",
            Self::Lol => "LoL",
            Self::Mlb => "MLB",
            Self::F1 => "F1",
        }
    }

    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Cs2 => "cs2",
            Self::Lol => "lol",
            Self::Mlb => "mlb",
            Self::F1 => "f1",
        }
    }

    /// A traditional sport (vs an esports title) — drives the top-bar mode toggle.
    #[must_use]
    pub const fn traditional(self) -> bool {
        matches!(self, Self::Mlb | Self::F1)
    }

    /// A single-entity sport (a race/session, not two opposing teams) — the row
    /// shows one competitor label rather than "A vs B".
    #[must_use]
    pub const fn single_entity(self) -> bool {
        matches!(self, Self::F1)
    }

    /// The traditional sports, in display order — drives the in-mode sport tabs.
    #[must_use]
    pub fn traditional_sports() -> &'static [Game] {
        &[Self::Mlb, Self::F1]
    }

    /// Parse a UI filter slug. Unknown values (incl. "all") map to `None`.
    #[must_use]
    pub fn from_filter(s: &str) -> Option<Self> {
        match s {
            "cs2" => Some(Self::Cs2),
            "lol" => Some(Self::Lol),
            "mlb" => Some(Self::Mlb),
            "f1" => Some(Self::F1),
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
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchView {
    pub id: i64,
    pub game: Game,
    /// Short league name, e.g. "LCK" or "IEM".
    pub league: String,
    /// The serie/edition name within the league (e.g. "Cologne Major"), empty
    /// when the league name is already the full event name. Combined with
    /// `league` to title the event/match pages (e.g. "IEM Cologne Major").
    #[serde(default)]
    pub serie_name: String,
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
    /// e.g. "Bo3"; empty when unknown.
    pub best_of: String,
    pub team_a: TeamView,
    pub team_b: TeamView,
    pub stream_url: Option<String>,
    /// Link to the event page: official site when known, else a Liquipedia search.
    pub event_url: String,
    pub begin_at_ms: i64,
}

/// The display name for an event: the league plus its serie/edition, e.g.
/// ("IEM", "Cologne Major") -> "IEM Cologne Major". An empty serie returns the
/// league alone (league seasons already name themselves). Also the key for the
/// `/event` page, so each edition is addressed distinctly.
#[must_use]
pub fn full_event_name(league: &str, serie: &str) -> String {
    if serie.is_empty() {
        league.to_string()
    } else {
        format!("{league} {serie}")
    }
}

/// Whether `full_event_name(league, serie) == target`, without allocating — used
/// to filter the snapshot by edition on every event request.
#[must_use]
pub fn event_name_eq(league: &str, serie: &str, target: &str) -> bool {
    if serie.is_empty() {
        target == league
    } else {
        target
            .strip_prefix(league)
            .and_then(|rest| rest.strip_prefix(' '))
            .is_some_and(|rest| rest == serie)
    }
}

/// Matches for one league/event within a day.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeagueGroup {
    pub league: String,
    /// The serie/edition name within the league (e.g. "Cologne Major"); empty
    /// for league seasons. Combined with `league` for the header + event link.
    #[serde(default)]
    pub serie_name: String,
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
}

/// Subscribe/unsubscribe to a whole game or event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscribeReq {
    pub sub: PushSub,
    /// "game" or "league".
    pub kind: String,
    /// "cs2"/"lol" for a game, else the league name.
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
    /// Which game this event is, so standings can use game-appropriate labels.
    pub game: Game,
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
        for (league, serie) in [("IEM", "Cologne Major"), ("LCK", ""), ("A B", "C")] {
            let full = full_event_name(league, serie);
            assert!(event_name_eq(league, serie, &full));
        }
        assert!(!event_name_eq("IEM", "Cologne Major", "IEM Katowice"));
        assert!(!event_name_eq("IEM", "Cologne", "IEM Cologne Major")); // serie is a prefix
        assert!(!event_name_eq("LCK", "", "LCK Spring"));
        assert!(!event_name_eq("IEM", "Cologne", "IEMCologne")); // missing the space
    }

    #[test]
    fn game_from_filter_maps_known_and_rejects_other() {
        assert_eq!(Game::from_filter("cs2"), Some(Game::Cs2));
        assert_eq!(Game::from_filter("lol"), Some(Game::Lol));
        assert_eq!(Game::from_filter("all"), None);
        assert_eq!(Game::from_filter(""), None);
    }

    #[test]
    fn game_slug_and_label() {
        assert_eq!(Game::Cs2.slug(), "cs2");
        assert_eq!(Game::Lol.slug(), "lol");
        assert_eq!(Game::Cs2.label(), "CS2");
        assert_eq!(Game::Lol.label(), "LoL");
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
}
