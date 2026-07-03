//! YouTube Data API v3 client (server-only): channel-handle resolution (cached)
//! + live-stream lookup via the cheap uploads→videos.list path ("Method B"),
//! under an in-memory daily-unit budget. Key absent ⇒ every call is a no-op that
//! returns an empty map, so the page never depends on it.

use crate::config::config;
use crate::http::USER_AGENT;
use chrono::{DateTime, Duration, NaiveDate, Utc};
use once_cell::sync::Lazy;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{PoisonError, RwLock};

const API: &str = "https://www.googleapis.com/youtube/v3";
/// How deep to scan a channel's recent uploads for a live stream. Freshly-started
/// event streams (co-streamer watch parties, WEC races) sit at the top.
const UPLOADS_DEPTH: usize = 15;
/// In-memory daily unit ceiling — well under the 10k/day quota, headroom to spare.
pub const YT_DAILY_BUDGET: u32 = 8000;

/// Timeouts keep a hung googleapis.com from pinning the enrichment task and a
/// pooled connection indefinitely — reqwest sets none by default.
static CLIENT: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(std::time::Duration::from_secs(4))
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .unwrap_or_default()
});
/// (utc_date, units_used_today) — resets on date change.
static BUDGET: Lazy<RwLock<(NaiveDate, u32)>> =
    Lazy::new(|| RwLock::new((Utc::now().date_naive(), 0)));
/// `@handle` → `UC…` channel id, cached forever (a channel id is permanent).
static ID_CACHE: Lazy<RwLock<HashMap<String, String>>> = Lazy::new(|| RwLock::new(HashMap::new()));
/// Single-channel live-status cache (~90s TTL) for the WEC watch-line badge, so
/// repeated views during a session don't re-hit the API.
static LIVE_CACHE: Lazy<RwLock<HashMap<String, (DateTime<Utc>, Option<LiveInfo>)>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));
const LIVE_CACHE_TTL_SECS: i64 = 90;

/// Live viewer count for a channel that's streaming now (`None` = hidden count).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveInfo {
    pub viewers: Option<u64>,
}

/// Normalize a YouTube channel handle or URL to a canonical identifier: `@handle`
/// (lowercased) or a `UC…` channel id. `None` for non-YouTube / unrecognized.
pub fn channel_ident(s: &str) -> Option<String> {
    let t = s.trim();
    if let Some(rest) = t.strip_prefix("@") {
        let h = rest.split(['/', '?', '#']).next().unwrap_or("").trim();
        return (!h.is_empty()).then(|| format!("@{}", h.to_ascii_lowercase()));
    }
    if t.starts_with("UC") && !t.contains('/') && !t.contains(' ') {
        return Some(t.to_string());
    }
    let low = t.to_ascii_lowercase();
    if !low.contains("youtube.com") && !low.contains("youtu.be") {
        return None;
    }
    if let Some(i) = t.find("/channel/") {
        let cid = t[i + "/channel/".len()..]
            .split(['/', '?', '#'])
            .next()
            .unwrap_or("")
            .trim();
        return (cid.starts_with("UC")).then(|| cid.to_string());
    }
    if let Some(i) = t.find("/@") {
        let h = t[i + 2..]
            .split(['/', '?', '#'])
            .next()
            .unwrap_or("")
            .trim();
        return (!h.is_empty()).then(|| format!("@{}", h.to_ascii_lowercase()));
    }
    None
}

/// A YouTube URL classified for channel resolution: a specific `Video` (resolved
/// to its channel via `videos.list`), a `Channel` id already in hand, or a
/// `Handle`/custom name (resolved via `channels.list?forHandle`).
#[derive(Debug, PartialEq, Eq)]
enum YtRef {
    Video(String),
    Channel(String),
    Handle(String),
}

