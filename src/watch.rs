//! Shared "where to watch" mapping for national broadcasters, used by the MLB
//! and ESPN (NHL/NBA/NFL) normalizers. Only national networks get a link; local
//! RSNs and radio are rendered as plain text by the callers.

/// A national broadcaster's "where to watch" landing page, when we recognize the
/// network. Applied ONLY to national broadcasts — a local "NBC Sports Bay Area"
/// RSN must never be mislinked to NBC's national page. `None` for unrecognized or
/// local networks; the caller renders those as plain text.
///
/// Matching is case-insensitive substring. Order matters where one name contains
/// another: check the more specific/streaming brand first so it wins.
pub fn national_watch_url(name: &str) -> Option<&'static str> {
    let n = name.to_ascii_uppercase();
    Some(if n.contains("PEACOCK") {
        "https://www.peacocktv.com/"
    } else if n.contains("PARAMOUNT") {
        "https://www.paramountplus.com/"
    } else if n.contains("NBC") {
        "https://www.nbcsports.com/live"
    } else if n.contains("CBS") {
        "https://www.cbssports.com/live/"
    } else if n.contains("FOX") || n.contains("FS1") {
        "https://www.foxsports.com/live"
    } else if n.contains("ESPN") {
        // Covers "ESPN", "ESPN+", "ESPN2".
        "https://www.espn.com/watch/"
    } else if n.contains("ABC") {
        "https://abc.com/watch-live"
    } else if n.contains("TNT") {
        "https://www.tntdrama.com/watchtnt"
    } else if n.contains("TBS") {
        "https://www.tbs.com/watchtbs/now"
    } else if n.contains("MAX") {
        "https://www.max.com/"
    } else if n.contains("NETFLIX") {
        "https://www.netflix.com/"
    } else if n.contains("APPLE") {
        "https://tv.apple.com/"
    } else if n.contains("ROKU") {
        "https://therokuchannel.roku.com/"
    } else if n.contains("PRIME") || n.contains("AMAZON") {
        "https://www.amazon.com/gp/video/storefront"
    } else if n.contains("MLB NETWORK") {
        "https://www.mlb.com/network"
    } else if n.contains("NFL NETWORK") {
        "https://www.nfl.com/network/"
    } else if n.contains("NBA TV") || n.contains("NBA LEAGUE PASS") {
        "https://www.nba.com/watch/"
    } else if n.contains("NHL NETWORK") {
        "https://www.nhl.com/tv"
    } else {
        return None;
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_known_national_networks() {
        for (name, needle) in [
            ("ESPN", "espn.com"),
            ("ESPN+", "espn.com"),
            ("ABC", "abc.com"),
            ("TNT", "tntdrama.com"),
            ("TBS", "tbs.com"),
            ("FOX", "foxsports.com"),
            ("FS1", "foxsports.com"),
            ("NBC", "nbcsports.com"),
            ("Peacock", "peacocktv.com"),
            ("CBS", "cbssports.com"),
            ("Paramount+", "paramountplus.com"),
            ("Prime Video", "amazon.com"),
            ("Netflix", "netflix.com"),
            ("Max", "max.com"),
            ("Apple TV+", "tv.apple.com"),
            ("MLB Network", "mlb.com"),
            ("NFL Network", "nfl.com"),
            ("NBA TV", "nba.com"),
            ("NBA League Pass", "nba.com"),
            ("NHL Network", "nhl.com"),
        ] {
            let url = national_watch_url(name).unwrap_or_else(|| panic!("no url for {name}"));
            assert!(url.contains(needle), "{name} -> {url}, expected {needle}");
        }
    }

    // The helper is name-based; telling a national network from a local RSN is the
    // CALLER's job (the national-market gate), matching MLB's design — so a local
    // "NBC Sports Bay Area" is filtered by the caller, not here. We only assert that
    // genuinely-unrecognized names return None.
    #[test]
    fn unknown_networks_are_none() {
        assert_eq!(national_watch_url(""), None);
        assert_eq!(national_watch_url("FanDuel SN IN"), None);
        assert_eq!(national_watch_url("MSG"), None);
    }
}
