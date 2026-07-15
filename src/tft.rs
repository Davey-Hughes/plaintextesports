//! The Teamfight Tactics domain model: a broadcast session and its conversion to
//! a [`NormalizedMatch`] (server-only).
//!
//! Source-agnostic on purpose — `competetft` is the only TFT source and produces
//! the [`ParsedSession`] values that land here. TFT is lobby-based, so a "match"
//! is one broadcast session (a tournament day/stage) rendered as a single-entity
//! [`NormalizedMatch`]: the session label is the one competitor label, and there
//! is no opponent.

use crate::feed::{NormalizedMatch, NormalizedTeam};
use crate::types::{MatchStatus, Sport, StreamView};
use chrono::{DateTime, Duration, Utc};

/// TFT ids sit in their own high range so they never collide with another
/// source's ids (identity is `(sport, id)`, but a distinct base keeps them
/// obviously TFT in the DB).
const TFT_ID_BASE: i64 = 40_000_000_000;

/// How long after its start a session still reads as "live" when the feed gives
/// no end time. TFT tournament days run long; a few hours is a safe window for a
/// schedule site (we favor a correct schedule over live-score precision).
const LIVE_WINDOW: Duration = Duration::hours(3);

/// One broadcast session of a TFT tournament.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSession {
    /// Tournament edition, e.g. "Tactician's Crown 2026".
    pub tournament: String,
    /// The session/day label, e.g. "Day 1 - Swiss", "Grand Finals".
    pub session_label: String,
    pub begin_at: DateTime<Utc>,
    /// Absolute URL of the tournament's page (source-link attribution).
    pub tournament_url: String,
    /// Official broadcast streams, ready to enrich with live viewers like a
    /// CS2/LoL match.
    pub streams: Vec<StreamView>,
    /// Explicit match status override. `None` ⇒ derive from the clock via
    /// `status_at`. Set for tournament-level (tier-3) rows, whose `begin_at` is the
    /// range *start* — the clock's fixed live window would wrongly finish a
    /// still-running multi-day event, so the scraped tournament status is used.
    pub status: Option<MatchStatus>,
}

/// A stable 0..1e9 FNV-1a hash of a string.
fn hash(s: &str) -> i64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    (h % 1_000_000_000) as i64
}

/// Deterministic id for a session, keyed on tournament + label (not time). The
/// key excludes `begin_at`, so a rescheduled session upserts into the same row
/// rather than duplicating — the schedule (and any armed reminder) moves with it.
/// Assumes session labels are unique within a tournament (each stage is labelled
/// distinctly, e.g. "Day 1 - Swiss" vs "Grand Finals").
pub fn session_id(tournament: &str, session_label: &str) -> i64 {
    TFT_ID_BASE + hash(&format!("{tournament}\u{1f}{session_label}"))
}

/// Derive display status from the clock: upcoming until start, live for
/// `LIVE_WINDOW` after, finished thereafter.
fn status_at(begin_at: DateTime<Utc>, now: DateTime<Utc>) -> MatchStatus {
    if now < begin_at {
        MatchStatus::Upcoming
    } else if now - begin_at <= LIVE_WINDOW {
        MatchStatus::Live
    } else {
        MatchStatus::Finished
    }
}

/// Map a parsed session to the source-agnostic row. Single-entity: `team_a`
/// carries the session label; `team_b` is empty.
pub fn session_to_match(s: &ParsedSession, now: DateTime<Utc>) -> NormalizedMatch {
    let entity = NormalizedTeam {
        label: s.session_label.clone(),
        name: s.session_label.clone(),
        abbrev: String::new(),
        score: None,
    };
    let empty = NormalizedTeam {
        label: String::new(),
        name: String::new(),
        abbrev: String::new(),
        score: None,
    };
    let mut m = NormalizedMatch::team_sport(
        session_id(&s.tournament, &s.session_label),
        Sport::Tft,
        "TFT",
        s.begin_at,
        s.status.unwrap_or_else(|| status_at(s.begin_at, now)),
        entity,
        empty,
    );
    m.series_name = s.tournament.clone();
    // The tournament's CompeteTFT page — the "view this" source link for this row.
    if !s.tournament_url.is_empty() {
        m.league_url = Some(s.tournament_url.clone());
    }
    m.streams = s.streams.clone();
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        s.parse::<DateTime<Utc>>().unwrap()
    }

    #[test]
    fn session_id_is_stable_and_reschedule_invariant() {
        let a = session_id("Tactician's Crown 2026", "Day 1 - Swiss");
        let b = session_id("Tactician's Crown 2026", "Day 1 - Swiss");
        assert_eq!(a, b, "same key must hash identically across fetches");
        // A different session in the same tournament gets a different id.
        assert_ne!(a, session_id("Tactician's Crown 2026", "Grand Finals"));
        // Ids are non-negative (they seed a DB primary key alongside sport).
        assert!(a >= 0);
    }

    #[test]
    fn session_maps_to_single_entity_upcoming_match() {
        let now = at("2026-07-10T00:00:00Z");
        let s = ParsedSession {
            tournament: "Tactician's Crown 2026".into(),
            session_label: "Day 1 - Swiss".into(),
            begin_at: at("2026-07-10T12:30:00Z"),
            tournament_url: "https://competetft.com/tournament/116323184504995859".into(),
            streams: Vec::new(),
            status: None,
        };
        let m = session_to_match(&s, now);
        assert_eq!(m.sport, Sport::Tft);
        assert_eq!(m.league, "TFT");
        assert_eq!(m.series_name, "Tactician's Crown 2026");
        assert_eq!(m.team_a.label, "Day 1 - Swiss");
        assert!(m.team_b.label.is_empty(), "single-entity: no opponent");
        assert_eq!(m.status, MatchStatus::Upcoming);
        // Carries the tournament page as its source link.
        assert_eq!(m.league_url.as_deref(), Some(s.tournament_url.as_str()));
    }

    #[test]
    fn status_tracks_the_clock_with_a_live_window() {
        let s = |begin: &str| ParsedSession {
            tournament: "T".into(),
            session_label: "S".into(),
            begin_at: at(begin),
            tournament_url: String::new(),
            streams: Vec::new(),
            status: None,
        };
        let now = at("2026-07-10T14:00:00Z");
        // Future -> Upcoming.
        assert_eq!(
            session_to_match(&s("2026-07-10T15:00:00Z"), now).status,
            MatchStatus::Upcoming
        );
        // Started 1h ago, within the 3h live window -> Live.
        assert_eq!(
            session_to_match(&s("2026-07-10T13:00:00Z"), now).status,
            MatchStatus::Live
        );
        // Started 4h ago, past the window -> Finished.
        assert_eq!(
            session_to_match(&s("2026-07-10T10:00:00Z"), now).status,
            MatchStatus::Finished
        );
    }

    #[test]
    fn explicit_status_overrides_the_clock() {
        // Tier-3 rows carry the scraped tournament status: begin_at is the range
        // start, so the clock would wrongly finish a still-running multi-day event.
        let s = ParsedSession {
            tournament: "Tactician's Crown 2026".into(),
            session_label: "Main Event".into(),
            begin_at: at("2026-07-10T00:00:00Z"),
            tournament_url: String::new(),
            streams: Vec::new(),
            status: Some(MatchStatus::Live),
        };
        let long_after = at("2026-07-12T00:00:00Z");
        assert_eq!(session_to_match(&s, long_after).status, MatchStatus::Live);
    }
}
