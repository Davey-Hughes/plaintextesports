//! Runtime configuration, loaded from `config.toml` with env-var overrides
//! (server-only).

use crate::types::{SiteLink, Sport};
use chrono::{DateTime, Months, Utc};
use chrono_tz::Tz;
use once_cell::sync::Lazy;
use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;

/// `[twitch_discovery]` table shape (all optional).
#[derive(Debug, Default, Deserialize)]
struct TwitchDiscoveryFile {
    enabled_sports: Option<Vec<String>>,
    languages: Option<Vec<String>>,
    min_viewers: Option<u64>,
    max_per_match: Option<usize>,
    game_ids: Option<HashMap<String, String>>,
    gql_costreamers: Option<bool>,
}

/// `config.toml` shape. Everything is optional; missing values fall back to
/// defaults (or a matching env var, which takes precedence).
#[derive(Debug, Default, Deserialize)]
struct FileConfig {
    pandascore_token: Option<String>,
    /// Orange Cat Blacktop API key (WRC + WEC + MotoGP). `x-api-key` header.
    ocblacktop_token: Option<String>,
    /// Twitch app credentials for esports live-stream enrichment.
    twitch_client_id: Option<String>,
    twitch_client_secret: Option<String>,
    /// Curated co-streamers per league: league name → Twitch logins.
    costreamers: Option<HashMap<String, Vec<String>>>,
    /// YouTube Data API v3 key.
    youtube_api_key: Option<String>,
    youtube_costreamers: Option<HashMap<String, Vec<String>>>,
    /// Enable SOOP (formerly AfreecaTV) live enrichment via its unofficial
    /// station endpoint (no key). Off by default since the endpoint is unofficial.
    soop_enabled: Option<bool>,
    /// Curated SOOP co-streamers per game (sport slug) or league — broadcaster ids.
    soop_costreamers: Option<HashMap<String, Vec<String>>>,
    /// Enable the first-party CompeteTFT source (Riot competetft.com + the
    /// official published sheet). Off by default — scrape-based, opt-in.
    competetft_enabled: Option<bool>,
    /// Crawl competetft's schedule feed to auto-discover TFT tournaments.
    competetft_autodiscover: Option<bool>,
    /// Tournament ids to always include (in addition to / instead of discovery).
    competetft_pins: Option<Vec<String>>,
    /// Tournament ids to never include.
    competetft_exclude: Option<Vec<String>>,
    /// PandaScore tiers to show per esports title (slug → tier letters). Only
    /// lol/cs2 have tiers; other keys are ignored. Missing ⇒ ["s","a"].
    tiers: Option<HashMap<String, Vec<String>>>,
    /// Extra force-include slugs per title, merged with the built-in allowlist.
    tier_allow: Option<HashMap<String, Vec<String>>>,
    /// Extra force-exclude slugs per title, merged with the built-in denylist.
    tier_deny: Option<HashMap<String, Vec<String>>>,
    /// Twitch category-discovery of unlisted co-streamers.
    twitch_discovery: Option<TwitchDiscoveryFile>,
    /// League name → Twitch title keywords for discovery attribution.
    twitch_league_aliases: Option<HashMap<String, Vec<String>>>,
    /// Calendar poll interval (s) for a series with a session live or starting
    /// within the hour — the fast tier (only the in-window series pays it).
    ocblacktop_live_poll_secs: Option<u64>,
    /// Calendar poll interval (s) for a series with an event within ~2 weeks.
    ocblacktop_near_poll_secs: Option<u64>,
    /// Calendar poll interval (s) for a series with nothing close (off-season /
    /// between rounds) — the slow passive tier.
    ocblacktop_idle_poll_secs: Option<u64>,
    /// Standings poll interval (s) — a daily floor; a finished round also refreshes
    /// them sooner (edge-triggered), since they only change post-round.
    ocblacktop_standings_poll_secs: Option<u64>,
    /// Hard per-UTC-day cap on Orange Cat Blacktop requests (free tier is 250).
    ocblacktop_daily_cap: Option<u64>,
    demo: Option<bool>,
    display_tz: Option<String>,
    upcoming_days: Option<i64>,
    idle_poll_secs: Option<u64>,
    active_poll_secs: Option<u64>,
    reminder_lead_minutes: Option<i64>,
    archive_months: Option<i64>,
    /// Days to keep a persisted box score after its last fetch before pruning it
    /// from result_cache (default 360; 0 disables pruning).
    box_score_retention_days: Option<i64>,
    /// Days to keep persisted TFT enrichment after its last refresh before pruning
    /// it from result_cache (default 540; 0 disables pruning).
    tft_retention_days: Option<i64>,
    past_refresh: Option<bool>,
    backfill: Option<bool>,
    rate_limit_floor: Option<u64>,
    db_path: Option<String>,
    icons_dir: Option<String>,
    resolve_links: Option<bool>,
    vapid: Option<VapidFile>,
    site: Option<SiteFile>,
}

