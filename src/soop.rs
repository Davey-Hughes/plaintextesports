//! SOOP (formerly AfreecaTV) live-stream lookup (server-only): Korea's dominant
//! streaming platform, where LCK and Korean pro-player broadcasts live instead of
//! Twitch. Uses SOOP's unofficial station endpoint (no API key), so enrichment is
//! opt-in via `soop_enabled`; any error is a no-op that returns an empty map, and
//! the page never depends on it.

use once_cell::sync::Lazy;
use serde::Deserialize;
use std::collections::HashMap;

/// Browser-like User-Agent. Unlike our other feeds, SOOP's station endpoint
/// serves a 404 HTML error page to a non-browser UA (it doesn't accept the
/// project's contact UA), so this unofficial endpoint needs one that looks like a
/// browser. Kept local rather than in `http::USER_AGENT` for exactly that reason.
const BROWSER_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
    AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36";

/// One outbound client (connection pool) reused across calls. Timeouts keep a
/// hung upstream (chapi.sooplive.co.kr) from pinning the enrichment task and a
/// pooled connection indefinitely — reqwest has none by default.
static CLIENT: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .user_agent(BROWSER_UA)
        .connect_timeout(std::time::Duration::from_secs(4))
        .timeout(std::time::Duration::from_secs(8))
        .build()
        // The browser UA is required (a default client gets a 404 HTML page), and
        // a build failure would otherwise fall back to a UA-less default (itself a
        // panic in reqwest); fail loudly and specifically instead.
        .expect("failed to build the SOOP HTTP client")
});

/// Live viewer count for a SOOP channel that's broadcasting now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveInfo {
    pub viewers: u64,
}

/// First-path-segment words SOOP reserves for pages/APIs rather than channels, so
/// a URL like `sooplive.com/live/all` doesn't resolve to a bogus "live" channel.
const RESERVED: &[&str] = &[
    "afreeca",
    "video",
    "player",
    "embed",
    "api",
    "live",
    "vod",
    "watch",
    "auth",
    "directory",
    "search",
    "app",
    "board",
    "post",
    "story",
    "feed",
    "hashtag",
    "signin",
    "event",
    "about",
];

/// The lowercased SOOP broadcaster id from a channel URL, or `None` for non-SOOP
/// URLs and forms we can't resolve to a channel. Handles `{host}/{id}`,
/// `{host}/{id}/{broad_no}`, and the `{host}/station/{id}/...` form, across the
/// sooplive / afreeca hosts.
pub fn bid_of(url: &str) -> Option<String> {
    let lower = url.to_ascii_lowercase();
    let after = lower.split_once("//").map_or(lower.as_str(), |(_, r)| r);
    let (host, path) = after.split_once('/').unwrap_or((after, ""));
    if !(host.contains("sooplive") || host.contains("afreeca") || host.contains("soop")) {
        return None;
    }
    let mut segs = path.split('/').filter(|s| !s.is_empty());
    let clean = |s: &str| s.split(['?', '#']).next().unwrap_or("").trim().to_string();
    let first = clean(segs.next()?);
    // `/station/<id>/...` puts the channel in the second segment.
    let id = if first == "station" {
        clean(segs.next()?)
    } else {
        first
    };
    if id.is_empty() || RESERVED.contains(&id.as_str()) {
        return None;
    }
    // SOOP ids are lowercase alphanumerics plus `_`/`-`; anything else (a '.' in
    // an API path, say) isn't a channel.
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return None;
    }
    Some(id)
}

/// Parse a station-endpoint body into `Some(LiveInfo)` when the channel is live
/// (a non-null `broad` object), else `None`. Malformed input yields `None` —
/// never panics.
fn parse_station(json: &str) -> Option<LiveInfo> {
    #[derive(Deserialize)]
    struct StationResp {
        broad: Option<Broad>,
    }
    #[derive(Deserialize)]
    struct Broad {
        #[serde(default)]
        current_sum_viewer: u64,
    }
    let resp: StationResp = serde_json::from_str(json).ok()?;
    let broad = resp.broad?;
    Some(LiveInfo {
        viewers: broad.current_sum_viewer,
    })
}

