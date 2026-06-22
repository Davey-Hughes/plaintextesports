//! Runtime configuration, loaded from `config.toml` with env-var overrides
//! (server-only).

use crate::types::SiteLink;
use chrono::{DateTime, Months, Utc};
use chrono_tz::Tz;
use once_cell::sync::Lazy;
use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;

/// `config.toml` shape. Everything is optional; missing values fall back to
/// defaults (or a matching env var, which takes precedence).
#[derive(Debug, Default, Deserialize)]
struct FileConfig {
    pandascore_token: Option<String>,
    demo: Option<bool>,
    display_tz: Option<String>,
    upcoming_days: Option<i64>,
    idle_poll_secs: Option<u64>,
    active_poll_secs: Option<u64>,
    reminder_lead_minutes: Option<i64>,
    archive_months: Option<i64>,
    past_refresh: Option<bool>,
    backfill: Option<bool>,
    rate_limit_floor: Option<u64>,
    db_path: Option<String>,
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
        put("DEMO", self.demo.map(|b| b.to_string()));
        put("DISPLAY_TZ", self.display_tz.clone());
        put("UPCOMING_DAYS", self.upcoming_days.map(|n| n.to_string()));
        put("POLL_INTERVAL_SECS", self.idle_poll_secs.map(|n| n.to_string()));
        put("POLL_ACTIVE_SECS", self.active_poll_secs.map(|n| n.to_string()));
        put("REMINDER_LEAD_MINUTES", self.reminder_lead_minutes.map(|n| n.to_string()));
        put("ARCHIVE_MONTHS", self.archive_months.map(|n| n.to_string()));
        put("ENABLE_PAST_REFRESH", self.past_refresh.map(|b| b.to_string()));
        put("ENABLE_BACKFILL", self.backfill.map(|b| b.to_string()));
        put("RATE_LIMIT_FLOOR", self.rate_limit_floor.map(|n| n.to_string()));
        put("DB_PATH", self.db_path.clone());
        put("ENABLE_LIQUIPEDIA", self.resolve_links.map(|b| b.to_string()));
        if let Some(v) = &self.vapid {
            put("VAPID_PUBLIC_KEY", v.public.clone());
            put("VAPID_PRIVATE_KEY", v.private.clone());
            put("VAPID_SUBJECT", v.subject.clone());
        }
        m
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    /// PandaScore API token. `None` => serve demo fixture data.
    pub token: Option<String>,
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
    /// Footer links (GitHub, social, …).
    pub links: Vec<SiteLink>,
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

impl Config {
    /// Load once (reads `config.toml` on first access) and reuse.
    #[must_use]
    pub fn from_env() -> Self {
        static CACHE: Lazy<Config> = Lazy::new(Config::load);
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
        // Site/footer settings aren't string-flat, so apply them directly.
        if let Some(site) = file.site {
            cfg.copyright = site.copyright;
            cfg.links = site.links;
        }
        cfg
    }

    /// Build from an arbitrary key→value lookup. Keeps the parsing/clamping
    /// logic pure and testable, separate from the config source.
    fn from_vars(get: impl Fn(&str) -> Option<String>) -> Self {
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

        // Both default on; disabled with the usual falsey values.
        let flag = |key: &str, default: bool| -> bool {
            get(key)
                .map(|v| !matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "false" | "no"))
                .unwrap_or(default)
        };
        let past_refresh = flag("ENABLE_PAST_REFRESH", true);
        let backfill = flag("ENABLE_BACKFILL", true);
        let rate_limit_floor = secs("RATE_LIMIT_FLOOR", 200, 0);

        let db_path = get("DB_PATH").unwrap_or_else(|| "data/cache.db".to_string());

        let demo = get("DEMO")
            .is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"));

        let resolve_links = get("ENABLE_LIQUIPEDIA")
            .map(|v| !matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "false" | "no"))
            .unwrap_or(true);

        let trimmed = |k: &str| get(k).map(|s| s.trim().to_string()).unwrap_or_default();
        let vapid_public = trimmed("VAPID_PUBLIC_KEY");
        let vapid_private = trimmed("VAPID_PRIVATE_KEY");
        let vapid_subject = trimmed("VAPID_SUBJECT");

        Self {
            token,
            tz,
            idle_poll,
            active_poll,
            upcoming_days,
            reminder_lead_ms,
            archive_months,
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
            links: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn cfg(pairs: &[(&str, &str)]) -> Config {
        let map: HashMap<String, String> =
            pairs.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect();
        Config::from_vars(move |k| map.get(k).cloned())
    }

    #[test]
    fn defaults_when_unset() {
        let c = cfg(&[]);
        assert!(c.token.is_none());
        assert_eq!(c.tz, chrono_tz::America::Los_Angeles);
        assert_eq!(c.idle_poll.as_secs(), 1200);
        assert_eq!(c.active_poll.as_secs(), 60);
        assert_eq!(c.upcoming_days, 30);
        assert_eq!(c.reminder_lead_ms, 15 * 60_000);
        assert_eq!(c.archive_months, 1);
        assert!(c.past_refresh);
        assert!(c.backfill);
        assert_eq!(c.rate_limit_floor, 200);
        assert_eq!(c.db_path, "data/cache.db");
        assert!(c.resolve_links);
        assert!(!c.demo);
    }

    #[test]
    fn archive_and_polling_knobs_parse_and_clamp() {
        assert_eq!(cfg(&[("ARCHIVE_MONTHS", "3")]).archive_months, 3);
        // Out of range (free-tier wall) → default 1.
        assert_eq!(cfg(&[("ARCHIVE_MONTHS", "12")]).archive_months, 1);
        assert_eq!(cfg(&[("ARCHIVE_MONTHS", "0")]).archive_months, 1);
        assert!(!cfg(&[("ENABLE_PAST_REFRESH", "false")]).past_refresh);
        assert!(!cfg(&[("ENABLE_BACKFILL", "0")]).backfill);
        assert_eq!(cfg(&[("RATE_LIMIT_FLOOR", "500")]).rate_limit_floor, 500);
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
        assert_eq!(cfg(&[("REMINDER_LEAD_MINUTES", "30")]).reminder_lead_ms, 30 * 60_000);
        assert_eq!(cfg(&[("REMINDER_LEAD_MINUTES", "0")]).reminder_lead_ms, 0);
        // Out of range / unparseable fall back to the 15-minute default.
        assert_eq!(cfg(&[("REMINDER_LEAD_MINUTES", "9999")]).reminder_lead_ms, 15 * 60_000);
        assert_eq!(cfg(&[("REMINDER_LEAD_MINUTES", "x")]).reminder_lead_ms, 15 * 60_000);
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
            ("DISPLAY_TZ", "Europe/London"),
            ("POLL_ACTIVE_SECS", "120"),
            ("DB_PATH", "/tmp/x.db"),
        ]);
        assert_eq!(c.token.as_deref(), Some("abc"));
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
