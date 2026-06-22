//! Runtime configuration, loaded from `config.toml` with env-var overrides
//! (server-only).

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
    db_path: Option<String>,
    resolve_links: Option<bool>,
    vapid: Option<VapidFile>,
}

#[derive(Debug, Default, Deserialize)]
struct VapidFile {
    public: Option<String>,
    private: Option<String>,
    subject: Option<String>,
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
}

impl Config {
    /// True when Web Push is fully configured.
    #[must_use]
    pub fn push_enabled(&self) -> bool {
        !self.vapid_public.is_empty()
            && !self.vapid_private.is_empty()
            && !self.vapid_subject.is_empty()
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
        Self::from_vars(|k| {
            std::env::var(k)
                .ok()
                .filter(|s| !s.trim().is_empty())
                .or_else(|| map.get(k).cloned())
        })
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
            db_path,
            demo,
            resolve_links,
            vapid_public,
            vapid_private,
            vapid_subject,
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
        assert_eq!(c.db_path, "data/cache.db");
        assert!(c.resolve_links);
        assert!(!c.demo);
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
