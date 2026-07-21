//! Decides whether a match is "tier 1" and should be shown.
//!
//! Precedence (highest first), matching the project plan:
//!   1. ALLOWLIST  — force-include. Exact full-slug match on the league, series,
//!      or tournament slug. Use this for events PandaScore mis-tiers or tags
//!      late. Exact match (not substring) so it can't accidentally pull in
//!      adjacent noise like "...-lck-challengers-...".
//!   2. DENYLIST   — force-exclude. Substring match on any slug. Strips S/A-
//!      tagged noise (academy/challenger/qualifier/showmatch...).
//!   3. BASE       — keep if the tournament tier is in the policy's allowed set
//!      (`[tiers]`; default S/A). ALLOWLIST/DENYLIST merge the built-in consts
//!      below with the policy's config-supplied `extra_allow`/`extra_deny`.
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
        // TFT comes pre-curated from CompeteTFT, so it isn't tier-filtered either.
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

/// The config-driven inputs to the tier decision for one sport: which tiers pass
/// the base rule, plus extra allow/deny slugs that merge with the built-in lists.
/// All slices are borrowed from `Config`, so this is cheap to build per fetch.
#[derive(Clone, Copy, Debug)]
pub struct TierPolicy<'a> {
    /// Tier letters that pass the base rule (lowercased), e.g. `["s","a"]`.
    pub allowed_tiers: &'a [String],
    /// Extra force-include slugs, merged with the built-in allowlist.
    pub extra_allow: &'a [String],
    /// Extra force-exclude slugs, merged with the built-in denylist.
    pub extra_deny: &'a [String],
}

impl<'a> TierPolicy<'a> {
    /// A policy with only a base tier set and no config allow/deny extras.
    #[must_use]
    pub fn tiers(allowed_tiers: &'a [String]) -> Self {
        Self {
            allowed_tiers,
            extra_allow: &[],
            extra_deny: &[],
        }
    }
}

/// Returns true when the match should be shown as tier 1.
#[must_use]
pub fn is_tier_one(sport: Sport, input: &TierInput, policy: &TierPolicy) -> bool {
    decide(allowlist(sport), denylist(sport), input, policy)
}

/// Case-insensitive membership test of a tier letter in the allowed set.
/// `allowed` holds lowercase letters (as the config loader stores them); the
/// tier from the API/DB may be any case (the DB stores it uppercase).
#[must_use]
pub fn tier_allowed(allowed: &[String], tier: &str) -> bool {
    allowed.iter().any(|a| a.eq_ignore_ascii_case(tier))
}

