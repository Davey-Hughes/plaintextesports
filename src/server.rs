//! Leptos server functions. The bodies read the in-memory cache on the server;
//! on the client the `#[server]` macro replaces them with a network call.

use crate::types::{
    EventInfo, F1Result, F1Standings, MatchDetail, MatchView, ReminderReq, ScheduleView, SiteInfo,
    SubscribeReq,
};
#[cfg(feature = "ssr")]
use crate::types::Game;
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
            Some(tid) => crate::cache::event_info(tid, match_view.game).await,
            None => EventInfo::default(),
        };
        event.event = league;
        // MLB matches have no per-tournament event; show the two teams' division
        // standings instead, plus the series between them (fetched on demand,
        // request-time, TTL-cached so it doesn't hammer the MLB API).
        let (stages, series) = if match_view.game == Game::Mlb {
            (
                crate::cache::mlb_team_divisions(
                    &match_view.team_a.label,
                    &match_view.team_b.label,
                ),
                crate::cache::mlb_series(id, &tz, hour24).await.unwrap_or_default(),
            )
        } else {
            (Vec::new(), crate::types::Series::default())
        };
        Ok(MatchDetail {
            found: true,
            match_view: Some(match_view),
            streams,
            event,
            stages,
            series,
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

/// The given (starred) match ids that are still upcoming, sorted by start, for
/// the notifications page. Started/finished matches are dropped server-side.
#[server(GetNotifications, "/api")]
pub async fn get_notifications(
    ids: Vec<i64>,
    tz: String,
    hour24: bool,
) -> Result<Vec<MatchView>, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        Ok(crate::cache::upcoming_matches_by_ids(&ids, &tz, hour24))
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = (ids, tz, hour24);
        Ok(Vec::new())
    }
}

/// All cached matches involving a team (by full name), past + upcoming, for the
/// team page.
#[server(GetTeamSchedule, "/api")]
pub async fn get_team_schedule(
    team: String,
    tz: String,
    hour24: bool,
) -> Result<ScheduleView, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        Ok(crate::cache::team_view(&team, &tz, hour24))
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = (team, tz, hour24);
        Ok(ScheduleView::default())
    }
}

/// Standings/bracket for every stage of an event (by league name), in order —
/// e.g. Swiss Stage 1/2/3 then the Playoffs bracket. Empty stages are dropped.
#[server(GetEventStages, "/api")]
pub async fn get_event_stages(league: String) -> Result<Vec<EventInfo>, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        // MLB has no per-tournament standings; its event page shows the league's
        // six division tables instead.
        if league == "MLB" {
            return Ok(crate::cache::mlb_standings());
        }
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

/// A Grand Prix's results (full finishing order per finished session) for the F1
/// event page. Keyed by season + round (the client derives them from a session
/// id). Fetched on demand from Jolpica and TTL-cached.
#[server(GetF1Results, "/api")]
pub async fn get_f1_results(season: i64, round: i64) -> Result<Vec<F1Result>, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        Ok(crate::cache::f1_results(season, round).await)
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = (season, round);
        Ok(Vec::new())
    }
}

/// The F1 drivers'/constructors' championship standings as of a GP's round, for
/// the F1 event page. Fetched on demand from Jolpica and TTL-cached.
#[server(GetF1Standings, "/api")]
pub async fn get_f1_standings(season: i64, round: i64) -> Result<F1Standings, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        Ok(crate::cache::f1_standings(season, round).await)
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = (season, round);
        Ok(F1Standings::default())
    }
}

/// Footer copyright + links from config.
#[server(GetSite, "/api")]
pub async fn get_site() -> Result<SiteInfo, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        use chrono::Datelike;
        let cfg = crate::config::Config::from_env();
        // The config holds just the holder name; prepend "© <current year>".
        let year = chrono::Utc::now().year();
        Ok(SiteInfo {
            copyright: cfg.copyright.map(|name| format!("© {year} {name}")),
            copyright_url: cfg.copyright_url,
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
            team_a: seed.team_a,
            team_b: seed.team_b,
            event: seed.event,
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

/// Opt a single match out of a covering game/event/team subscription (a
/// no-notify tombstone) without unsubscribing from the whole scope.
#[server(ExcludeReminder, "/api")]
pub async fn exclude_reminder(req: ReminderReq) -> Result<(), ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        let cfg = crate::config::Config::from_env();
        if !cfg.push_enabled() || cfg.db_path.is_empty() {
            return Err(ServerFnError::new("reminders are not available"));
        }
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
            team_a: seed.team_a,
            team_b: seed.team_b,
            event: seed.event,
        };
        crate::store::exclude_reminder(&conn, &r)
            .map_err(|e| ServerFnError::new(format!("db: {e}")))?;
        Ok(())
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = req;
        Ok(())
    }
}

/// Clear everything this endpoint is signed up for — all subscriptions and
/// pending reminders. Backs the notifications page's "clear all".
#[server(ClearNotifications, "/api")]
pub async fn clear_notifications(endpoint: String) -> Result<(), ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        let cfg = crate::config::Config::from_env();
        if cfg.db_path.is_empty() {
            return Ok(());
        }
        let conn = crate::store::shared(&cfg.db_path)
            .map_err(|e| ServerFnError::new(format!("db: {e}")))?;
        crate::store::clear_endpoint(&conn, &endpoint)
            .map_err(|e| ServerFnError::new(format!("db: {e}")))?;
        Ok(())
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = endpoint;
        Ok(())
    }
}
