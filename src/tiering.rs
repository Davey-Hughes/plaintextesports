//! Decides whether a match is "tier 1" and should be shown.
//!
//! Precedence (highest first), matching the project plan:
//!   1. ALLOWLIST  — force-include. Exact full-slug match on the league, series,
//!      or tournament slug. Use this for events PandaScore mis-tiers or tags
//!      late. Exact match (not substring) so it can't accidentally pull in
//!      adjacent noise like "...-lck-challengers-...".
//!   2. DENYLIST   — force-exclude. Substring match on any slug. Strips S/A-
//!      tagged noise (academy/challenger/qualifier/showmatch...).
//!   3. BASE       — keep if the tournament tier is S or A.
//!
//! Allowlist beats denylist beats base. To tune what shows up, edit the
//! `*_ALLOWLIST` / `*_DENYLIST` slices below — they are the single source of
//! truth and are covered by the tests at the bottom of this file.

use crate::types::Sport;

/// Exact league/series/tournament slugs to always include, even if not S/A.
/// Seed values are best-effort; verify against live PandaScore slugs and add
/// here as needed. Leaving this empty is fine — the BASE tier filter already
/// covers the major leagues.
const LOL_ALLOWLIST: &[&str] = &[
    // e.g. "league-of-legends-first-stand" if a new flagship event is mis-tiered
];
const CS_ALLOWLIST: &[&str] = &[
    // e.g. "cs-go-iem-dallas-2026" if a marquee event slips to tier B
];

/// Substrings that mark noise to exclude even when tagged S/A.
const LOL_DENYLIST: &[&str] = &[
    "academy",
    "challenger", // LCK CL / regional challenger leagues
    "amateur",
    "scouting",
    "qualifier",
    "showmatch",
];
const CS_DENYLIST: &[&str] = &[
    "qualifier",
    "open-qualifier",
    "closed-qualifier",
    "relegation",
    "showmatch",
    "amateur",
    "academy",
];

/// Inputs to the tier-1 decision. All slugs are matched case-insensitively.
#[derive(Debug, Clone, Copy, Default)]
pub struct TierInput<'a> {
    pub league_slug: Option<&'a str>,
    pub series_slug: Option<&'a str>,
    pub tournament_slug: Option<&'a str>,
    /// PandaScore tournament tier, e.g. "s", "a", "b". Case-insensitive.
    pub tier: Option<&'a str>,
}

fn allowlist(sport: Sport) -> &'static [&'static str] {
    match sport {
        Sport::Lol => LOL_ALLOWLIST,
        Sport::Cs2 => CS_ALLOWLIST,
        // Traditional sports aren't tier-filtered (every sport/session is shown).
        // TFT comes pre-curated from Liquipedia, so it isn't tier-filtered either.
        Sport::Mlb
        | Sport::Nhl
        | Sport::Nba
        | Sport::Nfl
        | Sport::Soccer
        | Sport::Motorsport
        | Sport::Tft => &[],
    }
}

fn denylist(sport: Sport) -> &'static [&'static str] {
    match sport {
        Sport::Lol => LOL_DENYLIST,
        Sport::Cs2 => CS_DENYLIST,
        Sport::Mlb
        | Sport::Nhl
        | Sport::Nba
        | Sport::Nfl
        | Sport::Soccer
        | Sport::Motorsport
        | Sport::Tft => &[],
    }
}

/// Returns true when the match should be shown as tier 1.
#[must_use]
pub fn is_tier_one(sport: Sport, input: &TierInput) -> bool {
    decide(allowlist(sport), denylist(sport), input)
}