/// Classify a YouTube URL/handle into the form we can resolve to a channel id.
/// `None` for non-YouTube URLs and reserved paths (`/results`, a bare `/watch`
/// with no `v=`, …). Pure — the API resolution happens in `channel_id_of_url`.
fn youtube_ref(url: &str) -> Option<YtRef> {
    let t = url.trim();
    if t.is_empty() {
        return None;
    }
    // Bare forms (no scheme): @handle, or a UC… channel id.
    if let Some(rest) = t.strip_prefix('@') {
        let h = rest.split(['/', '?', '#']).next().unwrap_or("").trim();
        return (!h.is_empty()).then(|| YtRef::Handle(format!("@{}", h.to_ascii_lowercase())));
    }
    if t.starts_with("UC") && !t.contains('/') && !t.contains(' ') {
        return Some(YtRef::Channel(t.to_string()));
    }
    let low = t.to_ascii_lowercase();
    if !low.contains("youtube.com") && !low.contains("youtu.be") {
        return None;
    }
    // youtu.be/<id> is always an opaque video id.
    if let Some(i) = low.find("youtu.be/") {
        let id = t[i + "youtu.be/".len()..]
            .split(['/', '?', '#'])
            .next()
            .unwrap_or("")
            .trim();
        return (!id.is_empty()).then(|| YtRef::Video(id.to_string()));
    }
    let after = t
        .split_once("youtube.com/")
        .map(|(_, r)| r)
        .unwrap_or("")
        .trim_start_matches('/');
    if after.is_empty() {
        return None;
    }
    let first = after.split('/').next().unwrap_or("");
    let first_clean = first.split(['?', '#']).next().unwrap_or("");
    let second = || {
        after
            .split('/')
            .nth(1)
            .unwrap_or("")
            .split(['?', '#'])
            .next()
            .unwrap_or("")
            .trim()
    };
    match first_clean {
        // /watch?v=<id> — the id lives in the query string.
        "watch" => after
            .split_once('?')
            .map(|(_, q)| q)
            .unwrap_or("")
            .split('&')
            .find_map(|kv| kv.strip_prefix("v="))
            .map(|v| v.split(['&', '#']).next().unwrap_or("").trim())
            .filter(|id| !id.is_empty())
            .map(|id| YtRef::Video(id.to_string())),
        // /live/<id> and /embed/<id> carry the video id in the next segment.
        "live" | "embed" => {
            let id = second();
            (!id.is_empty()).then(|| YtRef::Video(id.to_string()))
        }
        // /channel/<UC…> — an explicit channel id.
        "channel" => {
            let cid = second();
            cid.starts_with("UC")
                .then(|| YtRef::Channel(cid.to_string()))
        }
        // /c/<name> and /user/<name> put the custom name in the next segment.
        "c" | "user" => {
            let name = second();
            (!name.is_empty()).then(|| YtRef::Handle(format!("@{}", name.to_ascii_lowercase())))
        }
        // Reserved non-channel paths.
        "shorts" | "playlist" | "results" | "feed" => None,
        // /@handle
        h if h.starts_with('@') => {
            let hh = h.trim_start_matches('@');
            (!hh.is_empty()).then(|| YtRef::Handle(format!("@{}", hh.to_ascii_lowercase())))
        }
        // A bare custom name (optionally with a trailing `/live`) — treat it as a
        // handle; a wrong guess just fails to resolve, so dedup safely no-ops.
        name => Some(YtRef::Handle(format!("@{}", name.to_ascii_lowercase()))),
    }
}

/// True when a YouTube URL points at a channel (an `@handle`, custom name, or
/// `/channel/UC…`) rather than a specific video (`/watch`, `/live/<id>`,
/// `youtu.be/<id>`). Used to prefer a stable channel permalink when collapsing
/// duplicate streams that resolve to the same channel.
pub fn is_channel_url(url: &str) -> bool {
    matches!(youtube_ref(url), Some(YtRef::Handle(_) | YtRef::Channel(_)))
}

/// Try to reserve `n` units of today's budget; false (and reserves nothing) if it
/// would exceed the ceiling. Resets the counter when the UTC date rolls over.
fn spend(n: u32) -> bool {
    let today = Utc::now().date_naive();
    let mut g = BUDGET.write().unwrap_or_else(PoisonError::into_inner);
    if g.0 != today {
        *g = (today, 0);
    }
    if g.1.saturating_add(n) > YT_DAILY_BUDGET {
        return false;
    }
    g.1 += n;
    true
}

