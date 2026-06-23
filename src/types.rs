//! Wire/view types shared between the server and the browser (WASM).
//!
//! These are plain `serde` structs with no server-only dependencies (no
//! `chrono`, no `reqwest`) so they compile cheaply into the WASM bundle. All
//! display formatting (times, day labels) is done server-side, so the client
//! only ever renders ready-to-display strings.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Game {
    #[default]
    Cs2,
    Lol,
}

impl Game {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Cs2 => "CS2",
            Self::Lol => "LoL",
        }
    }

    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Cs2 => "cs2",
            Self::Lol => "lol",
        }
    }

    /// Parse a UI filter slug. Unknown values (incl. "all") map to `None`.
    #[must_use]
    pub fn from_filter(s: &str) -> Option<Self> {
        match s {
            "cs2" => Some(Self::Cs2),
            "lol" => Some(Self::Lol),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatchStatus {
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

/// One broadcast/stream for a match (from PandaScore `streams_list`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamView {
    pub url: String,
    /// ISO-639-1 language code (e.g. "en"), or empty if unknown.
    pub language: String,
    pub official: bool,
    pub main: bool,
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
}

/// One match within a bracket round.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BracketMatch {
    pub team_a: String,
    pub team_b: String,
    pub score_a: Option<i32>,
    pub score_b: Option<i32>,
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
    pub score_a: Option<i32>,
    pub score_b: Option<i32>,
    /// Which side won, if decided: "a", "b", or empty.
    pub winner: String,
    pub match_id: i64,
    /// Outcome this match triggered for each side: "advanced" (reached the win
    /// target), "eliminated" (reached the loss limit), or "" (still in).
    #[serde(default)]
    pub a_outcome: String,
    #[serde(default)]
    pub b_outcome: String,
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

/// Everything the per-match detail page shows.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchDetail {
    pub found: bool,
    pub match_view: Option<MatchView>,
    pub streams: Vec<StreamView>,
    pub event: EventInfo,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleView {
    pub days: Vec<DayGroup>,
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
