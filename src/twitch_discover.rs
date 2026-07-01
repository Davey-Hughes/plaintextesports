//! Twitch category discovery (server-only): scan `Get Streams` by game_id +
//! language to surface co-streamers not in the curated list. Reuses `twitch.rs`
//! for auth/client. Unconfigured / any error ⇒ empty, so the page never depends
//! on it.

use crate::config::config;
use crate::types::Sport;
use chrono::{DateTime, Duration, Utc};
use once_cell::sync::Lazy;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{PoisonError, RwLock};

/// A live stream found by category discovery (title/tags kept for attribution).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredStream {
    pub login: String,
    pub viewers: u64,
    pub title: String,
    pub tags: Vec<String>,
}

/// Built-in sport → Twitch category id (CS2 = the "Counter-Strike" category
/// `32399`, which absorbed CS:GO's id). Overridable via config.
const BUILTIN_GAME_IDS: &[(Sport, &str)] = &[(Sport::Lol, "21779"), (Sport::Cs2, "32399")];

/// The Twitch category id for `sport`: a config override wins, else the built-in
/// table; `None` for a sport with no mapping (never discovered).
pub fn game_id(sport: Sport, overrides: &HashMap<String, String>) -> Option<String> {
    if let Some(id) = overrides.get(sport.slug()) {
        return Some(id.clone());
    }
    BUILTIN_GAME_IDS
        .iter()
        .find(|(s, _)| *s == sport)
        .map(|(_, id)| (*id).to_string())
}

#[derive(Deserialize, Default)]
struct StreamsResp {
    #[serde(default)]
    data: Vec<RawDiscover>,
    #[serde(default)]
    pagination: Pagination,
}

#[derive(Deserialize, Default)]
struct Pagination {
    #[serde(default)]
    cursor: Option<String>,
}

#[derive(Deserialize)]
struct RawDiscover {
    #[serde(default)]
    user_login: String,
    #[serde(default)]
    viewer_count: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    tags: Vec<String>,
}

/// Parse a `Get Streams` body into `(streams, next cursor)`. Empty-login rows are
/// dropped; logins are lowercased. Malformed input ⇒ `(vec![], None)`, no panic.
pub fn parse_discover_page(json: &str) -> (Vec<DiscoveredStream>, Option<String>) {
    let resp: StreamsResp = serde_json::from_str(json).unwrap_or_default();
    let streams = resp
        .data
        .into_iter()
        .filter(|r| !r.user_login.is_empty())
        .map(|r| DiscoveredStream {
            login: r.user_login.to_ascii_lowercase(),
            viewers: r.viewer_count,
            title: r.title,
            tags: r.tags,
        })
        .collect();
    (streams, resp.pagination.cursor)
}

/// Per-game cache of the last category scan, keyed by game_id. One scan per game
/// per TTL serves every live match of that game.
static DISCOVER_CACHE: Lazy<RwLock<HashMap<String, (DateTime<Utc>, Vec<DiscoveredStream>)>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));
const DISCOVER_TTL_SECS: i64 = 120;
/// Pages of 100 to pull (top streams are viewer-sorted, so 1–2 pages suffice).
const MAX_PAGES: usize = 2;

/// Live streams in a Twitch category, filtered to `langs`, sorted by viewers
/// desc. Cached ~120s per game_id. Empty when Twitch is unconfigured or on any
/// error — callers degrade to no discovery.
pub async fn discover(game_id: &str, langs: &[String]) -> Vec<DiscoveredStream> {
    {
        let g = DISCOVER_CACHE
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some((at, v)) = g.get(game_id) {
            if *at + Duration::seconds(DISCOVER_TTL_SECS) > Utc::now() {
                return v.clone();
            }
        }
    }
    let Some(id) = config().twitch_client_id.clone().filter(|s| !s.is_empty()) else {
        return Vec::new();
    };
    let Some(token) = crate::twitch::app_token().await else {
        return Vec::new();
    };
    let client = crate::twitch::client();
    let mut out: Vec<DiscoveredStream> = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut q: Vec<(&str, String)> = vec![
            ("game_id", game_id.to_string()),
            ("first", "100".to_string()),
        ];
        for l in langs {
            q.push(("language", l.clone()));
        }
        if let Some(c) = &cursor {
            q.push(("after", c.clone()));
        }
        let resp = match client
            .get("https://api.twitch.tv/helix/streams")
            .query(&q)
            .header("Client-Id", &id)
            .bearer_auth(&token)
            .send()
            .await
        {
            Ok(r) => r,
            Err(_) => break,
        };
        if !resp.status().is_success() {
            break;
        }
        let Ok(body) = resp.text().await else {
            break;
        };
        let (mut page, next) = parse_discover_page(&body);
        out.append(&mut page);
        match next {
            Some(c) if !c.is_empty() => cursor = Some(c),
            _ => break,
        }
    }
    DISCOVER_CACHE
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(game_id.to_string(), (Utc::now(), out.clone()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn parse_discover_page_extracts_streams_and_cursor() {
        let json = r#"{"data":[
            {"user_login":"Caedrel","viewer_count":40000,"title":"MSI CO-STREAM","tags":["English","Esports"]},
            {"user_login":"","viewer_count":5,"title":"x","tags":[]}
        ],"pagination":{"cursor":"abc"}}"#;
        let (v, cursor) = parse_discover_page(json);
        assert_eq!(v.len(), 1); // empty login dropped
        assert_eq!(v[0].login, "caedrel"); // lowercased
        assert_eq!(v[0].viewers, 40000);
        assert_eq!(v[0].title, "MSI CO-STREAM");
        assert_eq!(v[0].tags, vec!["English", "Esports"]);
        assert_eq!(cursor.as_deref(), Some("abc"));
        // Malformed / empty → no panic, empty page, no cursor.
        assert!(parse_discover_page("nonsense").0.is_empty());
        assert!(parse_discover_page(r#"{"data":[]}"#).1.is_none());
    }

    #[test]
    fn game_id_builtin_and_override() {
        let empty = HashMap::new();
        assert_eq!(game_id(Sport::Lol, &empty).as_deref(), Some("21779"));
        assert_eq!(game_id(Sport::Cs2, &empty).as_deref(), Some("32399"));
        assert_eq!(game_id(Sport::Mlb, &empty), None);
        let mut ov = HashMap::new();
        ov.insert("lol".to_string(), "999".to_string());
        assert_eq!(game_id(Sport::Lol, &ov).as_deref(), Some("999"));
    }
}
