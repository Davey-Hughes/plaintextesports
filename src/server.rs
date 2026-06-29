//! Leptos server functions. The bodies read the in-memory cache on the server;
//! on the client the `#[server]` macro replaces them with a network call.

#[cfg(feature = "ssr")]
use crate::types::Sport;
use crate::types::{
    EventInfo, F1Result, F1Standings, MatchDetail, MatchResults, MotorResult, MotorStandings,
    ReminderReq, ScheduleView, SiteInfo, SubscribeReq,
};
use leptos::prelude::*;

/// Homepage schedule. `sport` is "all"/"cs2"/"lol"; `tz` is an IANA name (empty
/// = server default); `hour24` selects 24h vs 12h time formatting.
#[server(GetSchedule, "/api")]
pub async fn get_schedule(
    sport: String,
    tz: String,
    hour24: bool,
) -> Result<ScheduleView, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        Ok(crate::cache::homepage_view(&sport, &tz, hour24))
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = (sport, tz, hour24);
        Ok(ScheduleView::default())
    }
}

/// Schedule for an inclusive date range ("YYYY-MM-DD" .. "YYYY-MM-DD"). Backs the
/// "show earlier days" view and the calendar range picker.
#[server(GetRange, "/api")]
pub async fn get_range(
    start: String,
    end: String,
    sport: String,
    tz: String,
    hour24: bool,
) -> Result<ScheduleView, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        Ok(crate::cache::range_view(&start, &end, &sport, &tz, hour24))
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = (start, end, sport, tz, hour24);
        Ok(ScheduleView::default())
    }
}

/// Single-day schedule for `date` ("YYYY-MM-DD").
#[server(GetDay, "/api")]
pub async fn get_day(
    date: String,
    sport: String,
    tz: String,
    hour24: bool,
) -> Result<ScheduleView, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        Ok(crate::cache::day_view(&date, &sport, &tz, hour24))
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = (date, sport, tz, hour24);
        Ok(ScheduleView::default())
    }
}

/// The heading for a motorsport result: a rally's overall classification, or the
/// stage/session's own label (e.g. "SS4 Stiri 1", "Race") for a single session.
#[cfg(feature = "ssr")]
fn motor_result_title(r: &crate::types::MotorResultRef, label: &str) -> String {
    if r.session_id.is_empty() {
        "Overall classification".to_string()
    } else {
        label.to_string()
    }
}

/// Per-match detail: the match, its broadcasts, and its event's standings/bracket.
#[server(GetMatchDetail, "/api")]
pub async fn get_match_detail(
    uid: String,
    tz: String,
    hour24: bool,
) -> Result<MatchDetail, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        // `uid` is "{sport}-{id}" — ids aren't unique across games.
        let Some((sport, id)) = crate::types::parse_match_uid(&uid) else {
            return Ok(MatchDetail::default());
        };
        let Some((match_view, streams, tournament_id, league, _motor_ref)) =
            crate::cache::match_basics(id, sport, &tz, hour24)
        else {
            return Ok(MatchDetail::default());
        };
        let mut event = match tournament_id {
            Some(tid) => crate::cache::event_info(tid, match_view.sport).await,
            None => EventInfo::default(),
        };
        event.event = league;
        // Traditional matches have no per-tournament event; show the table(s) the
        // two teams sit in (division/conference/group) instead. MLB additionally
        // shows the series between them (fetched on demand, request-time, TTL-
        // cached so it doesn't hammer the MLB API).
        let stages = crate::cache::team_standings_for_match(
            match_view.sport,
            &match_view.league,
            &match_view.team_a.label,
            &match_view.team_a.name,
            &match_view.team_b.label,
            &match_view.team_b.name,
        );
        let series = if match_view.sport == Sport::Mlb {
            crate::cache::mlb_series(id, &tz, hour24)
                .await
                .unwrap_or_default()
        } else {
            crate::types::Series::default()
        };
        // Results (the F1 session order / motorsport classification) are NOT fetched
        // here — they go through `get_match_results` on a separate resource, so the
        // header renders without waiting on a slow (or 500ing) upstream.
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
        let _ = (uid, tz, hour24);
        Ok(MatchDetail::default())
    }
}