/// Core decision, parameterized over the lists so it can be unit-tested with
/// arbitrary allow/deny sets independent of the shipped consts.
///
/// All comparisons are case-insensitive but allocation-free: the allow/deny
/// consts are lowercase and slugs are short, so we compare bytes directly
/// (`eq_ignore_ascii_case` / a no-alloc ASCII-CI substring scan) rather than
/// lowercasing each slug into a fresh `String` — this runs once per raw match,
/// thousands of times per deep scan.
fn decide(allow: &[&str], deny: &[&str], input: &TierInput, policy: &TierPolicy) -> bool {
    let present = [input.league_slug, input.series_slug, input.tournament_slug]
        .into_iter()
        .flatten();

    // Allowlist (exact match) beats denylist (substring) beats base tier. A
    // single pass suffices: allow wins immediately; deny is remembered but can
    // still be overridden by an allow hit on a later slug.
    let mut denied = false;
    for s in present {
        // 1. Allowlist — exact full-slug match (built-in OR config) wins outright.
        if allow.iter().any(|a| a.eq_ignore_ascii_case(s))
            || policy.extra_allow.iter().any(|a| a.eq_ignore_ascii_case(s))
        {
            return true;
        }
        // 2. Denylist — substring match (built-in OR config) excludes (unless a
        //    later slug allows).
        if !denied
            && (deny.iter().any(|bad| contains_ascii_ci(s, bad))
                || policy
                    .extra_deny
                    .iter()
                    .any(|bad| contains_ascii_ci(s, bad)))
        {
            denied = true;
        }
    }
    if denied {
        return false;
    }

    // 3. Base — tournament tier is in the policy's allowed set.
    input
        .tier
        .is_some_and(|t| tier_allowed(policy.allowed_tiers, t))
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

    /// The default allowed tier set (PandaScore S + A), as the config seeds it.
    fn sa() -> Vec<String> {
        vec!["s".to_string(), "a".to_string()]
    }

    #[test]
    fn base_keeps_s_and_a_tiers() {
        let t = sa();
        assert!(is_tier_one(
            Sport::Lol,
            &league("league-of-legends-lck", Some("a")),
            &TierPolicy::tiers(&t),
        ));
        assert!(is_tier_one(
            Sport::Cs2,
            &league("cs-go-blast-premier", Some("s")),
            &TierPolicy::tiers(&t),
        ));
        // Case-insensitive tier.
        assert!(is_tier_one(
            Sport::Lol,
            &league("league-of-legends-lec", Some("A")),
            &TierPolicy::tiers(&t),
        ));
    }

    #[test]
    fn base_drops_low_and_missing_tiers() {
        let t = sa();
        assert!(!is_tier_one(
            Sport::Cs2,
            &league("cs-go-some-cup", Some("b")),
            &TierPolicy::tiers(&t),
        ));
        assert!(!is_tier_one(
            Sport::Lol,
            &league("league-of-legends-prm", None),
            &TierPolicy::tiers(&t),
        ));
    }

    #[test]
    fn denylist_overrides_base() {
        let t = sa();
        assert!(!is_tier_one(
            Sport::Lol,
            &league("league-of-legends-lck-challengers", Some("a")),
            &TierPolicy::tiers(&t),
        ));
        assert!(!is_tier_one(
            Sport::Cs2,
            &league("cs-go-major-open-qualifier", Some("s")),
            &TierPolicy::tiers(&t),
        ));
    }

    #[test]
    fn allowlist_overrides_denylist_and_base() {
        let allow = ["league-of-legends-lck-challengers"];
        let deny = ["challenger"];
        let t = sa();
        let input = league("league-of-legends-lck-challengers", Some("b"));
        assert!(decide(&allow, &deny, &input, &TierPolicy::tiers(&t)));
    }

    #[test]
    fn allowlist_is_exact_not_substring() {
        let allow = ["league-of-legends-lck"];
        let deny: [&str; 0] = [];
        let t = sa();
        assert!(decide(
            &allow,
            &deny,
            &league("league-of-legends-lck", None),
            &TierPolicy::tiers(&t),
        ));
        assert!(!decide(
            &allow,
            &deny,
            &league("league-of-legends-lck-challengers", None),
            &TierPolicy::tiers(&t),
        ));
    }

    #[test]
    fn denylist_matches_any_slug_field() {
        let t = sa();
        let input = TierInput {
            league_slug: Some("league-of-legends-lck"),
            tournament_slug: Some("lck-academy-series"),
            tier: Some("s"),
            ..Default::default()
        };
        assert!(!is_tier_one(Sport::Lol, &input, &TierPolicy::tiers(&t)));
    }

    #[test]
    fn base_honors_configured_tier_set() {
        // ["s"] only: A-tier no longer passes.
        assert!(!is_tier_one(
            Sport::Lol,
            &league("league-of-legends-lck", Some("a")),
            &TierPolicy::tiers(&["s".to_string()]),
        ));
        // Widen to include B.
        assert!(is_tier_one(
            Sport::Cs2,
            &league("cs-go-some-league", Some("b")),
            &TierPolicy::tiers(&["s".to_string(), "a".to_string(), "b".to_string()]),
        ));
        // Empty set: nothing passes the base rule.
        assert!(!is_tier_one(
            Sport::Lol,
            &league("league-of-legends-lck", Some("s")),
            &TierPolicy::tiers(&[]),
        ));
    }

    #[test]
    fn config_allow_force_includes_regardless_of_tier() {
        let tiers = sa();
        let allow = vec!["cs-go-special-invitational".to_string()];
        let deny: Vec<String> = vec![];
        let policy = TierPolicy {
            allowed_tiers: &tiers,
            extra_allow: &allow,
            extra_deny: &deny,
        };
        // B-tier, but the config allowlist (exact slug) forces it in.
        assert!(is_tier_one(
            Sport::Cs2,
            &league("cs-go-special-invitational", Some("b")),
            &policy,
        ));
    }

    #[test]
    fn config_deny_excludes_even_an_allowed_tier() {
        let tiers = sa();
        let allow: Vec<String> = vec![];
        let deny = vec!["boring".to_string()];
        let policy = TierPolicy {
            allowed_tiers: &tiers,
            extra_allow: &allow,
            extra_deny: &deny,
        };
        // A-tier (allowed), but the config denylist substring excludes it.
        assert!(!is_tier_one(
            Sport::Lol,
            &league("league-of-legends-boring-cup", Some("a")),
            &policy,
        ));
    }

    #[test]
    fn tier_allowed_is_case_insensitive() {
        let allowed = sa();
        assert!(tier_allowed(&allowed, "S"));
        assert!(tier_allowed(&allowed, "a"));
        assert!(!tier_allowed(&allowed, "B"));
    }
}