#[derive(Debug, Default, Deserialize)]
struct VapidFile {
    public: Option<String>,
    private: Option<String>,
    subject: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct SiteFile {
    copyright: Option<String>,
    /// Optional URL — when set, the copyright line links to it.
    copyright_url: Option<String>,
    #[serde(default)]
    links: Vec<SiteLink>,
}

impl FileConfig {
    /// Flatten into the same string keys the env-based parser uses, so both
    /// sources share one code path.
    fn to_map(&self) -> HashMap<String, String> {
        let mut m = HashMap::new();
        let mut put = |k: &str, v: Option<String>| {
            if let Some(v) = v {
                m.insert(k.to_string(), v);
            }
        };
        put("PANDASCORE_TOKEN", self.pandascore_token.clone());
        put("OCBLACKTOP_TOKEN", self.ocblacktop_token.clone());
        put("TWITCH_CLIENT_ID", self.twitch_client_id.clone());
        put("TWITCH_CLIENT_SECRET", self.twitch_client_secret.clone());
        put("YOUTUBE_API_KEY", self.youtube_api_key.clone());
        put("SOOP_ENABLED", self.soop_enabled.map(|b| b.to_string()));
        put(
            "COMPETETFT_ENABLED",
            self.competetft_enabled.map(|b| b.to_string()),
        );
        put(
            "COMPETETFT_AUTODISCOVER",
            self.competetft_autodiscover.map(|b| b.to_string()),
        );
        put(
            "OCBLACKTOP_LIVE_POLL_SECS",
            self.ocblacktop_live_poll_secs.map(|n| n.to_string()),
        );
        put(
            "OCBLACKTOP_NEAR_POLL_SECS",
            self.ocblacktop_near_poll_secs.map(|n| n.to_string()),
        );
        put(
            "OCBLACKTOP_IDLE_POLL_SECS",
            self.ocblacktop_idle_poll_secs.map(|n| n.to_string()),
        );
        put(
            "OCBLACKTOP_STANDINGS_POLL_SECS",
            self.ocblacktop_standings_poll_secs.map(|n| n.to_string()),
        );
        put(
            "OCBLACKTOP_DAILY_CAP",
            self.ocblacktop_daily_cap.map(|n| n.to_string()),
        );
        put("DEMO", self.demo.map(|b| b.to_string()));
        put("DISPLAY_TZ", self.display_tz.clone());
        put("UPCOMING_DAYS", self.upcoming_days.map(|n| n.to_string()));
        put(
            "POLL_INTERVAL_SECS",
            self.idle_poll_secs.map(|n| n.to_string()),
        );
        put(
            "POLL_ACTIVE_SECS",
            self.active_poll_secs.map(|n| n.to_string()),
        );
        put(
            "REMINDER_LEAD_MINUTES",
            self.reminder_lead_minutes.map(|n| n.to_string()),
        );
        put("ARCHIVE_MONTHS", self.archive_months.map(|n| n.to_string()));
        put(
            "BOX_SCORE_RETENTION_DAYS",
            self.box_score_retention_days.map(|n| n.to_string()),
        );
        put(
            "TFT_RETENTION_DAYS",
            self.tft_retention_days.map(|n| n.to_string()),
        );
        put(
            "ENABLE_PAST_REFRESH",
            self.past_refresh.map(|b| b.to_string()),
        );
        put("ENABLE_BACKFILL", self.backfill.map(|b| b.to_string()));
        put(
            "RATE_LIMIT_FLOOR",
            self.rate_limit_floor.map(|n| n.to_string()),
        );
        put("DB_PATH", self.db_path.clone());
        put("ICONS_DIR", self.icons_dir.clone());
        put(
            "ENABLE_LIQUIPEDIA",
            self.resolve_links.map(|b| b.to_string()),
        );
        if let Some(v) = &self.vapid {
            put("VAPID_PUBLIC_KEY", v.public.clone());
            put("VAPID_PRIVATE_KEY", v.private.clone());
            put("VAPID_SUBJECT", v.subject.clone());
        }
        m
    }
}

/// Twitch category-discovery settings. `enabled_sports` empty ⇒ feature off.
#[derive(Clone, Debug, Default)]
pub struct TwitchDiscovery {
    /// Sport slugs the discovery runs for (e.g. "lol", "cs2"). Empty ⇒ off.
    pub enabled_sports: Vec<String>,
    /// Stream languages to include (ISO-639-1). Defaults to ["en"].
    pub languages: Vec<String>,
    /// Skip discovered streams below this viewer count.
    pub min_viewers: u64,
    /// Cap on co-streams appended per match (highest-viewer first).
    pub max_per_match: usize,
    /// Optional sport-slug → Twitch category id override (else built-in table).
    pub game_ids: HashMap<String, String>,
    /// Phase 2: read the official co-streamer list from the unofficial GQL API.
    /// Off until validated against a live co-streamed event.
    pub gql_costreamers: bool,
}

#[derive(Debug, Clone)]
pub struct Config {
    /// PandaScore API token. `None` => serve demo fixture data.
    pub token: Option<String>,
    /// Orange Cat Blacktop API key for WRC + WEC + MotoGP (free tier: 250 req/day).
    /// `None` => those series are skipped. Polled per-series on a proximity-driven
    /// cadence (fast only near a live event), so the daily quota covers all three
    /// with room to spare.
    pub ocblacktop_token: Option<String>,
    /// Twitch app credentials (client-credentials flow). Both must be set to
    /// enable esports live-stream enrichment; otherwise it's a no-op.
    pub twitch_client_id: Option<String>,
    pub twitch_client_secret: Option<String>,
    /// Curated co-streamers per league (league name → Twitch logins), surfaced
    /// when live and streaming the match's game. Empty when unconfigured.
    pub costreamers: HashMap<String, Vec<String>>,
    /// YouTube Data API v3 key. `None` ⇒ YouTube enrichment is a no-op.
    pub youtube_api_key: Option<String>,
    /// Curated YouTube co-streamers per game (sport slug) or league — channel
    /// handles/URLs, surfaced when live (not game-filtered). Empty when unset.
    pub youtube_costreamers: HashMap<String, Vec<String>>,
    /// Enable SOOP live enrichment (live status + viewer counts on SOOP streams,
    /// plus curated SOOP co-streamers). Off by default — it uses SOOP's
    /// unofficial station endpoint, so it's opt-in.
    pub soop_enabled: bool,
    /// Curated SOOP co-streamers per game (sport slug) or league — broadcaster
    /// ids, surfaced when live (not game-filtered). Empty when unset.
    pub soop_costreamers: HashMap<String, Vec<String>>,
    /// Enable the first-party CompeteTFT source (opt-in). Primary for TFT when on.
    pub competetft_enabled: bool,
    /// Auto-discover tournaments from competetft's schedule feed.
    pub competetft_autodiscover: bool,
    /// Tournament ids always included.
    pub competetft_pins: Vec<String>,
    /// Tournament ids never included.
    pub competetft_exclude: Vec<String>,
    /// Twitch category-discovery of unlisted co-streamers. Off when
    /// `enabled_sports` is empty.
    pub twitch_discovery: TwitchDiscovery,
    /// League name → Twitch title keywords for attributing discovered streams
    /// (e.g. "Mid-Season Invitational" → ["MSI"]). Empty when unconfigured.
    pub twitch_league_aliases: HashMap<String, Vec<String>>,
    /// PandaScore tiers to show per title (slug → lowercased letters). Seeded with
    /// ["s","a"] for lol/cs2; overridden by `[tiers]`. Read by ingest + display.
    pub tiers: HashMap<String, Vec<String>>,
    /// Extra force-include slugs per title (lowercased), merged with the built-in
    /// allowlist. Empty unless `[tier_allow]` sets it.
    pub tier_allow: HashMap<String, Vec<String>>,
    /// Extra force-exclude slugs per title (lowercased), merged with the built-in
    /// denylist. Empty unless `[tier_deny]` sets it.
    pub tier_deny: HashMap<String, Vec<String>>,
    /// Calendar poll interval for a series with a session live or starting within
    /// the hour (fast tier; only the in-window series uses it).
    pub ocblacktop_live_poll: Duration,
    /// Calendar poll interval for a series with an event within ~2 weeks (medium).
    pub ocblacktop_near_poll: Duration,
    /// Calendar poll interval for a series with nothing close — off-season /
    /// between rounds (slow passive tier).
    pub ocblacktop_idle_poll: Duration,
    /// Standings poll interval — a daily floor; a just-finished round also
    /// refreshes them sooner (they only change post-round).
    pub ocblacktop_standings_poll: Duration,
    /// Hard per-UTC-day cap on Orange Cat Blacktop requests (a backstop under the
    /// free tier's 250/day; the poll intervals normally keep usage well below it).
    pub ocblacktop_daily_cap: u64,
    /// Timezone for formatting times and grouping by day.
    pub tz: Tz,
    /// Poll interval when nothing is live or about to start. Schedules change
    /// slowly, so this can be generous.
    pub idle_poll: Duration,
    /// Poll interval while a match is live or starts soon, to catch final
    /// scores/status promptly.
    pub active_poll: Duration,
    /// Days ahead shown on the homepage "upcoming" view.
    pub upcoming_days: i64,
    /// How long before a match starts to fire its reminder (milliseconds).
    pub reminder_lead_ms: i64,
    /// How many months of past matches to keep + backfill (1-3; free-tier
    /// history is shallow, so this is capped low).
    pub archive_months: i64,
    /// Days to keep a persisted box score after its last fetch before it's pruned
    /// from result_cache (0 disables pruning).
    pub box_score_retention_days: i64,
    /// Days to keep persisted TFT enrichment (standings, lobbies, streamers, …)
    /// after its last refresh before it's pruned from result_cache (0 disables).
    pub tft_retention_days: i64,
    /// Periodically re-fetch recent past days to catch late score corrections.
    pub past_refresh: bool,
    /// Slowly backfill older past matches up to `archive_months` on a fresh DB.
    pub backfill: bool,
    /// Skip past-refresh/backfill when the API's remaining hourly budget is
    /// below this, so current/future polling always has headroom.
    pub rate_limit_floor: u64,
    /// Path to the SQLite cache file. Empty string disables persistence.
    pub db_path: String,
    /// Force demo/fixture data, ignoring the token and any cached DB data.
    pub demo: bool,
    /// Resolve exact event pages via Liquipedia (default on).
    pub resolve_links: bool,
    /// VAPID keys for Web Push (base64url public/private + a contact subject).
    /// Push is enabled only when all three are set.
    pub vapid_public: String,
    pub vapid_private: String,
    pub vapid_subject: String,
    /// Footer copyright line (optional).
    pub copyright: Option<String>,
    /// Optional URL the copyright line links to (otherwise plain text).
    pub copyright_url: Option<String>,
    /// Footer links (GitHub, social, …).
    pub links: Vec<SiteLink>,
    /// Directory the server reads optional site-icon / PWA files from at runtime
    /// (favicon.ico, favicon.svg, apple-touch-icon.png, icon-192/512[-maskable].png,
    /// manifest.webmanifest). Missing files are simply not served or linked.
    pub icons_dir: String,
}

impl Config {
    /// True when Web Push is fully configured.
    #[must_use]
    pub fn push_enabled(&self) -> bool {
        !self.vapid_public.is_empty()
            && !self.vapid_private.is_empty()
            && !self.vapid_subject.is_empty()
    }