/// The match's on-demand results (an F1 session's finishing order, or a WRC/WEC/
/// MotoGP classification), fetched separately from `get_match_detail` so the detail
/// page can show its header/meta before these resolve. `unavailable` flags a
/// finished, result-bearing session whose results came back empty.
#[server(GetMatchResults, "/api")]
pub async fn get_match_results(
    uid: String,
    tz: String,
    hour24: bool,
) -> Result<MatchResults, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        let Some((sport, id)) = crate::types::parse_match_uid(&uid) else {
            return Ok(MatchResults::default());
        };
        let Some((match_view, _streams, _tid, _league, motor_ref)) =
            crate::cache::match_basics(id, sport, &tz, hour24)
        else {
            return Ok(MatchResults::default());
        };
        let finished = matches!(match_view.status, crate::types::MatchStatus::Finished);
        // F1: this session's finishing order. Unavailable only when the whole GP's
        // results are missing (the upstream case) — NOT when this session type just
        // isn't classified (e.g. Sprint Qualifying), which legitimately has none.
        let (f1_result, f1_unavailable) =
            if sport == Sport::Motorsport && match_view.league == "F1" && finished {
                let (season, round) = (id / 100_000, (id / 100) % 1000);
                let all = crate::cache::f1_results(season, round).await;
                let mine = all
                    .iter()
                    .find(|r| r.session == match_view.team_a.label)
                    .cloned();
                let empty = all.is_empty();
                (mine, empty)
            } else {
                (None, false)
            };
        // Motorsport: this row's classification. Unavailable when a finished,
        // result-bearing (has a ref) session returns no rows (upstream 500 / not
        // yet posted).
        let (motor_result, motor_unavailable) = match motor_ref {
            Some(ref r) if finished => {
                let rows = crate::cache::motor_result(r).await;
                if rows.is_empty() {
                    (None, true)
                } else {
                    (
                        Some(crate::types::MotorResult {
                            title: motor_result_title(r, &match_view.team_a.label),
                            rows,
                        }),
                        false,
                    )
                }
            }
            _ => (None, false),
        };
        Ok(MatchResults {
            unavailable: (f1_unavailable || motor_unavailable)
                && f1_result.is_none()
                && motor_result.is_none(),
            f1_result,
            motor_result,
        })
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = (uid, tz, hour24);
        Ok(MatchResults::default())
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

/// The given (starred) match uids that are still upcoming, sorted by start, for
/// the notifications page. Started/finished matches are dropped server-side.
#[server(GetNotifications, "/api")]
pub async fn get_notifications(
    uids: Vec<String>,
    tz: String,
    hour24: bool,
) -> Result<crate::types::NotificationsView, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        Ok(crate::cache::notifications_view(&uids, &tz, hour24))
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = (uids, tz, hour24);
        Ok(crate::types::NotificationsView::default())
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
        // Traditional sports have no per-tournament standings; their event page
        // shows the league's division/conference/group tables instead.
        if let Some(tables) = crate::cache::standings_for_event_name(&league) {
            return Ok(tables);
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

/// The WRC or WEC championship standings (by series league name), for the home
/// page and event page. Refreshed by the poller and served from its cache — never
/// fetched per request, to stay inside the free tier's daily quota.
#[server(GetMotorStandings, "/api")]
pub async fn get_motor_standings(league: String) -> Result<MotorStandings, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        Ok(crate::cache::motor_standings(&league))
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = league;
        Ok(MotorStandings::default())
    }
}

/// Every result section for a motorsport event page (the WRC overall
/// classification + Power Stage, or each finished WEC/MotoGP session), by full
/// edition name. Empty for F1 / non-motorsport editions and not-yet-finished
/// events.
#[server(GetMotorResults, "/api")]
pub async fn get_motor_results(event: String) -> Result<Vec<MotorResult>, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        Ok(crate::cache::motor_results(&event).await)
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = event;
        Ok(Vec::new())
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

