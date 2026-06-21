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
    /// Ready-to-display time/status, e.g. "6:00 PM", "LIVE", or "Final".
    pub time_label: String,
    /// e.g. "Bo3"; empty when unknown.
    pub best_of: String,
    pub team_a: TeamView,
    pub team_b: TeamView,
    pub stream_url: Option<String>,
    pub begin_at_ms: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DayGroup {
    /// ISO date in the display timezone, e.g. "2026-06-21".
    pub day_key: String,
    /// Human label, e.g. "Sunday, June 21".
    pub day_label: String,
    pub matches: Vec<MatchView>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleView {
    /// Matches currently live, pinned above the day groups.
    pub live: Vec<MatchView>,
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
