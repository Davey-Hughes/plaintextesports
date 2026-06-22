//! Leptos server functions. The bodies read the in-memory cache on the server;
//! on the client the `#[server]` macro replaces them with a network call.

use crate::types::{ReminderReq, ScheduleView};
use leptos::prelude::*;

/// Homepage schedule. `game` is "all"/"cs2"/"lol"; `tz` is an IANA name (empty
/// = server default); `hour24` selects 24h vs 12h time formatting.
#[server(GetSchedule, "/api")]
pub async fn get_schedule(
    game: String,
    tz: String,
    hour24: bool,
) -> Result<ScheduleView, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        Ok(crate::cache::homepage_view(&game, &tz, hour24))
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = (game, tz, hour24);
        Ok(ScheduleView::default())
    }
}

/// Single-day schedule for `date` ("YYYY-MM-DD").
#[server(GetDay, "/api")]
pub async fn get_day(
    date: String,
    game: String,
    tz: String,
    hour24: bool,
) -> Result<ScheduleView, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        Ok(crate::cache::day_view(&date, &game, &tz, hour24))
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = (date, game, tz, hour24);
        Ok(ScheduleView::default())
    }
}

/// The VAPID public key (base64url) if Web Push is configured, else `None`.
#[server(GetVapidKey, "/api")]
pub async fn get_vapid_key() -> Result<Option<String>, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        let cfg = crate::config::Config::from_env();
        Ok(cfg.push_enabled().then_some(cfg.vapid_public))
    }
    #[cfg(not(feature = "ssr"))]
    {
        Ok(None)
    }
}

/// Store a reminder for a starred match (a Web Push notification before start).
#[server(AddReminder, "/api")]
pub async fn add_reminder(req: ReminderReq) -> Result<(), ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        let cfg = crate::config::Config::from_env();
        if !cfg.push_enabled() || cfg.db_path.is_empty() {
            return Err(ServerFnError::new("reminders are not available"));
        }
        let conn = crate::store::open(&cfg.db_path)
            .map_err(|e| ServerFnError::new(format!("db: {e}")))?;
        let r = crate::store::Reminder {
            endpoint: req.sub.endpoint,
            p256dh: req.sub.p256dh,
            auth: req.sub.auth,
            match_id: req.match_id,
            notify_at_ms: req.notify_at_ms,
            title: req.title,
            body: req.body,
            url: req.url,
        };
        crate::store::add_reminder(&conn, &r).map_err(|e| ServerFnError::new(format!("db: {e}")))?;
        Ok(())
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = req;
        Ok(())
    }
}

/// Remove a previously-set reminder.
#[server(RemoveReminder, "/api")]
pub async fn remove_reminder(endpoint: String, match_id: i64) -> Result<(), ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        let cfg = crate::config::Config::from_env();
        if cfg.db_path.is_empty() {
            return Ok(());
        }
        let conn = crate::store::open(&cfg.db_path)
            .map_err(|e| ServerFnError::new(format!("db: {e}")))?;
        crate::store::remove_reminder(&conn, &endpoint, match_id)
            .map_err(|e| ServerFnError::new(format!("db: {e}")))?;
        Ok(())
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = (endpoint, match_id);
        Ok(())
    }
}