#[cfg(test)]
fn reset_budget_for_test() {
    *BUDGET.write().unwrap_or_else(PoisonError::into_inner) = (Utc::now().date_naive(), 0);
}

#[derive(Deserialize)]
struct VideosResp {
    #[serde(default)]
    items: Vec<VideoItem>,
}
#[derive(Deserialize)]
struct VideoItem {
    #[serde(default)]
    id: String,
    #[serde(default)]
    snippet: VideoSnippet,
    #[serde(default)]
    #[serde(rename = "liveStreamingDetails")]
    live: VideoLive,
}
#[derive(Deserialize, Default)]
struct VideoSnippet {
    #[serde(default)]
    #[serde(rename = "liveBroadcastContent")]
    live_broadcast_content: String,
}
#[derive(Deserialize, Default)]
struct VideoLive {
    #[serde(default)]
    #[serde(rename = "concurrentViewers")]
    concurrent_viewers: Option<String>,
}

/// Parse a `videos.list` body into `video_id → Option<viewers>` for the live ones
/// (`liveBroadcastContent == "live"`; `concurrentViewers` is a string, hidden ⇒
/// None). Malformed input ⇒ empty map, never panics.
fn parse_live_videos(json: &str) -> HashMap<String, Option<u64>> {
    let resp: VideosResp = serde_json::from_str(json).unwrap_or(VideosResp { items: Vec::new() });
    resp.items
        .into_iter()
        .filter(|v| v.snippet.live_broadcast_content == "live" && !v.id.is_empty())
        .map(|v| {
            (
                v.id,
                v.live
                    .concurrent_viewers
                    .and_then(|s| s.parse::<u64>().ok()),
            )
        })
        .collect()
}

async fn get_json(url: &str) -> Option<serde_json::Value> {
    let resp = CLIENT.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json::<serde_json::Value>().await.ok()
}

/// Resolve an ident to its `UC…` channel id (a `UC…` ident passes through; an
/// `@handle` is resolved via `channels.list?forHandle` and cached forever).
async fn resolve_channel_id(ident: &str, key: &str) -> Option<String> {
    if ident.starts_with("UC") {
        return Some(ident.to_string());
    }
    if let Some(cid) = ID_CACHE
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .get(ident)
    {
        return Some(cid.clone());
    }
    if !spend(1) {
        return None;
    }
    let handle = ident.trim_start_matches('@');
    let url = format!("{API}/channels?part=id&forHandle={handle}&key={key}");
    let v = get_json(&url).await?;
    let cid = v.get("items")?.get(0)?.get("id")?.as_str()?.to_string();
    ID_CACHE
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(ident.to_string(), cid.clone());
    Some(cid)
}

/// Resolve a video id to its channel's `UC…` id via `videos.list`, cached forever
/// (a video never changes channels). `None` on error or spent budget.
async fn video_channel_id(vid: &str, key: &str) -> Option<String> {
    let cache_key = format!("vid:{vid}");
    if let Some(cid) = ID_CACHE
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&cache_key)
    {
        return Some(cid.clone());
    }
    if !spend(1) {
        return None;
    }
    let url = format!("{API}/videos?part=snippet&id={vid}&key={key}");
    let v = get_json(&url).await?;
    let cid = v
        .get("items")?
        .get(0)?
        .get("snippet")?
        .get("channelId")?
        .as_str()?
        .to_string();
    ID_CACHE
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(cache_key, cid.clone());
    Some(cid)
}

