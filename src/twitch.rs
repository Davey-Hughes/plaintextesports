//! Twitch Helix client (server-only): app-token auth + live-stream lookup for
//! esports co-stream enrichment. Credentials come from config; absent ⇒ every
//! call is a no-op that returns an empty map, so the page never depends on it.

use crate::config::config;
use crate::http::USER_AGENT;
use chrono::{DateTime, Duration, Utc};
use once_cell::sync::Lazy;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{PoisonError, RwLock};

/// One outbound client (connection pool) reused across calls. Timeouts keep a
/// hung upstream (Helix / id.twitch.tv / gql.twitch.tv) from pinning the
/// enrichment task and a pooled connection indefinitely — reqwest has none by
/// default. Shared by the discovery + GQL modules too (`client()`).
static CLIENT: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(std::time::Duration::from_secs(4))
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .unwrap_or_default()
});

struct AppToken {
    token: String,
    expires_at: DateTime<Utc>,
}

/// Cached client-credentials app token (valid ~57 days; refreshed on expiry/401).
static TOKEN: Lazy<RwLock<Option<AppToken>>> = Lazy::new(|| RwLock::new(None));

/// Live viewer count + current game for a channel that's streaming now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveInfo {
    pub viewers: u64,
    pub game: String,
    /// ISO-639-1 stream language from Helix (e.g. "en"), empty when absent.
    pub language: String,
}

/// The lowercased Twitch channel login from a `twitch.tv/<login>` URL, or `None`
/// for non-Twitch URLs and embed/query forms we don't link to.
pub fn login_of(url: &str) -> Option<String> {
    let lower = url.to_ascii_lowercase();
    let rest = lower.split("twitch.tv/").nth(1)?;
    let login = rest.split(['/', '?', '#']).next()?.trim();
    (!login.is_empty()).then(|| login.to_string())
}

#[derive(Deserialize)]
struct StreamsResp {
    #[serde(default)]
    data: Vec<RawLive>,
}

#[derive(Deserialize)]
struct RawLive {
    #[serde(default)]
    user_login: String,
    #[serde(default)]
    viewer_count: u64,
    #[serde(default)]
    game_name: String,
    #[serde(default)]
    language: String,
}

/// Parse a Get Streams body into `login → LiveInfo` (lowercased logins). Malformed
/// input yields an empty map — never panics.
fn parse_streams(json: &str) -> HashMap<String, LiveInfo> {
    let resp: StreamsResp = serde_json::from_str(json).unwrap_or(StreamsResp { data: Vec::new() });
    resp.data
        .into_iter()
        .filter(|r| !r.user_login.is_empty())
        .map(|r| {
            (
                r.user_login.to_ascii_lowercase(),
                LiveInfo {
                    viewers: r.viewer_count,
                    game: r.game_name,
                    language: r.language,
                },
            )
        })
        .collect()
}

/// A valid app token, from cache when fresh, else freshly fetched. `None` when
/// credentials are unset or the token request fails.
async fn token() -> Option<String> {
    let cfg = config();
    let id = cfg.twitch_client_id.clone().filter(|s| !s.is_empty())?;
    let secret = cfg.twitch_client_secret.clone().filter(|s| !s.is_empty())?;
    {
        let g = TOKEN.read().unwrap_or_else(PoisonError::into_inner);
        if let Some(t) = g.as_ref()
            && t.expires_at > Utc::now() + Duration::minutes(5)
        {
            return Some(t.token.clone());
        }
    }
    let resp = CLIENT
        .post("https://id.twitch.tv/oauth2/token")
        .form(&[
            ("client_id", id.as_str()),
            ("client_secret", secret.as_str()),
            ("grant_type", "client_credentials"),
        ])
        .send()
        .await
        .ok()?;
    #[derive(Deserialize)]
    struct Tok {
        access_token: String,
        #[serde(default)]
        expires_in: i64,
    }
    let tok: Tok = resp.json().await.ok()?;
    let expires_at = Utc::now() + Duration::seconds(tok.expires_in.max(0));
    let token = tok.access_token.clone();
    *TOKEN.write().unwrap_or_else(PoisonError::into_inner) = Some(AppToken {
        token: token.clone(),
        expires_at,
    });
    Some(token)
}

/// The shared Helix HTTP client, reused by the discovery + GQL modules.
pub(crate) fn client() -> &'static reqwest::Client {
    &CLIENT
}

/// A valid app token for sibling modules (delegates to the cached `token()`).
pub(crate) async fn app_token() -> Option<String> {
    token().await
}

/// Live status for `logins` (offline/unknown channels are absent). Empty map when
/// Twitch is unconfigured or on any network/auth error — callers degrade to "no
/// live info", so the page never breaks.
pub async fn live_streams(logins: &[String]) -> HashMap<String, LiveInfo> {
    let mut uniq: Vec<String> = logins.iter().map(|l| l.to_ascii_lowercase()).collect();
    uniq.sort();
    uniq.dedup();
    if uniq.is_empty() {
        return HashMap::new();
    }
    let Some(id) = config().twitch_client_id.clone().filter(|s| !s.is_empty()) else {
        return HashMap::new();
    };
    let Some(token) = token().await else {
        return HashMap::new();
    };
    let mut out = HashMap::new();
    for chunk in uniq.chunks(100) {
        let q: Vec<(&str, &str)> = chunk.iter().map(|l| ("user_login", l.as_str())).collect();
        let resp = match CLIENT
            .get("https://api.twitch.tv/helix/streams")
            .query(&q)
            .header("Client-Id", &id)
            .bearer_auth(&token)
            .send()
            .await
        {
            Ok(r) => r,
            Err(_) => continue,
        };
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            // Token rejected — drop it so the next call refetches.
            *TOKEN.write().unwrap_or_else(PoisonError::into_inner) = None;
            continue;
        }
        if let Ok(body) = resp.text().await {
            out.extend(parse_streams(&body));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_of_extracts_channel() {
        assert_eq!(
            login_of("https://www.twitch.tv/ESL_CSGO").as_deref(),
            Some("esl_csgo")
        );
        assert_eq!(login_of("https://twitch.tv/lck/").as_deref(), Some("lck"));
        assert_eq!(
            login_of("https://www.twitch.tv/riotgames?x=1").as_deref(),
            Some("riotgames")
        );
        assert_eq!(login_of("https://youtube.com/@x"), None);
        assert_eq!(login_of("https://player.twitch.tv/?channel=x"), None);
        assert_eq!(login_of(""), None);
    }

    #[test]
    fn parse_streams_keeps_live_only() {
        let json = r#"{"data":[
            {"user_login":"Gaules","viewer_count":375,"game_name":"Back 4 Blood","language":"pt","type":"live"},
            {"user_login":"esl_dota2","viewer_count":31,"game_name":"Dota 2","type":"live"}
        ]}"#;
        let m = parse_streams(json);
        assert_eq!(m.len(), 2);
        assert_eq!(
            m.get("gaules").unwrap(),
            &LiveInfo {
                viewers: 375,
                game: "Back 4 Blood".to_string(),
                language: "pt".to_string(), // captured from Helix
            }
        );
        assert_eq!(m.get("esl_dota2").unwrap().viewers, 31);
        assert_eq!(m.get("esl_dota2").unwrap().language, ""); // absent → empty
        // Empty / malformed → empty map, never panics.
        assert!(parse_streams(r#"{"data":[]}"#).is_empty());
        assert!(parse_streams("not json").is_empty());
    }
}
