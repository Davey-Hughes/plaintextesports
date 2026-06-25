//! Resolve exact Liquipedia event-page URLs (server-only).
//!
//! PandaScore doesn't provide Liquipedia links, and an event's Liquipedia title
//! isn't derivable from its name (e.g. "Mid-Season Invitational" → page
//! `Mid-Season_Invitational/2026`). So we query Liquipedia's MediaWiki
//! full-text search and accept the top result only when we're confident it's
//! the right edition (title contains the year and every significant token of
//! the event name). Callers cache the result and resolve each event once — see
//! the resolver loop + SQLite cache in `cache.rs` — which keeps us within
//! Liquipedia's API terms (descriptive User-Agent, aggressive caching, low rate).

use crate::types::Game;
use serde::Deserialize;

type DynError = Box<dyn std::error::Error + Send + Sync>;

const USER_AGENT: &str =
    "plaintextesports/0.1 (https://github.com/ralphpotato/plaintextesports; ralphpotato@gmail.com)";

/// Tokens too generic to require a match on.
const STOPWORDS: &[&str] = &["the", "of", "and"];

const fn wiki(game: Game) -> &'static str {
    match game {
        Game::Cs2 => "counterstrike",
        Game::Lol => "leagueoflegends",
        // Traditional sports aren't resolved against Liquipedia.
        Game::Mlb | Game::Nhl | Game::F1 => "",
    }
}

#[derive(Deserialize)]
struct SearchResp {
    #[serde(default)]
    query: Option<SearchQuery>,
}

#[derive(Deserialize)]
struct SearchQuery {
    #[serde(default)]
    search: Vec<SearchHit>,
}

#[derive(Deserialize)]
struct SearchHit {
    title: String,
}

/// Resolve the exact Liquipedia URL for an event, or `None` if no result is a
/// confident match (caller then keeps its generic fallback link).
pub async fn resolve_event(
    client: &reqwest::Client,
    game: Game,
    league: &str,
    year: i32,
) -> Result<Option<String>, DynError> {
    let wiki = wiki(game);
    let term = format!("{league} {year}");
    let url = format!("https://liquipedia.net/{wiki}/api.php");
    let resp: SearchResp = client
        .get(&url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .query(&[
            ("action", "query"),
            ("list", "search"),
            ("format", "json"),
            ("srlimit", "5"),
            ("srsearch", term.as_str()),
        ])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let hits = resp.query.map(|q| q.search).unwrap_or_default();
    let year = year.to_string();
    let wanted = name_tokens(league);
    Ok(hits
        .into_iter()
        .find(|h| title_matches(&h.title, &year, &wanted))
        .map(|h| page_url(wiki, &h.title)))
}

/// Significant lowercase tokens of a name (≥2 chars, minus stopwords).
fn name_tokens(name: &str) -> Vec<String> {
    name.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| t.len() >= 2)
        .map(str::to_ascii_lowercase)
        .filter(|t| !STOPWORDS.contains(&t.as_str()))
        .collect()
}

/// A title is the event iff it contains the year and every wanted token.
fn title_matches(title: &str, year: &str, wanted: &[String]) -> bool {
    if wanted.is_empty() || !title.contains(year) {
        return false;
    }
    let have = name_tokens(title);
    wanted.iter().all(|w| have.iter().any(|h| h == w))
}

/// Build the canonical Liquipedia page URL from a title (spaces → `_`,
/// percent-encoded but keeping `/` for subpages).
fn page_url(wiki: &str, title: &str) -> String {
    let underscored: String = title
        .chars()
        .map(|c| if c == ' ' { '_' } else { c })
        .collect();
    format!("https://liquipedia.net/{wiki}/{}", encode_path(&underscored))
}

fn encode_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_exact_edition() {
        let wanted = name_tokens("Mid-Season Invitational");
        assert!(title_matches(
            "Mid-Season Invitational/2026",
            "2026",
            &wanted
        ));
        // Overview page (no year) is rejected.
        assert!(!title_matches("Mid-Season Invitational", "2026", &wanted));
    }

    #[test]
    fn rejects_wrong_event_and_unrelated() {
        // Same org/series prefix but a different event (missing "premier").
        let blast = name_tokens("BLAST Premier");
        assert!(!title_matches("BLAST/Bounty/2026/Winter", "2026", &blast));
        // Unrelated team page for an event with no Liquipedia presence.
        let xse = name_tokens("XSE Pro League");
        assert!(!title_matches("EYEBALLERS", "2026", &xse));
    }

    #[test]
    fn name_tokens_drops_stopwords_and_short() {
        assert_eq!(name_tokens("League of Legends"), vec!["league", "legends"]);
    }

    #[test]
    fn page_url_underscores_and_keeps_subpaths() {
        assert_eq!(
            page_url("leagueoflegends", "Mid-Season Invitational/2026"),
            "https://liquipedia.net/leagueoflegends/Mid-Season_Invitational/2026"
        );
    }
}
