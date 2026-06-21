//! Runtime configuration from environment variables (server-only).

use chrono_tz::Tz;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Config {
    /// PandaScore API token. `None` => serve demo fixture data.
    pub token: Option<String>,
    /// Timezone for formatting times and grouping by day.
    pub tz: Tz,
    /// How often to poll PandaScore.
    pub poll_interval: Duration,
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

        let poll_interval = Duration::from_secs(
            std::env::var("POLL_INTERVAL_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .filter(|&n| n >= 30)
                .unwrap_or(120),
        );

        let upcoming_days = std::env::var("UPCOMING_DAYS")
            .ok()
            .and_then(|s| s.parse().ok())
            .filter(|&n| n >= 1 && n <= 31)
            .unwrap_or(7);

        let db_path = std::env::var("DB_PATH").unwrap_or_else(|_| "data/cache.db".to_string());

        Self {
            token,
            tz,
            poll_interval,
            upcoming_days,
            db_path,
        }
    }
}
