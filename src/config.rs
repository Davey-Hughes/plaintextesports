//! Runtime configuration from environment variables (server-only).

use chrono_tz::Tz;
use std::time::Duration;

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
}

impl Config {
    #[must_use]
    pub fn from_env() -> Self {
        let token = std::env::var("PANDASCORE_TOKEN")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let tz = std::env::var("DISPLAY_TZ")
            .ok()
            .and_then(|s| s.parse::<Tz>().ok())
            .unwrap_or(chrono_tz::America::Los_Angeles);

        // Idle base: schedules barely change, and the free tier has no live
        // feed, so polling fast around the clock buys nothing. Default 20 min.
        let idle_poll = Duration::from_secs(secs_env("POLL_INTERVAL_SECS", 1200, 60));
        // Active burst when a match is live/imminent. Default 1 min.
        let active_poll = Duration::from_secs(secs_env("POLL_ACTIVE_SECS", 60, 30));

        let upcoming_days = std::env::var("UPCOMING_DAYS")
            .ok()
            .and_then(|s| s.parse().ok())
            .filter(|&n| (1..=60).contains(&n))
            .unwrap_or(30);

        let db_path = std::env::var("DB_PATH").unwrap_or_else(|_| "data/cache.db".to_string());

        Self {
            token,
            tz,
            idle_poll,
            active_poll,
            upcoming_days,
            db_path,
        }
    }
}

/// Read a positive seconds env var, clamping to at least `min` and falling back
/// to `default` when unset/unparseable/too small.
fn secs_env(key: &str, default: u64, min: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&n| n >= min)
        .unwrap_or(default)
}
