//! Teamfight Tactics schedule from Liquipedia's MediaWiki API (server-only).
//!
//! No commercial esports data API covers TFT, and its LPDB (structured) API is
//! access-gated, so this parses the *rendered HTML* of Liquipedia's free
//! `action=parse` endpoint. TFT is lobby-based, so a "match" here is one
//! broadcast session (a tournament day/stage) rendered as a single-entity
//! [`NormalizedMatch`] — the session label is the one competitor label, and
//! there is no opponent. Opt-in behind `Config::liquipedia_enabled`; polled on a
//! proximity cadence under a per-day request cap (Liquipedia's terms require a
//! low rate + aggressive caching + CC-BY-SA attribution).

use crate::feed::{NormalizedMatch, NormalizedTeam};
use crate::types::{MatchStatus, Sport};
use chrono::{DateTime, Duration, Utc};
use tl::{HTMLTag, Parser, ParserOptions};

/// TFT ids sit in their own high range so they never collide with another
/// source's ids (identity is `(sport, id)`, but a distinct base keeps them
/// obviously TFT in the DB).
const TFT_ID_BASE: i64 = 40_000_000_000;

/// How long after its start a session still reads as "live" when the feed gives
/// no end time. TFT tournament days run long; a few hours is a safe window for a
/// schedule site (we favor a correct schedule over live-score precision).
const LIVE_WINDOW: Duration = Duration::hours(3);

/// One parsed broadcast session from the Liquipedia upcoming-matches page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSession {
    /// Tournament edition, e.g. "Tactician's Crown: Space Gods".
    pub tournament: String,
    /// The session/day label, e.g. "Day 1 - Swiss", "Grand Finals".
    pub session_label: String,
    pub begin_at: DateTime<Utc>,
    /// Absolute Liquipedia URL of the tournament page (source-link attribution).
    pub tournament_url: String,
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
/// Assumes session labels are unique within a tournament (Liquipedia labels each
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
        status_at(s.begin_at, now),
        entity,
        empty,
    );
    m.series_name = s.tournament.clone();
    // The tournament's Liquipedia page — the "view this" source link and the
    // CC-BY-SA attribution surface for this row.
    if !s.tournament_url.is_empty() {
        m.league_url = Some(s.tournament_url.clone());
    }
    m
}

// ----- Parsing the rendered "Upcoming and ongoing matches" page -------------
//
// Each match is a `<table class="wikitable wikitable-striped infobox_matches_content">`
// (the underscores are `&#95;`-encoded in the source, so we key on the clean
// `wikitable-striped` token instead). Within a table:
//   - the session label is `<td class="versus">Day 2 - Game 1</td>`
//   - the start time is `<span class="timer-object" data-timestamp="<unix>">`
//   - the tournament is the first non-`Special:` `/tft/...` anchor; its `title`
//     is the readable edition ("Space Gods Tactician's Crown - Swiss Stage").
// `tl` does not decode HTML entities, so extracted text is run through
// `decode_entities` + whitespace normalization.

const WIKI_HOST: &str = "https://liquipedia.net";

/// Parse the rendered upcoming-matches HTML into sessions. Skips any table
/// missing a timestamp, label, or tournament link rather than emitting a
/// half-populated row.
pub fn parse_upcoming(html: &str) -> Vec<ParsedSession> {
    let Ok(dom) = tl::parse(html, ParserOptions::default()) else {
        return Vec::new();
    };
    let parser = dom.parser();
    let Some(tables) = dom.query_selector("table.wikitable-striped") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for handle in tables {
        let Some(tag) = handle.get(parser).and_then(|n| n.as_tag()) else {
            continue;
        };
        let Some(begin_at) = table_timestamp(tag, parser) else {
            continue;
        };
        let Some(session_label) = table_label(tag, parser) else {
            continue;
        };
        let Some((tournament, tournament_url)) = table_tournament(tag, parser) else {
            continue;
        };
        out.push(ParsedSession {
            tournament,
            session_label,
            begin_at,
            tournament_url,
        });
    }
    out
}

/// An attribute's value as an owned string (`tl` stores raw, entity-encoded).
fn attr_str(tag: &HTMLTag, key: &str) -> Option<String> {
    tag.attributes()
        .get(key)
        .flatten()
        .map(|b| b.as_utf8_str().to_string())
}

/// The `data-timestamp` (unix seconds) of the table's countdown span.
fn table_timestamp(tag: &HTMLTag, parser: &Parser) -> Option<DateTime<Utc>> {
    let node = tag
        .query_selector(parser, "span.timer-object")?
        .next()?
        .get(parser)?
        .as_tag()?;
    let ts: i64 = attr_str(node, "data-timestamp")?.trim().parse().ok()?;
    DateTime::from_timestamp(ts, 0)
}

/// The session label from the table's `td.versus` (e.g. "Day 2 - Game 1").
fn table_label(tag: &HTMLTag, parser: &Parser) -> Option<String> {
    let node = tag
        .query_selector(parser, "td.versus")?
        .next()?
        .get(parser)?;
    let text = normalize_ws(&decode_entities(&node.inner_text(parser)));
    (!text.is_empty()).then_some(text)
}

