//! Wire/view types shared between the server and the browser (WASM).
//!
//! These are plain `serde` structs with no server-only dependencies (no
//! `chrono`, no `reqwest`) so they compile cheaply into the WASM bundle. All
//! display formatting (times, day labels) is done server-side, so the client
//! only ever renders ready-to-display strings.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Game {
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

    #[must_use]
    pub fn from_str(s: &str) -> Self {
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

/// Matches for one league/event within a day.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeagueGroup {
    pub league: String,
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
    /// Populated only for the single-day view: ISO dates for prev/next nav.
    pub date_label: Option<String>,
    pub prev_date: Option<String>,
    pub next_date: Option<String>,
}
