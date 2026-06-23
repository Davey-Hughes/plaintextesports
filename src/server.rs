//! Leptos server functions. The bodies read the in-memory cache on the server;
//! on the client the `#[server]` macro replaces them with a network call.

use crate::types::{EventInfo, MatchDetail, ReminderReq, ScheduleView, SiteInfo, SubscribeReq};
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

/// Schedule for an inclusive date range ("YYYY-MM-DD" .. "YYYY-MM-DD"). Backs the
/// "show earlier days" view and the calendar range picker.
#[server(GetRange, "/api")]
pub async fn get_range(
    start: String,
    end: String,
    game: String,
    tz: String,
    hour24: bool,
) -> Result<ScheduleView, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        Ok(crate::cache::range_view(&start, &end, &game, &tz, hour24))
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = (start, end, game, tz, hour24);
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

/// Per-match detail: the match, its broadcasts, and its event's standings/bracket.
#[server(GetMatchDetail, "/api")]
pub async fn get_match_detail(
    id: i64,
    tz: String,
    hour24: bool,
) -> Result<MatchDetail, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        let Some((match_view, streams, tournament_id, league)) =
            crate::cache::match_basics(id, &tz, hour24)
        else {
            return Ok(MatchDetail::default());
        };
        let mut event = match tournament_id {
            Some(tid) => crate::cache::event_info(tid).await,
            None => EventInfo::default(),
        };
        event.event = league;
        Ok(MatchDetail {
            found: true,
            match_view: Some(match_view),
            streams,
            event,
        })
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = (id, tz, hour24);
        Ok(MatchDetail::default())
    }
}

/// All cached matches for one event/league (past + upcoming), for the event page.
#[server(GetEventSchedule, "/api")]
pub async fn get_event_schedule(
    league: String,
    tz: String,
    hour24: bool,
) -> Result<ScheduleView, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        Ok(crate::cache::event_view(&league, &tz, hour24))
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = (league, tz, hour24);
        Ok(ScheduleView::default())
    }
}

/// Standings/bracket for every stage of an event (by league name), in order —
/// e.g. Swiss Stage 1/2/3 then the Playoffs bracket. Empty stages are dropped.
#[server(GetEventStages, "/api")]
pub async fn get_event_stages(league: String) -> Result<Vec<EventInfo>, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        let tids = crate::cache::event_stages(&league);
        Ok(crate::cache::stages_info(tids, &league).await)
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = league;
        Ok(Vec::new())
    }
}

/// Like [`get_event_stages`] but keyed on the (short) league name the front-page
/// event filter selects, so a single-event filter shows every stage's bracket.
#[server(GetEventStagesByLeague, "/api")]
pub async fn get_event_stages_by_league(league: String) -> Result<Vec<EventInfo>, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        let tids = crate::cache::league_stages(&league);
        Ok(crate::cache::stages_info(tids, &league).await)
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = league;
        Ok(Vec::new())
    }
}

/// Standings + bracket for one tournament stage (by league name), kept for the
/// API surface; the single-event filter uses [`get_event_stages_by_league`].
#[server(GetEventInfo, "/api")]
pub async fn get_event_info(league: String) -> Result<EventInfo, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        let Some(tid) = crate::cache::league_tournament(&league) else {
            return Ok(EventInfo::default());
        };
        let mut event = crate::cache::event_info(tid).await;
        event.event = league;
        Ok(event)
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = league;
        Ok(EventInfo::default())
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
        // Derive time/title/body/url from the snapshot so a client can't forge an
        // arbitrary notification (it only supplies its push subscription + a match id).
        let seed = crate::cache::reminder_seed_for_match(req.match_id, cfg.reminder_lead_ms)
            .ok_or_else(|| ServerFnError::new("match not found or already started"))?;
        let conn = crate::store::shared(&cfg.db_path)
            .map_err(|e| ServerFnError::new(format!("db: {e}")))?;
        let r = crate::store::Reminder {
            endpoint: req.sub.endpoint,
            p256dh: req.sub.p256dh,
            auth: req.sub.auth,
            match_id: seed.match_id,
            notify_at_ms: seed.notify_at_ms,
            title: seed.title,
            body: seed.body,
            url: seed.url,
            game: seed.game,
            league: seed.league,
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
        let conn = crate::store::shared(&cfg.db_path)
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
        let conn = crate::store::shared(&cfg.db_path)
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
        let conn = crate::store::shared(&cfg.db_path)
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