/// The first non-`Special:` `/tft/` anchor's readable title + absolute URL.
fn table_tournament(tag: &HTMLTag, parser: &Parser) -> Option<(String, String)> {
    for h in tag.query_selector(parser, "a")? {
        let Some(a) = h.get(parser).and_then(|n| n.as_tag()) else {
            continue;
        };
        let Some(href) = attr_str(a, "href") else {
            continue;
        };
        if !href.starts_with("/tft/") || href.starts_with("/tft/Special:") {
            continue;
        }
        // Prefer the `title` (full edition); fall back to the anchor text.
        let title = attr_str(a, "title")
            .map(|t| normalize_ws(&decode_entities(&t)))
            .filter(|t| !t.is_empty());
        let name = title.unwrap_or_else(|| normalize_ws(&decode_entities(&a.inner_text(parser))));
        if !name.is_empty() {
            return Some((name, format!("{WIKI_HOST}{href}")));
        }
    }
    None
}

/// Collapse all runs of whitespace (incl. the `&#160;` no-break spaces MediaWiki
/// litters through these labels) to single ASCII spaces.
fn normalize_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Decode the handful of HTML entities MediaWiki emits here: the named ones plus
/// numeric decimal/hex (`&#160;`, `&#x27;`). `tl` leaves these raw.
fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < s.len() {
        if bytes[i] == b'&'
            && let Some(semi) = s[i..].find(';')
            && let Some(c) = decode_one(&s[i + 1..i + semi])
        {
            out.push(c);
            i += semi + 1;
            continue;
        }
        let ch = s[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Decode one entity body (the text between `&` and `;`), or `None` if unknown.
fn decode_one(ent: &str) -> Option<char> {
    match ent {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some('\u{a0}'),
        _ => {
            let num = ent.strip_prefix('#')?;
            let code = match num.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => num.parse::<u32>().ok()?,
            };
            char::from_u32(code)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        s.parse::<DateTime<Utc>>().unwrap()
    }

    #[test]
    fn session_id_is_stable_and_reschedule_invariant() {
        let a = session_id("Tactician's Crown: Space Gods", "Day 1 - Swiss");
        let b = session_id("Tactician's Crown: Space Gods", "Day 1 - Swiss");
        assert_eq!(a, b, "same key must hash identically across fetches");
        // A different session in the same tournament gets a different id.
        assert_ne!(
            a,
            session_id("Tactician's Crown: Space Gods", "Grand Finals")
        );
        // Ids are non-negative (they seed a DB primary key alongside sport).
        assert!(a >= 0);
    }

    #[test]
    fn session_maps_to_single_entity_upcoming_match() {
        let now = at("2026-07-10T00:00:00Z");
        let s = ParsedSession {
            tournament: "Tactician's Crown: Space Gods".into(),
            session_label: "Day 1 - Swiss".into(),
            begin_at: at("2026-07-10T12:30:00Z"),
            tournament_url: "https://liquipedia.net/teamfighttactics/Tactician's_Crown".into(),
        };
        let m = session_to_match(&s, now);
        assert_eq!(m.sport, Sport::Tft);
        assert_eq!(m.league, "TFT");
        assert_eq!(m.series_name, "Tactician's Crown: Space Gods");
        assert_eq!(m.team_a.label, "Day 1 - Swiss");
        assert!(m.team_b.label.is_empty(), "single-entity: no opponent");
        assert_eq!(m.status, MatchStatus::Upcoming);
        // Carries the Liquipedia page as its source link (CC-BY-SA attribution).
        assert_eq!(m.league_url.as_deref(), Some(s.tournament_url.as_str()));
    }

    #[test]
    fn status_tracks_the_clock_with_a_live_window() {
        let s = |begin: &str| ParsedSession {
            tournament: "T".into(),
            session_label: "S".into(),
            begin_at: at(begin),
            tournament_url: String::new(),
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
    fn decode_entities_handles_named_and_numeric() {
        assert_eq!(
            decode_entities("Tactician&#39;s Crown"),
            "Tactician's Crown"
        );
        assert_eq!(decode_entities("A&#160;-&#160;B"), "A\u{a0}-\u{a0}B");
        assert_eq!(decode_entities("infobox&#95;matches"), "infobox_matches");
        assert_eq!(decode_entities("R&amp;D &lt;x&gt;"), "R&D <x>");
        // A bare & or unknown entity is passed through untouched.
        assert_eq!(decode_entities("cats & dogs"), "cats & dogs");
    }

    #[test]
    fn parses_sessions_from_the_fixture() {
        let html = include_str!("fixtures/tft_upcoming.html");
        let sessions = parse_upcoming(html);
        // The committed fixture holds six match tables.
        assert_eq!(sessions.len(), 6, "fixture has six match tables");
        // Every parsed row has a real tournament, label, and timestamp.
        for s in &sessions {
            assert!(!s.tournament.is_empty(), "tournament name required");
            assert!(!s.session_label.is_empty(), "session label required");
            assert!(
                s.begin_at.timestamp() > 1_600_000_000,
                "plausible unix time"
            );
            assert!(
                s.tournament_url.starts_with("https://liquipedia.net/tft/"),
                "absolute Liquipedia URL"
            );
        }
        // Pin the first session to the captured fixture.
        let first = &sessions[0];
        assert_eq!(first.session_label, "Day 2 - Game 1");
        assert_eq!(
            first.tournament,
            "Space Gods Tactician's Crown - Swiss Stage"
        );
        assert_eq!(
            first.tournament_url,
            "https://liquipedia.net/tft/Space_Gods/Tacticians_Crown/Swiss_Stage"
        );
        assert_eq!(first.begin_at.timestamp(), 1_783_767_600);
    }
}