/// Live status for `bids` (offline/unknown channels are absent). Empty map on any
/// network/parse error — callers degrade to "no live info", so the page never
/// breaks. One station GET per channel; a match carries only a handful and the
/// merged result is cached by `live_streams_for`, so this stays cheap.
pub async fn live_streams(bids: &[String]) -> HashMap<String, LiveInfo> {
    let mut uniq: Vec<String> = bids
        .iter()
        .map(|b| b.to_ascii_lowercase())
        .filter(|b| !b.is_empty())
        .collect();
    uniq.sort();
    uniq.dedup();
    let mut out = HashMap::new();
    for bid in uniq {
        let url = format!("https://chapi.sooplive.co.kr/api/{bid}/station");
        let resp = match CLIENT.get(&url).send().await {
            Ok(r) => r,
            Err(_) => continue,
        };
        if let Ok(body) = resp.text().await
            && let Some(info) = parse_station(&body)
        {
            out.insert(bid, info);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bid_of_extracts_broadcaster_id() {
        // Plain channel URL.
        assert_eq!(
            bid_of("https://www.sooplive.com/he0901").as_deref(),
            Some("he0901")
        );
        // play. host with a broadcast number appended.
        assert_eq!(
            bid_of("https://play.sooplive.com/pyh3646/237852185").as_deref(),
            Some("pyh3646")
        );
        // /station/<id>/... form.
        assert_eq!(
            bid_of("https://www.sooplive.com/station/lolglobal/vod/normal").as_deref(),
            Some("lolglobal")
        );
        // Korean channel host.
        assert_eq!(
            bid_of("https://ch.sooplive.co.kr/ecvhao").as_deref(),
            Some("ecvhao")
        );
        // Legacy afreeca host, trailing slash.
        assert_eq!(
            bid_of("https://play.afreecatv.com/faker/").as_deref(),
            Some("faker")
        );
        // Uppercase is normalised.
        assert_eq!(
            bid_of("https://www.sooplive.com/HE0901").as_deref(),
            Some("he0901")
        );
        // Query string on the id is stripped.
        assert_eq!(
            bid_of("https://www.sooplive.com/he0901?foo=1").as_deref(),
            Some("he0901")
        );
    }

    #[test]
    fn bid_of_rejects_non_channels() {
        assert_eq!(bid_of("https://www.twitch.tv/lck"), None);
        assert_eq!(bid_of("https://youtube.com/@x"), None);
        assert_eq!(bid_of(""), None);
        // SOOP host but no channel segment.
        assert_eq!(bid_of("https://www.sooplive.com/"), None);
        // Reserved directory word, not a channel.
        assert_eq!(bid_of("https://www.sooplive.com/live/all"), None);
        // API path (has a '.') is not a channel id.
        assert_eq!(
            bid_of("https://live.sooplive.co.kr/afreeca/player_live_api.php"),
            None
        );
    }

    #[test]
    fn parse_station_reads_live_viewers() {
        let json = r#"{"station":{"user_nick":"x"},"broad":{"broad_no":295300529,
            "broad_title":"t","current_sum_viewer":1252,"broad_cate_no":40019}}"#;
        assert_eq!(parse_station(json), Some(LiveInfo { viewers: 1252 }));
    }

    #[test]
    fn parse_station_offline_and_malformed_are_none() {
        // Offline: broad is null.
        assert_eq!(parse_station(r#"{"station":{},"broad":null}"#), None);
        // broad key absent entirely.
        assert_eq!(parse_station(r#"{"station":{}}"#), None);
        // Garbage never panics.
        assert_eq!(parse_station("not json"), None);
        assert_eq!(parse_station(""), None);
    }
}
