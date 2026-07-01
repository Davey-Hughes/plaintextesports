//! YouTube Data API v3 client (server-only): channel-handle resolution (cached)
//! + live-stream lookup via the cheap uploads→videos.list path ("Method B"),
//! under an in-memory daily-unit budget. Key absent ⇒ every call is a no-op that
//! returns an empty map, so the page never depends on it.

use crate::config::config;
use crate::http::USER_AGENT;
use chrono::{NaiveDate, Utc};
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

static CLIENT: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .unwrap_or_default()
});
/// (utc_date, units_used_today) — resets on date change.
static BUDGET: Lazy<RwLock<(NaiveDate, u32)>> =
    Lazy::new(|| RwLock::new((Utc::now().date_naive(), 0)));
/// `@handle` → `UC…` channel id, cached forever (a channel id is permanent).
static ID_CACHE: Lazy<RwLock<HashMap<String, String>>> = Lazy::new(|| RwLock::new(HashMap::new()));

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
        if cid.len() < 2 {
            continue;
        }
        if !spend(1) {
            break;
        }
        let uploads = format!("UU{}", &cid[2..]);
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

#[cfg(test)]
mod tests {
    use super::*;

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
