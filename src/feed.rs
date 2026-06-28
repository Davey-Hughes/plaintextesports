//! The normalized, source-agnostic match model (server-only).
//!
//! Every feed — PandaScore (esports), ESPN, the MLB/NHL stats APIs, F1, and the
//! WRC/MotoGP motorsport sources — deserializes its own wire format and produces
//! a [`NormalizedMatch`]; the store and cache consume only this shape, never a
//! source's raw types. Uses `chrono`, so it lives here rather than in the
//! WASM-safe [`crate::types`] module.

use crate::http::DynError;
use crate::types::{MatchStatus, Sport, StreamView};
use chrono::{DateTime, Utc};

/// Result of fetching one sport's tier-1 matches.
pub type FetchResult = Result<Vec<NormalizedMatch>, DynError>;

/// A match after deserialization, normalization, and tier-1 filtering.
#[derive(Debug, Clone)]
pub struct NormalizedMatch {
    pub id: i64,
    pub sport: Sport,
    pub league: String,
    /// Official league/event site from the API, if any (often absent).
    pub league_url: Option<String>,
    /// The series/edition name within the league (e.g. "Cologne Major"); empty
    /// when absent or the league already names the event. Persisted.
    pub series_name: String,
    pub tier: String,
    pub begin_at: DateTime<Utc>,
    pub status: MatchStatus,
    pub best_of: Option<i64>,
    pub team_a: NormalizedTeam,
    pub team_b: NormalizedTeam,
    pub stream_url: Option<String>,
    /// PandaScore tournament id (used to fetch standings/brackets). Persisted.
    pub tournament_id: Option<i64>,
    /// IANA timezone of the venue, when the source provides it (MLB stadiums).
    /// Lets the UI show the local time at the event; `None` for esports (no
    /// venue tz). Persisted.
    pub venue_tz: Option<String>,
    /// The venue's name (e.g. "loanDepot park", "Albert Park Grand Prix Circuit")
    /// and its city/country (e.g. "Miami, FL", "Melbourne, Australia"), shown on
    /// the match page. Empty for esports (no physical venue). Persisted.
    pub venue_name: String,
    pub venue_location: String,
    /// Each team's vector (SVG) logo URL, when a vector source exists — NHL (the
    /// schedule's `team.logo`) and MLB (mlbstatic, by team id). Empty otherwise
    /// (ESPN/PandaScore expose only raster). Persisted.
    pub team_a_logo: String,
    pub team_b_logo: String,
    /// All broadcasts for the match (from `streams_list`). In-memory only —
    /// repopulated each poll, not persisted, and only used on the detail page.
    pub streams: Vec<StreamView>,
    /// MLB-only: the two teams' MLB-Stats-API ids and this game's series position,
    /// used by the detail page to fetch the whole series between the two teams.
    /// `None` for esports (and unpersisted — repopulated each poll). See
    /// [`crate::types::MlbSeriesRef`].
    pub mlb_series: Option<crate::types::MlbSeriesRef>,
    /// Motorsport-only (WRC/MotoGP): how the detail page fetches this row's
    /// classification (rally overall / stage times / MotoGP session order).
    /// `None` for everything else (and unpersisted — repopulated each poll, like
    /// `mlb_series`). See [`crate::types::MotorResultRef`].
    pub motor_result_ref: Option<crate::types::MotorResultRef>,
}

#[derive(Debug, Clone)]
pub struct NormalizedTeam {
    /// Short display label (acronym/nickname) shown in compact rows.
    pub label: String,
    /// Full team name — keys the team page + per-team subscription, and titles
    /// the team page. Falls back to the label when the source has no distinct
    /// full name; "TBD" when the opponent isn't decided yet.
    pub name: String,
    /// Official 2-3 letter abbreviation (e.g. "LAD", "MTL") shown where the full
    /// name won't fit. Empty when the source has none — esports labels are already
    /// acronyms, so they leave this empty and keep showing the label.
    pub abbrev: String,
    pub score: Option<i64>,
}

impl NormalizedMatch {
    /// A traditional-sport match with the fields common to every keyless feed
    /// defaulted (tier "S", no best-of/stream/tournament/series, no venue tz). The
    /// feeds (MLB/NHL/NBA/NFL/soccer/F1) build on this and set only the sport-
    /// specific extras they have (`series_name`, `venue_tz`, `streams`,
    /// `mlb_series`) afterward. Single-entity sports (F1) pass an empty `team_b`.
    #[must_use]
    pub fn team_sport(
        id: i64,
        sport: Sport,
        league: impl Into<String>,
        begin_at: DateTime<Utc>,
        status: MatchStatus,
        team_a: NormalizedTeam,
        team_b: NormalizedTeam,
    ) -> Self {
        Self {
            id,
            sport,
            league: league.into(),
            league_url: None,
            series_name: String::new(),
            tier: "S".to_string(),
            begin_at,
            status,
            best_of: None,
            team_a,
            team_b,
            stream_url: None,
            tournament_id: None,
            venue_tz: None,
            venue_name: String::new(),
            venue_location: String::new(),
            team_a_logo: String::new(),
            team_b_logo: String::new(),
            streams: Vec::new(),
            mlb_series: None,
            motor_result_ref: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn team_sport_constructor_defaults_the_common_fields() {
        let t = |label: &str| NormalizedTeam {
            label: label.into(),
            name: label.into(),
            score: Some(3),
            abbrev: String::new(),
        };
        let when = "2026-06-24T18:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let m = NormalizedMatch::team_sport(
            7,
            Sport::Nhl,
            "NHL",
            when,
            MatchStatus::Finished,
            t("Bruins"),
            t("Canadiens"),
        );
        assert_eq!(m.id, 7);
        assert_eq!(m.sport, Sport::Nhl);
        assert_eq!(m.league, "NHL");
        assert_eq!(m.tier, "S");
        assert_eq!(m.team_a.label, "Bruins");
        assert_eq!(m.team_b.label, "Canadiens");
        // Everything a keyless team feed doesn't supply is defaulted off.
        assert!(m.league_url.is_none());
        assert!(m.series_name.is_empty());
        assert!(m.best_of.is_none());
        assert!(m.stream_url.is_none());
        assert!(m.tournament_id.is_none());
        assert!(m.venue_tz.is_none());
        assert!(m.streams.is_empty());
        assert!(m.mlb_series.is_none());
    }
}