/// Store reminders for a starred match — one per lead offset in `req.leads`
/// (its override else the global timer list, resolved client-side). An empty
/// list falls back to the config's single lead, preserving the prior behaviour.
#[server(AddReminder, "/api")]
pub async fn add_reminder(req: ReminderReq) -> Result<(), ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        let cfg = crate::config::Config::from_env();
        if !cfg.push_enabled() || cfg.db_path.is_empty() {
            return Err(ServerFnError::new("reminders are not available"));
        }
        let leads = resolve_leads(req.leads, cfg.reminder_lead_ms);
        // Derive each timer's time/title/body/url from the snapshot so a client
        // can't forge a notification (it only supplies its subscription + match
        // id + the lead offsets).
        let seeds = crate::cache::reminder_seeds_for_match(
            req.match_id,
            &req.sport,
            &leads,
            &req.tz,
            req.hour24,
        );
        if seeds.is_empty() {
            return Err(ServerFnError::new("match not found or already started"));
        }
        // Only arm timers whose lead window is still ahead — a timer added after
        // its notify time passed (e.g. a 1-hour timer on a match starting in 20
        // minutes) must not fire retroactively the moment it's set.
        let now = chrono::Utc::now().timestamp_millis();
        let reminders: Vec<_> = seeds
            .into_iter()
            .filter(|s| s.is_armable(now))
            .map(|s| s.into_reminder(req.sub.clone()))
            .collect();
        let conn = crate::store::shared(&cfg.db_path)
            .map_err(|e| ServerFnError::new(format!("db: {e}")))?;
        crate::store::set_match_reminders(&conn, &reminders)
            .map_err(|e| ServerFnError::new(format!("db: {e}")))?;
        Ok(())
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = req;
        Ok(())
    }
}

/// Sanitize a client-supplied lead-offset list: keep `0..=1 week`, sort, dedup,
/// cap at 10 (mirrors the client) so a forged request can't write 10k rows. An
/// empty result falls back to the config's single lead.
#[cfg(feature = "ssr")]
fn resolve_leads(leads: Vec<i64>, fallback_ms: i64) -> Vec<i64> {
    const WEEK_MS: i64 = 7 * 24 * 60 * 60 * 1000;
    let mut v: Vec<i64> = leads
        .into_iter()
        .filter(|&ms| (0..=WEEK_MS).contains(&ms))
        .collect();
    v.sort_unstable();
    v.dedup();
    v.truncate(10);
    if v.is_empty() {
        vec![fallback_ms]
    } else {
        v
    }
}

/// Subscribe to a whole sport ("sport"/"cs2"|"lol") or event ("league"/<name>).
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
        let lead_list = resolve_leads(req.leads, cfg.reminder_lead_ms);
        let s = crate::store::Subscription {
            endpoint: req.sub.endpoint,
            p256dh: req.sub.p256dh,
            auth: req.sub.auth,
            scope_kind: req.kind,
            scope_value: req.value,
            // Keep a single legacy value too (the smallest offset is the closest
            // to start, a sensible single fallback for any old reader).
            lead_ms: lead_list.first().copied().unwrap_or(cfg.reminder_lead_ms),
            lead_list,
            tz: req.tz,
            hour24: req.hour24,
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

/// Unsubscribe from a sport/event (also drops its pending reminders).
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
pub async fn remove_reminder(
    endpoint: String,
    match_id: i64,
    sport: String,
) -> Result<(), ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        let cfg = crate::config::Config::from_env();
        if cfg.db_path.is_empty() {
            return Ok(());
        }
        let conn = crate::store::shared(&cfg.db_path)
            .map_err(|e| ServerFnError::new(format!("db: {e}")))?;
        crate::store::remove_reminder(&conn, &endpoint, match_id, &sport)
            .map_err(|e| ServerFnError::new(format!("db: {e}")))?;
        Ok(())
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = (endpoint, match_id, sport);
        Ok(())
    }
}

/// Opt a single match out of a covering sport/event/team subscription (a
/// no-notify tombstone) without unsubscribing from the whole scope.
#[server(ExcludeReminder, "/api")]
pub async fn exclude_reminder(req: ReminderReq) -> Result<(), ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        let cfg = crate::config::Config::from_env();
        if !cfg.push_enabled() || cfg.db_path.is_empty() {
            return Err(ServerFnError::new("reminders are not available"));
        }
        // An opt-out needs no derived fields — just tombstone the match so every
        // covering subscription's timers skip it.
        let conn = crate::store::shared(&cfg.db_path)
            .map_err(|e| ServerFnError::new(format!("db: {e}")))?;
        crate::store::exclude_match(&conn, &req.sub.endpoint, req.match_id, &req.sport)
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