    /// The oldest match start to keep/backfill: `now - archive_months` (calendar
    /// months, so month-length differences are handled correctly).
    #[must_use]
    pub fn archive_cutoff(&self, now: DateTime<Utc>) -> DateTime<Utc> {
        now.checked_sub_months(Months::new(self.archive_months as u32))
            .unwrap_or(now)
    }
}

/// Fallback tier set for a slug with no `[tiers]` entry, and the seed for
/// lol/cs2: PandaScore's S and A tiers, matching the pre-config default.
static DEFAULT_TIERS: Lazy<Vec<String>> = Lazy::new(|| vec!["s".to_string(), "a".to_string()]);

/// Empty override list — the default for a sport with no `[tier_allow]` /
/// `[tier_deny]` entry (the built-in tiering consts still apply).
const EMPTY_SLUGS: &[String] = &[];

/// Lowercase a `slug → values` config table (keys and values), keeping only
/// entries whose key is a PandaScore title (lol/cs2). Used for `[tiers]`,
/// `[tier_allow]`, and `[tier_deny]` — the only sports that are tier-filtered.
fn lower_slug_map(m: HashMap<String, Vec<String>>) -> HashMap<String, Vec<String>> {
    m.into_iter()
        .filter_map(|(slug, list)| {
            let key = slug.to_ascii_lowercase();
            Sport::from_filter(&key)
                .is_some_and(Sport::pandascore)
                .then(|| {
                    let vals = list.iter().map(|s| s.trim().to_ascii_lowercase()).collect();
                    (key, vals)
                })
        })
        .collect()
}

/// The process-wide config, loaded once (reads `config.toml` on first access).
static CACHE: Lazy<Config> = Lazy::new(Config::load);

/// Borrow the loaded config without cloning — for the request hot paths that
/// only read a few (mostly `Copy`) fields.
#[must_use]
pub fn config() -> &'static Config {
    &CACHE
}

