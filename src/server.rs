//! Leptos server functions. The bodies read the in-memory cache on the server;
//! on the client the `#[server]` macro replaces them with a network call.

use crate::types::{ReminderReq, ScheduleView, SiteInfo, SubscribeReq};
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

/// Footer copyright + links from config.
#[server(GetSite, "/api")]
pub async fn get_site() -> Result<SiteInfo, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        let cfg = crate::config::Config::from_env();
        Ok(SiteInfo {
            copyright: cfg.copyright,
            links: cfg.links,
        })
    }
    #[cfg(not(feature = "ssr"))]
    {
        Ok(SiteInfo::default())
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
            notify_at_ms: req.begin_at_ms - cfg.reminder_lead_ms,
            title: req.title,
            body: req.body,
            url: req.url,
            game: req.game.slug().to_string(),
            league: req.league,
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

/// Subscribe to a whole game ("game"/"cs2"|"lol") or event ("league"/<name>).
#[server(AddSubscription, "/api")]
pub async fn add_subscription(req: SubscribeReq) -> Result<(), ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        let cfg = crate::config::Config::from_env();
        if !cfg.push_enabled() || cfg.db_path.is_empty() {
            return Err(ServerFnError::new("subscriptions are not available"));
        }
        let conn = crate::store::open(&cfg.db_path)
            .map_err(|e| ServerFnError::new(format!("db: {e}")))?;
        let s = crate::store::Subscription {
            endpoint: req.sub.endpoint,
            p256dh: req.sub.p256dh,
            auth: req.sub.auth,
            scope_kind: req.kind,
            scope_value: req.value,
            lead_ms: cfg.reminder_lead_ms,
        };
        crate::store::add_subscription(&conn, &s)
            .map_err(|e| ServerFnError::new(format!("db: {e}")))?;
        Ok(())
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = req;
        Ok(())
    }
}

/// Unsubscribe from a game/event (also drops its pending reminders).
#[server(RemoveSubscription, "/api")]
pub async fn remove_subscription(
    endpoint: String,
    kind: String,
    value: String,
) -> Result<(), ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        let cfg = crate::config::Config::from_env();
        if cfg.db_path.is_empty() {
            return Ok(());
        }
        let conn = crate::store::open(&cfg.db_path)
            .map_err(|e| ServerFnError::new(format!("db: {e}")))?;
        crate::store::remove_subscription(&conn, &endpoint, &kind, &value)
            .map_err(|e| ServerFnError::new(format!("db: {e}")))?;
        crate::store::delete_unsent_reminders_by_scope(&conn, &endpoint, &kind, &value)
            .map_err(|e| ServerFnError::new(format!("db: {e}")))?;
        Ok(())
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = (endpoint, kind, value);
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