/// Resolve any YouTube URL/handle to its channel's `UC…` id: a `/watch`, `/live`,
/// `/embed`, or `youtu.be` video → its channel via `videos.list`; an `@handle` or
/// custom name → `channels.forHandle`; a `UC…`/`/channel/UC…` passes straight
/// through (no key needed). `None` for non-YouTube or unresolvable inputs, so a
/// caller can safely skip dedup rather than break.
pub async fn channel_id_of_url(url: &str) -> Option<String> {
    let r = youtube_ref(url)?;
    if let YtRef::Channel(uc) = &r {
        return Some(uc.clone());
    }
    let key = config().youtube_api_key.clone().filter(|k| !k.is_empty())?;
    match r {
        YtRef::Channel(uc) => Some(uc),
        YtRef::Handle(h) => resolve_channel_id(&h, &key).await,
        YtRef::Video(vid) => video_channel_id(&vid, &key).await,
    }
}

/// Live status for `idents` (offline/unknown absent), keyed by the input ident.
/// Method B: uploads playlist → recent video ids → batched videos.list. Empty map
/// when the key is unset, on any error, or once the daily budget is spent.
pub async fn live_status(idents: &[String]) -> HashMap<String, LiveInfo> {
    let mut out = HashMap::new();
    let Some(key) = config().youtube_api_key.clone().filter(|k| !k.is_empty()) else {
        return out;
    };
    let mut uniq: Vec<String> = idents.to_vec();
    uniq.sort();
    uniq.dedup();

    // Resolve idents → channel ids, then fetch each channel's recent upload ids.
    let mut vid_to_ident: HashMap<String, String> = HashMap::new();
    let mut all_vids: Vec<String> = Vec::new();
    for ident in &uniq {
        let Some(cid) = resolve_channel_id(ident, &key).await else {
            continue;
        };
        let Some(tail) = cid.strip_prefix("UC") else {
            continue;
        };
        if !spend(1) {
            break;
        }
        let uploads = format!("UU{tail}");
        let url = format!(
            "{API}/playlistItems?part=contentDetails&maxResults={UPLOADS_DEPTH}&playlistId={uploads}&key={key}"
        );
        let Some(v) = get_json(&url).await else {
            continue;
        };
        if let Some(items) = v.get("items").and_then(|i| i.as_array()) {
            for it in items {
                if let Some(vid) = it
                    .get("contentDetails")
                    .and_then(|c| c.get("videoId"))
                    .and_then(|s| s.as_str())
                {
                    vid_to_ident
                        .entry(vid.to_string())
                        .or_insert_with(|| ident.clone());
                    all_vids.push(vid.to_string());
                }
            }
        }
    }

    // One batched videos.list per 50 ids → map live videos back to their ident.
    for chunk in all_vids.chunks(50) {
        if !spend(1) {
            break;
        }
        let ids = chunk.join(",");
        let url = format!("{API}/videos?part=snippet,liveStreamingDetails&id={ids}&key={key}");
        let Some(resp) = CLIENT.get(&url).send().await.ok() else {
            continue;
        };
        let Some(body) = resp.text().await.ok() else {
            continue;
        };
        for (vid, viewers) in parse_live_videos(&body) {
            if let Some(ident) = vid_to_ident.get(&vid) {
                out.entry(ident.clone()).or_insert(LiveInfo { viewers });
            }
        }
    }
    out
}