/// Core decision, parameterized over the lists so it can be unit-tested with
/// arbitrary allow/deny sets independent of the shipped consts.
///
/// All comparisons are case-insensitive but allocation-free: the allow/deny
/// consts are lowercase and slugs are short, so we compare bytes directly
/// (`eq_ignore_ascii_case` / a no-alloc ASCII-CI substring scan) rather than
/// lowercasing each slug into a fresh `String` — this runs once per raw match,
/// thousands of times per deep scan.
fn decide(allow: &[&str], deny: &[&str], input: &TierInput) -> bool {
    let present = [input.league_slug, input.series_slug, input.tournament_slug]
        .into_iter()
        .flatten();

    // Allowlist (exact match) beats denylist (substring) beats base tier. A
    // single pass suffices: allow wins immediately; deny is remembered but can
    // still be overridden by an allow hit on a later slug.
    let mut denied = false;
    for s in present {
        // 1. Allowlist — exact full-slug match wins outright.
        if allow.iter().any(|a| a.eq_ignore_ascii_case(s)) {
            return true;
        }
        // 2. Denylist — substring match excludes (unless a later slug allows).
        if !denied && deny.iter().any(|bad| contains_ascii_ci(s, bad)) {
            denied = true;
        }
    }
    if denied {
        return false;
    }

    // 3. Base — tournament tier S or A.
    input
        .tier
        .is_some_and(|t| t.eq_ignore_ascii_case("s") || t.eq_ignore_ascii_case("a"))
}

/// Case-insensitive ASCII substring test with no allocation. `needle` is assumed
/// already-lowercase (every denylist const is), so only the haystack is folded
/// as the windows are compared.
fn contains_ascii_ci(haystack: &str, needle: &str) -> bool {
    let (h, n) = (haystack.as_bytes(), needle.as_bytes());
    if n.is_empty() {
        return true;
    }
    if n.len() > h.len() {
        return false;
    }
    h.windows(n.len())
        .any(|w| w.iter().zip(n).all(|(a, b)| a.eq_ignore_ascii_case(b)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn league<'a>(slug: &'a str, tier: Option<&'a str>) -> TierInput<'a> {
        TierInput {
            league_slug: Some(slug),
            tier,
            ..Default::default()
        }
    }

    #[test]
    fn base_keeps_s_and_a_tiers() {
        assert!(is_tier_one(
            Sport::Lol,
            &league("league-of-legends-lck", Some("a"))
        ));
        assert!(is_tier_one(
            Sport::Cs2,
            &league("cs-go-blast-premier", Some("s"))
        ));
        // Case-insensitive tier.
        assert!(is_tier_one(
            Sport::Lol,
            &league("league-of-legends-lec", Some("A"))
        ));
    }

    #[test]
    fn base_drops_low_and_missing_tiers() {
        assert!(!is_tier_one(
            Sport::Cs2,
            &league("cs-go-some-cup", Some("b"))
        ));
        assert!(!is_tier_one(
            Sport::Lol,
            &league("league-of-legends-prm", None)
        ));
    }

    #[test]
    fn denylist_overrides_base() {
        // Challenger league tagged A is still noise.
        assert!(!is_tier_one(
            Sport::Lol,
            &league("league-of-legends-lck-challengers", Some("a"))
        ));
        // Qualifier to a major, even tagged S, is excluded.
        assert!(!is_tier_one(
            Sport::Cs2,
            &league("cs-go-major-open-qualifier", Some("s"))
        ));
    }

    #[test]
    fn allowlist_overrides_denylist_and_base() {
        let allow = ["league-of-legends-lck-challengers"];
        let deny = ["challenger"];
        // Slug matches BOTH allow (exact) and deny (substring) — allow wins.
        let input = league("league-of-legends-lck-challengers", Some("b"));
        assert!(decide(&allow, &deny, &input));
    }

    #[test]
    fn allowlist_is_exact_not_substring() {
        let allow = ["league-of-legends-lck"];
        let deny: [&str; 0] = [];
        // Exact match included.
        assert!(decide(
            &allow,
            &deny,
            &league("league-of-legends-lck", None)
        ));
        // Near-miss NOT force-included by the allowlist (would fall through to
        // base, which is None here -> excluded).
        assert!(!decide(
            &allow,
            &deny,
            &league("league-of-legends-lck-challengers", None)
        ));
    }

    #[test]
    fn denylist_matches_any_slug_field() {
        let input = TierInput {
            league_slug: Some("league-of-legends-lck"),
            tournament_slug: Some("lck-academy-series"),
            tier: Some("s"),
            ..Default::default()
        };
        // Denylisted substring on the tournament slug excludes the match.
        assert!(!is_tier_one(Sport::Lol, &input));
    }
}