impl Config {
    /// The loaded config, cloned. Prefer [`config`] on hot paths.
    #[must_use]
    pub fn from_env() -> Self {
        CACHE.clone()
    }

    /// Load from `config.toml` (path overridable via `CONFIG_PATH`); any matching
    /// env var wins, so `DEMO=1 cargo leptos watch` and container `-e` overrides
    /// still work.
    fn load() -> Self {
        let path = std::env::var("CONFIG_PATH").unwrap_or_else(|_| "config.toml".to_string());
        let file: FileConfig = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| match toml::from_str::<FileConfig>(&s) {
                Ok(f) => Some(f),
                Err(e) => {
                    eprintln!("config: failed to parse {path}: {e}");
                    None
                }
            })
            .unwrap_or_default();
        let map = file.to_map();
        let mut cfg = Self::from_vars(|k| {
            std::env::var(k)
                .ok()
                .filter(|s| !s.trim().is_empty())
                .or_else(|| map.get(k).cloned())
        });
        // Structured (non-string-flat) settings apply directly.
        Self::patch_from_file(&mut cfg, file);
        cfg
    }

    /// Apply a parsed file's structured tables (site / co-streamers / discovery)
    /// onto a base config built from env/flat vars.
    fn patch_from_file(cfg: &mut Config, file: FileConfig) {
        if let Some(site) = file.site {
            cfg.copyright = site.copyright;
            cfg.copyright_url = site.copyright_url;
            cfg.links = site.links;
        }
        if let Some(cs) = file.costreamers {
            cfg.costreamers = cs;
        }
        if let Some(yc) = file.youtube_costreamers {
            cfg.youtube_costreamers = yc;
        }
        if let Some(sc) = file.soop_costreamers {
            cfg.soop_costreamers = sc;
        }
        if let Some(td) = file.twitch_discovery {
            cfg.twitch_discovery = TwitchDiscovery {
                enabled_sports: td.enabled_sports.unwrap_or_default(),
                languages: td
                    .languages
                    .filter(|v| !v.is_empty())
                    .unwrap_or_else(|| vec!["en".to_string()]),
                min_viewers: td.min_viewers.unwrap_or(300),
                max_per_match: td.max_per_match.unwrap_or(6),
                game_ids: td.game_ids.unwrap_or_default(),
                gql_costreamers: td.gql_costreamers.unwrap_or(false),
            };
        }
        if let Some(al) = file.twitch_league_aliases {
            cfg.twitch_league_aliases = al;
        }
        if let Some(pins) = file.competetft_pins {
            cfg.competetft_pins = pins;
        }
        if let Some(exclude) = file.competetft_exclude {
            cfg.competetft_exclude = exclude;
        }
        if let Some(tiers) = file.tiers {
            // Per-key override so a title absent from the file keeps its seed.
            for (slug, list) in lower_slug_map(tiers) {
                cfg.tiers.insert(slug, list);
            }
        }
        if let Some(allow) = file.tier_allow {
            cfg.tier_allow = lower_slug_map(allow);
        }
        if let Some(deny) = file.tier_deny {
            cfg.tier_deny = lower_slug_map(deny);
        }
    }

    /// True when Twitch discovery is enabled for `sport`.
    #[must_use]
    pub fn discovery_enabled(&self, sport: crate::types::Sport) -> bool {
        self.twitch_discovery
            .enabled_sports
            .iter()
            .any(|s| s == sport.slug())
    }

    /// Configured PandaScore tiers to show for `sport` (lowercased). Defaults to
    /// ["s","a"] for a title with no `[tiers]` entry. Only meaningful for
    /// PandaScore titles; never consulted for other sports.
    #[must_use]
    pub fn tiers_for(&self, sport: Sport) -> &[String] {
        self.tiers
            .get(sport.slug())
            .map_or(DEFAULT_TIERS.as_slice(), Vec::as_slice)
    }

    /// Extra force-include slugs for `sport` (merged with the built-in allowlist).
    #[must_use]
    pub fn allow_for(&self, sport: Sport) -> &[String] {
        self.tier_allow
            .get(sport.slug())
            .map_or(EMPTY_SLUGS, Vec::as_slice)
    }

    /// Extra force-exclude slugs for `sport` (merged with the built-in denylist).
    #[must_use]
    pub fn deny_for(&self, sport: Sport) -> &[String] {
        self.tier_deny
            .get(sport.slug())
            .map_or(EMPTY_SLUGS, Vec::as_slice)
    }

    /// Test helper: build a config from a parsed file only (no env), applying the
    /// same structured-table patches as `load`.
    #[cfg(test)]
    fn from_file_for_test(file: FileConfig) -> Self {
        let mut cfg = Config::from_vars(|_| None);
        Config::patch_from_file(&mut cfg, file);
        cfg
    }

    /// Build from an arbitrary key→value lookup. Keeps the parsing/clamping
    /// logic pure and testable, separate from the config source.
    pub(crate) fn from_vars(get: impl Fn(&str) -> Option<String>) -> Self {
        // Seconds value, clamped to at least `min`, else `default`.
        let secs = |key: &str, default: u64, min: u64| -> u64 {
            get(key)
                .and_then(|s| s.parse().ok())
                .filter(|&n| n >= min)
                .unwrap_or(default)
        };

        let token = get("PANDASCORE_TOKEN")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let ocblacktop_token = get("OCBLACKTOP_TOKEN")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let twitch_client_id = get("TWITCH_CLIENT_ID")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let twitch_client_secret = get("TWITCH_CLIENT_SECRET")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let youtube_api_key = get("YOUTUBE_API_KEY")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        // Per-series, proximity-driven cadence sized for the free tier's 250
        // req/day across WRC + WEC + MotoGP. Each series is polled only as fast as
        // its nearest event warrants: off-season at the slow `idle` tier (a
        // ~2-day tick just to catch schedule changes), ramping to `near` within
        // ~2 weeks of an event, and `live` only while a session of THAT series is
        // running or imminent. So a quiet day costs a handful of requests and even
        // a race day stays well under the cap — versus a flat hourly poll of all
        // three, which burned ~120-144/day around the clock. Standings change only
        // post-round, so they sit on a multi-day floor and refresh right after a
        // round via the edge-trigger. The idle + standings cadences were lengthened
        // (24h → 48h / 72h) once results + standings became DB-persisted — a
        // restart now reloads them from the cache rather than re-polling, so the
        // periodic floor only needs to catch genuine between-poll changes.
        let ocblacktop_live_poll = Duration::from_secs(secs("OCBLACKTOP_LIVE_POLL_SECS", 300, 60));
        let ocblacktop_near_poll =
            Duration::from_secs(secs("OCBLACKTOP_NEAR_POLL_SECS", 10800, 300));
        let ocblacktop_idle_poll =
            Duration::from_secs(secs("OCBLACKTOP_IDLE_POLL_SECS", 172800, 3600));
        let ocblacktop_standings_poll =
            Duration::from_secs(secs("OCBLACKTOP_STANDINGS_POLL_SECS", 259200, 3600));
        let ocblacktop_daily_cap = secs("OCBLACKTOP_DAILY_CAP", 240, 0);

        let tz = get("DISPLAY_TZ")
            .and_then(|s| s.parse::<Tz>().ok())
            .unwrap_or(chrono_tz::America::Los_Angeles);

        // Idle base (default 20 min); active burst when live/imminent (1 min).
        let idle_poll = Duration::from_secs(secs("POLL_INTERVAL_SECS", 1200, 60));
        let active_poll = Duration::from_secs(secs("POLL_ACTIVE_SECS", 60, 30));

        let upcoming_days = get("UPCOMING_DAYS")
            .and_then(|s| s.parse().ok())
            .filter(|&n| (1..=60).contains(&n))
            .unwrap_or(30);

        // Reminder lead time in minutes (default 15), clamped to a sane range.
        let reminder_lead_ms = get("REMINDER_LEAD_MINUTES")
            .and_then(|s| s.parse::<i64>().ok())
            .filter(|&n| (0..=1440).contains(&n))
            .unwrap_or(15)
            * 60_000;

        // Months of history to keep + backfill (default 1, capped at 3 — the
        // free tier's history window is shallow).
        let archive_months = get("ARCHIVE_MONTHS")
            .and_then(|s| s.parse().ok())
            .filter(|&n| (1..=3).contains(&n))
            .unwrap_or(1);

        // Days to keep a persisted box score after its last fetch/view before
        // pruning it (default 540 — comfortably over a year, to clear any single-
        // season edge cases; 0 disables). result_cache's box_score namespace is
        // id-keyed, so without this it grows one row per game ever viewed.
        let box_score_retention_days = get("BOX_SCORE_RETENTION_DAYS")
            .and_then(|s| s.parse().ok())
            .filter(|&n| (0..=3650).contains(&n))
            .unwrap_or(540);

        // Days to keep persisted TFT enrichment after its last refresh. Same shape
        // and default as the box-score knob above: result_cache's tft_* namespaces
        // are event-name-keyed, so without this they grow one row per namespace per
        // tournament ever polled, and every one of them is read back at boot.
        let tft_retention_days = get("TFT_RETENTION_DAYS")
            .and_then(|s| s.parse().ok())
            .filter(|&n| (0..=3650).contains(&n))
            .unwrap_or(540);

        // Both default on; disabled with the usual falsey values.
        let flag = |key: &str, default: bool| -> bool {
            get(key)
                .map(|v| !matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "false" | "no"))
                .unwrap_or(default)
        };
        let past_refresh = flag("ENABLE_PAST_REFRESH", true);
        let backfill = flag("ENABLE_BACKFILL", true);
        let soop_enabled = flag("SOOP_ENABLED", false);
        let competetft_enabled = flag("COMPETETFT_ENABLED", false);
        let competetft_autodiscover = flag("COMPETETFT_AUTODISCOVER", true);
        let rate_limit_floor = secs("RATE_LIMIT_FLOOR", 200, 0);

        let db_path = get("DB_PATH").unwrap_or_else(|| "data/cache.db".to_string());

        let demo = get("DEMO").is_some_and(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        });

        let resolve_links = get("ENABLE_LIQUIPEDIA")
            .map(|v| !matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "false" | "no"))
            .unwrap_or(true);

        let trimmed = |k: &str| get(k).map(|s| s.trim().to_string()).unwrap_or_default();
        let vapid_public = trimmed("VAPID_PUBLIC_KEY");
        let vapid_private = trimmed("VAPID_PRIVATE_KEY");
        let vapid_subject = trimmed("VAPID_SUBJECT");

        let icons_dir = get("ICONS_DIR")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "icons".to_string());

        // Seed the PandaScore titles with the S+A default; `[tiers]` overrides
        // per-key in patch_from_file. Overrides start empty (built-ins apply).
        let mut tiers = HashMap::new();
        tiers.insert("lol".to_string(), DEFAULT_TIERS.clone());
        tiers.insert("cs2".to_string(), DEFAULT_TIERS.clone());

        Self {
            token,
            ocblacktop_token,
            twitch_client_id,
            twitch_client_secret,
            costreamers: HashMap::new(),
            youtube_api_key,
            youtube_costreamers: HashMap::new(),
            soop_enabled,
            soop_costreamers: HashMap::new(),
            competetft_enabled,
            competetft_autodiscover,
            competetft_pins: Vec::new(),
            competetft_exclude: Vec::new(),
            ocblacktop_live_poll,
            ocblacktop_near_poll,
            ocblacktop_idle_poll,
            ocblacktop_standings_poll,
            ocblacktop_daily_cap,
            tz,
            idle_poll,
            active_poll,
            upcoming_days,
            reminder_lead_ms,
            archive_months,
            box_score_retention_days,
            tft_retention_days,
            past_refresh,
            backfill,
            rate_limit_floor,
            db_path,
            demo,
            resolve_links,
            vapid_public,
            vapid_private,
            vapid_subject,
            copyright: None,
            copyright_url: None,
            links: Vec::new(),
            twitch_discovery: TwitchDiscovery::default(),
            twitch_league_aliases: HashMap::new(),
            tiers,
            tier_allow: HashMap::new(),
            tier_deny: HashMap::new(),
            icons_dir,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn cfg(pairs: &[(&str, &str)]) -> Config {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect();
        Config::from_vars(move |k| map.get(k).cloned())
    }

    #[test]
    fn competetft_defaults_and_flags() {
        let c = cfg(&[]);
        assert!(!c.competetft_enabled);
        assert!(c.competetft_autodiscover);
        assert!(c.competetft_pins.is_empty());
        assert!(c.competetft_exclude.is_empty());
        let c = cfg(&[
            ("COMPETETFT_ENABLED", "true"),
            ("COMPETETFT_AUTODISCOVER", "false"),
        ]);
        assert!(c.competetft_enabled);
        assert!(!c.competetft_autodiscover);
    }

    #[test]
    fn defaults_when_unset() {
        let c = cfg(&[]);
        assert!(c.token.is_none());
        assert!(c.ocblacktop_token.is_none());
        assert_eq!(c.ocblacktop_live_poll.as_secs(), 300);
        assert_eq!(c.ocblacktop_near_poll.as_secs(), 10800);
        assert_eq!(c.ocblacktop_idle_poll.as_secs(), 172800);
        assert_eq!(c.ocblacktop_standings_poll.as_secs(), 259200);
        assert_eq!(c.ocblacktop_daily_cap, 240);
        assert_eq!(c.tz, chrono_tz::America::Los_Angeles);
        assert_eq!(c.idle_poll.as_secs(), 1200);
        assert_eq!(c.active_poll.as_secs(), 60);
        assert_eq!(c.upcoming_days, 30);
        assert_eq!(c.reminder_lead_ms, 15 * 60_000);
        assert_eq!(c.archive_months, 1);
        assert_eq!(c.box_score_retention_days, 540);
        assert_eq!(c.tft_retention_days, 540);
        assert!(c.past_refresh);
        assert!(c.backfill);
        assert_eq!(c.rate_limit_floor, 200);
        assert_eq!(c.db_path, "data/cache.db");
        assert!(c.resolve_links);
        assert!(!c.demo);
    }

    #[test]
    fn tiers_default_and_override() {
        let c = cfg(&[]);
        assert_eq!(
            c.tiers_for(crate::types::Sport::Lol).to_vec(),
            vec!["s", "a"]
        );
        assert_eq!(
            c.tiers_for(crate::types::Sport::Cs2).to_vec(),
            vec!["s", "a"]
        );
        // Non-PandaScore sport falls back to the same default (never consulted).
        assert_eq!(
            c.tiers_for(crate::types::Sport::Mlb).to_vec(),
            vec!["s", "a"]
        );
        // No overrides by default.
        assert!(c.allow_for(crate::types::Sport::Lol).is_empty());
        assert!(c.deny_for(crate::types::Sport::Cs2).is_empty());
    }

    #[test]
    fn tier_tables_parse_lowercase_and_ignore_non_pandascore() {
        let src = r#"
            [tiers]
            lol = ["S", "A", "B"]
            mlb = ["s"]
            [tier_allow]
            lol = ["League-of-Legends-First-Stand"]
            [tier_deny]
            cs2 = ["Showmatch-Cup"]
            nba = ["nope"]
        "#;
        let fc: FileConfig = toml::from_str(src).unwrap();
        let c = Config::from_file_for_test(fc);
        assert_eq!(
            c.tiers_for(crate::types::Sport::Lol).to_vec(),
            vec!["s", "a", "b"]
        );
        // cs2 tiers untouched by the file → default.
        assert_eq!(
            c.tiers_for(crate::types::Sport::Cs2).to_vec(),
            vec!["s", "a"]
        );
        // mlb tier key ignored (not a PandaScore title).
        assert_eq!(
            c.tiers_for(crate::types::Sport::Mlb).to_vec(),
            vec!["s", "a"]
        );
        // Overrides lowercased; non-PandaScore keys dropped.
        assert_eq!(
            c.allow_for(crate::types::Sport::Lol).to_vec(),
            vec!["league-of-legends-first-stand"]
        );
        assert_eq!(
            c.deny_for(crate::types::Sport::Cs2).to_vec(),
            vec!["showmatch-cup"]
        );
        assert!(c.deny_for(crate::types::Sport::Nba).is_empty());
    }

    #[test]
    fn archive_and_polling_knobs_parse_and_clamp() {
        assert_eq!(cfg(&[("ARCHIVE_MONTHS", "3")]).archive_months, 3);
        // Out of range (free-tier wall) → default 1.
        assert_eq!(cfg(&[("ARCHIVE_MONTHS", "12")]).archive_months, 1);
        assert_eq!(cfg(&[("ARCHIVE_MONTHS", "0")]).archive_months, 1);
        // Box-score retention: a valid value passes; 0 disables; out of range → 360.
        assert_eq!(
            cfg(&[("BOX_SCORE_RETENTION_DAYS", "90")]).box_score_retention_days,
            90
        );
        assert_eq!(
            cfg(&[("BOX_SCORE_RETENTION_DAYS", "0")]).box_score_retention_days,
            0
        );
        assert_eq!(
            cfg(&[("BOX_SCORE_RETENTION_DAYS", "99999")]).box_score_retention_days,
            540
        );
        // TFT enrichment retention: same shape and default as the box-score knob,
        // since it bounds the same class of data (per-event history, keyed by name).
        assert_eq!(cfg(&[("TFT_RETENTION_DAYS", "90")]).tft_retention_days, 90);
        assert_eq!(cfg(&[("TFT_RETENTION_DAYS", "0")]).tft_retention_days, 0);
        assert_eq!(
            cfg(&[("TFT_RETENTION_DAYS", "99999")]).tft_retention_days,
            540
        );
        assert!(!cfg(&[("ENABLE_PAST_REFRESH", "false")]).past_refresh);
        assert!(!cfg(&[("ENABLE_BACKFILL", "0")]).backfill);
        assert_eq!(cfg(&[("RATE_LIMIT_FLOOR", "500")]).rate_limit_floor, 500);
        // Orange Cat Blacktop cadence tiers parse, and clamp below their floors.
        assert_eq!(
            cfg(&[("OCBLACKTOP_LIVE_POLL_SECS", "120")])
                .ocblacktop_live_poll
                .as_secs(),
            120
        );
        assert_eq!(
            cfg(&[("OCBLACKTOP_LIVE_POLL_SECS", "5")])
                .ocblacktop_live_poll
                .as_secs(),
            300 // below min 60 -> default
        );
        assert_eq!(
            cfg(&[("OCBLACKTOP_NEAR_POLL_SECS", "7200")])
                .ocblacktop_near_poll
                .as_secs(),
            7200
        );
        assert_eq!(
            cfg(&[("OCBLACKTOP_IDLE_POLL_SECS", "43200")])
                .ocblacktop_idle_poll
                .as_secs(),
            43200
        );
        assert_eq!(
            cfg(&[("OCBLACKTOP_STANDINGS_POLL_SECS", "100")])
                .ocblacktop_standings_poll
                .as_secs(),
            259200 // below min 3600 -> default
        );
    }

    #[test]
    fn archive_cutoff_subtracts_calendar_months() {
        let now = chrono::TimeZone::with_ymd_and_hms(&Utc, 2026, 6, 21, 12, 0, 0).unwrap();
        let c = cfg(&[("ARCHIVE_MONTHS", "1")]);
        assert_eq!(
            c.archive_cutoff(now),
            chrono::TimeZone::with_ymd_and_hms(&Utc, 2026, 5, 21, 12, 0, 0).unwrap()
        );
    }

    #[test]
    fn reminder_lead_parses_and_clamps() {
        assert_eq!(
            cfg(&[("REMINDER_LEAD_MINUTES", "30")]).reminder_lead_ms,
            30 * 60_000
        );
        assert_eq!(cfg(&[("REMINDER_LEAD_MINUTES", "0")]).reminder_lead_ms, 0);
        // Out of range / unparseable fall back to the 15-minute default.
        assert_eq!(
            cfg(&[("REMINDER_LEAD_MINUTES", "9999")]).reminder_lead_ms,
            15 * 60_000
        );
        assert_eq!(
            cfg(&[("REMINDER_LEAD_MINUTES", "x")]).reminder_lead_ms,
            15 * 60_000
        );
    }

    #[test]
    fn demo_flag() {
        assert!(cfg(&[("DEMO", "1")]).demo);
        assert!(cfg(&[("DEMO", "true")]).demo);
        assert!(!cfg(&[("DEMO", "0")]).demo);
        assert!(!cfg(&[]).demo);
    }

    #[test]
    fn liquipedia_can_be_disabled() {
        assert!(!cfg(&[("ENABLE_LIQUIPEDIA", "false")]).resolve_links);
        assert!(!cfg(&[("ENABLE_LIQUIPEDIA", "0")]).resolve_links);
        assert!(cfg(&[("ENABLE_LIQUIPEDIA", "true")]).resolve_links);
    }

    #[test]
    fn parses_values_and_trims_token() {
        let c = cfg(&[
            ("PANDASCORE_TOKEN", "  abc  "),
            ("OCBLACKTOP_TOKEN", "  ocb-key  "),
            ("DISPLAY_TZ", "Europe/London"),
            ("POLL_ACTIVE_SECS", "120"),
            ("DB_PATH", "/tmp/x.db"),
        ]);
        assert_eq!(c.token.as_deref(), Some("abc"));
        assert_eq!(c.ocblacktop_token.as_deref(), Some("ocb-key"));
        assert_eq!(c.tz, chrono_tz::Europe::London);
        assert_eq!(c.active_poll.as_secs(), 120);
        assert_eq!(c.db_path, "/tmp/x.db");
    }

    #[test]
    fn clamps_and_falls_back_on_bad_values() {
        let c = cfg(&[
            ("POLL_INTERVAL_SECS", "10"), // below min 60 -> default 1200
            ("UPCOMING_DAYS", "999"),     // above max 60 -> default 30
            ("DISPLAY_TZ", "Nope/Nope"),  // invalid -> default
            ("POLL_ACTIVE_SECS", "abc"),  // unparseable -> default 60
        ]);
        assert_eq!(c.idle_poll.as_secs(), 1200);
        assert_eq!(c.upcoming_days, 30);
        assert_eq!(c.tz, chrono_tz::America::Los_Angeles);
        assert_eq!(c.active_poll.as_secs(), 60);
    }

    #[test]
    fn blank_token_is_none() {
        assert!(cfg(&[("PANDASCORE_TOKEN", "   ")]).token.is_none());
    }

    #[test]
    fn icons_dir_defaults_and_overrides() {
        assert_eq!(cfg(&[]).icons_dir, "icons");
        assert_eq!(
            cfg(&[("ICONS_DIR", "/srv/pte/icons")]).icons_dir,
            "/srv/pte/icons"
        );
        // Blank / whitespace falls back to the default.
        assert_eq!(cfg(&[("ICONS_DIR", "  ")]).icons_dir, "icons");
    }

    #[test]
    fn parses_youtube_key_and_costreamers() {
        let src = r#"
            youtube_api_key = "yt-key"
            [youtube_costreamers]
            lol = ["@caedrel", "https://www.youtube.com/@sneaky"]
        "#;
        let fc: FileConfig = toml::from_str(src).unwrap();
        assert_eq!(
            fc.to_map().get("YOUTUBE_API_KEY").map(String::as_str),
            Some("yt-key")
        );
        let yc = fc.youtube_costreamers.expect("youtube_costreamers");
        assert_eq!(yc.get("lol").unwrap().len(), 2);
    }

    #[test]
    fn parses_twitch_creds_and_costreamers() {
        let src = r#"
            twitch_client_id = "cid"
            twitch_client_secret = "csecret"
            [costreamers]
            LCK = ["caedrel", "ludwig"]
            "PGL Major" = ["gaules"]
        "#;
        let fc: FileConfig = toml::from_str(src).unwrap();
        let m = fc.to_map();
        assert_eq!(m.get("TWITCH_CLIENT_ID").map(String::as_str), Some("cid"));
        assert_eq!(
            m.get("TWITCH_CLIENT_SECRET").map(String::as_str),
            Some("csecret")
        );
        let cs = fc.costreamers.expect("costreamers");
        assert_eq!(
            cs.get("LCK").unwrap(),
            &vec!["caedrel".to_string(), "ludwig".to_string()]
        );
        assert_eq!(cs.get("PGL Major").unwrap(), &vec!["gaules".to_string()]);
    }

    #[test]
    fn soop_enabled_defaults_off_and_parses() {
        // Off by default (unofficial endpoint ⇒ opt-in).
        assert!(!cfg(&[]).soop_enabled);
        assert!(cfg(&[("SOOP_ENABLED", "true")]).soop_enabled);
        assert!(!cfg(&[("SOOP_ENABLED", "false")]).soop_enabled);
    }

    #[test]
    fn parses_soop_flag_and_costreamers() {
        let src = r#"
            soop_enabled = true
            [soop_costreamers]
            lol = ["lck", "faker"]
        "#;
        let fc: FileConfig = toml::from_str(src).unwrap();
        assert_eq!(
            fc.to_map().get("SOOP_ENABLED").map(String::as_str),
            Some("true")
        );
        let cfg = Config::from_file_for_test(fc);
        assert_eq!(
            cfg.soop_costreamers.get("lol").unwrap(),
            &vec!["lck".to_string(), "faker".to_string()]
        );
    }

    #[test]
    fn parses_twitch_discovery_and_aliases() {
        let toml = r#"
            [twitch_discovery]
            enabled_sports = ["lol", "cs2"]
            languages = ["en", "ko"]
            min_viewers = 500
            max_per_match = 4
            gql_costreamers = true
            [twitch_discovery.game_ids]
            lol = "21779"
            [twitch_league_aliases]
            "Mid-Season Invitational" = ["MSI"]
        "#;
        let fc: FileConfig = toml::from_str(toml).expect("parse");
        let cfg = Config::from_file_for_test(fc);
        assert_eq!(cfg.twitch_discovery.enabled_sports, vec!["lol", "cs2"]);
        assert_eq!(cfg.twitch_discovery.languages, vec!["en", "ko"]);
        assert_eq!(cfg.twitch_discovery.min_viewers, 500);
        assert_eq!(cfg.twitch_discovery.max_per_match, 4);
        assert!(cfg.twitch_discovery.gql_costreamers);
        assert_eq!(
            cfg.twitch_discovery.game_ids.get("lol").map(String::as_str),
            Some("21779")
        );
        assert_eq!(
            cfg.twitch_league_aliases.get("Mid-Season Invitational"),
            Some(&vec!["MSI".to_string()])
        );
        assert!(cfg.discovery_enabled(crate::types::Sport::Lol));
        assert!(!cfg.discovery_enabled(crate::types::Sport::Mlb));
    }

    #[test]
    fn twitch_discovery_defaults_when_sparse_and_off_when_absent() {
        // Present but sparse: languages default to ["en"], numeric defaults apply.
        let fc: FileConfig =
            toml::from_str("[twitch_discovery]\nenabled_sports = [\"lol\"]\n").unwrap();
        let cfg = Config::from_file_for_test(fc);
        assert_eq!(cfg.twitch_discovery.languages, vec!["en"]);
        assert_eq!(cfg.twitch_discovery.min_viewers, 300);
        assert_eq!(cfg.twitch_discovery.max_per_match, 6);
        assert!(!cfg.twitch_discovery.gql_costreamers);
        // Absent entirely: disabled.
        let empty: FileConfig = toml::from_str("").unwrap();
        let cfg2 = Config::from_file_for_test(empty);
        assert!(cfg2.twitch_discovery.enabled_sports.is_empty());
        assert!(!cfg2.discovery_enabled(crate::types::Sport::Lol));
    }

    #[test]
    fn toml_parses_and_feeds_the_parser() {
        let src = r#"
            pandascore_token = "tok"
            demo = true
            display_tz = "Europe/London"
            upcoming_days = 14
            [vapid]
            public = "pub"
            private = "priv"
            subject = "mailto:x@example.com"
        "#;
        let fc: FileConfig = toml::from_str(src).unwrap();
        let m = fc.to_map();
        assert_eq!(m.get("DEMO").map(String::as_str), Some("true"));
        assert_eq!(m.get("UPCOMING_DAYS").map(String::as_str), Some("14"));
        assert_eq!(m.get("VAPID_PUBLIC_KEY").map(String::as_str), Some("pub"));

        let c = Config::from_vars(|k| m.get(k).cloned());
        assert!(c.demo);
        assert_eq!(c.upcoming_days, 14);
        assert_eq!(c.tz, chrono_tz::Europe::London);
        assert_eq!(c.token.as_deref(), Some("tok"));
        assert!(c.push_enabled());
    }
}