/// Cached live status for a single channel ident (~90s TTL). Used by the WEC
/// watch-line badge. Returns the channel's `LiveInfo` when live, else `None`.
pub async fn live_one(ident: &str) -> Option<LiveInfo> {
    {
        let g = LIVE_CACHE.read().unwrap_or_else(PoisonError::into_inner);
        if let Some((at, v)) = g.get(ident) {
            if *at + Duration::seconds(LIVE_CACHE_TTL_SECS) > Utc::now() {
                return v.clone();
            }
        }
    }
    let map = live_status(std::slice::from_ref(&ident.to_string())).await;
    let v = map.get(ident).cloned();
    LIVE_CACHE
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(ident.to_string(), (Utc::now(), v.clone()));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn youtube_ref_classifies_urls_for_channel_resolution() {
        use YtRef::*;
        // Per-video forms → a video id (resolved to a channel via videos.list).
        assert_eq!(
            youtube_ref("https://www.youtube.com/watch?v=_USAoRuPdl4"),
            Some(Video("_USAoRuPdl4".into()))
        );
        assert_eq!(
            youtube_ref("https://youtu.be/abc123?t=5"),
            Some(Video("abc123".into()))
        );
        assert_eq!(
            youtube_ref("https://www.youtube.com/live/xyz789"),
            Some(Video("xyz789".into()))
        );
        assert_eq!(
            youtube_ref("https://www.youtube.com/embed/vid42?autoplay=1"),
            Some(Video("vid42".into()))
        );
        // Channel-id forms → pass through.
        assert_eq!(
            youtube_ref("https://www.youtube.com/channel/UCabc123"),
            Some(Channel("UCabc123".into()))
        );
        assert_eq!(youtube_ref("UCabc123"), Some(Channel("UCabc123".into())));
        // Handle / custom-name forms → an @handle (resolved via channels.forHandle).
        assert_eq!(
            youtube_ref("https://www.youtube.com/@Caedrel"),
            Some(Handle("@caedrel".into()))
        );
        assert_eq!(youtube_ref("@Sneaky"), Some(Handle("@sneaky".into())));
        // The duplicate case: a bare custom name, with or without a `/live` suffix.
        assert_eq!(
            youtube_ref("https://www.youtube.com/lolesports/live"),
            Some(Handle("@lolesports".into()))
        );
        assert_eq!(
            youtube_ref("https://www.youtube.com/LoLEsports"),
            Some(Handle("@lolesports".into()))
        );
        assert_eq!(
            youtube_ref("https://www.youtube.com/c/SomeCast"),
            Some(Handle("@somecast".into()))
        );
        // Non-YouTube, reserved paths, and empties → not resolvable.
        assert_eq!(youtube_ref("https://www.twitch.tv/x"), None);
        assert_eq!(
            youtube_ref("https://www.youtube.com/results?search_query=x"),
            None
        );
        assert_eq!(youtube_ref("https://www.youtube.com/watch"), None); // no video id
        assert_eq!(youtube_ref(""), None);
    }

    #[test]
    fn channel_ident_normalizes() {
        assert_eq!(
            channel_ident("https://www.youtube.com/@Caedrel").as_deref(),
            Some("@caedrel")
        );
        assert_eq!(
            channel_ident("https://youtube.com/@FIAWEC/streams").as_deref(),
            Some("@fiawec")
        );
        assert_eq!(channel_ident("@Sneaky").as_deref(), Some("@sneaky"));
        assert_eq!(
            channel_ident("https://www.youtube.com/channel/UCabc123").as_deref(),
            Some("UCabc123")
        );
        assert_eq!(channel_ident("UCabc123").as_deref(), Some("UCabc123"));
        assert_eq!(channel_ident("https://www.twitch.tv/x"), None);
        assert_eq!(channel_ident("https://www.youtube.com/results?q=x"), None);
        assert_eq!(channel_ident(""), None);
    }

    #[test]
    fn parse_live_videos_keeps_live_with_viewers() {
        let json = r#"{"items":[
            {"id":"v1","snippet":{"liveBroadcastContent":"live"},"liveStreamingDetails":{"concurrentViewers":"127"}},
            {"id":"v2","snippet":{"liveBroadcastContent":"live"},"liveStreamingDetails":{}},
            {"id":"v3","snippet":{"liveBroadcastContent":"none"},"liveStreamingDetails":{}}
        ]}"#;
        let out = parse_live_videos(json);
        assert_eq!(out.len(), 2);
        assert_eq!(out.get("v1"), Some(&Some(127))); // live with count
        assert_eq!(out.get("v2"), Some(&None)); // live, hidden count
        assert!(!out.contains_key("v3")); // not live
        assert!(parse_live_videos("nonsense").is_empty());
    }

    #[test]
    fn budget_spends_and_resets() {
        // A fresh call for a huge amount is refused; small amounts pass until the cap.
        reset_budget_for_test();
        assert!(spend(10));
        assert!(!spend(YT_DAILY_BUDGET)); // would exceed
        assert!(spend(5));
    }
}
