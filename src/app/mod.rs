use crate::bracket;
use crate::server::{
    get_day, get_event_schedule, get_event_stages, get_event_stages_by_league, get_f1_results,
    get_f1_standings, get_match_detail, get_motor_results, get_motor_standings, get_notifications,
    get_range, get_schedule, get_site, get_team_schedule,
};
use crate::types::{
    competition_kind, full_event_name, DayGroup, EventInfo, F1Result, F1Standings, MatchDetail,
    MatchStatus, MatchView, MotorResult, MotorStandingTable, MotorStandings, ScheduleView, Series,
    SeriesGame, Sport, StreamView,
};
use leptos::prelude::*;
use leptos_meta::{provide_meta_context, HashedStylesheet, MetaTags, Title};
use leptos_router::{
    components::{Route, Router, Routes, A},
    hooks::use_params,
    params::Params,
    ParamSegment, StaticSegment,
};
use std::collections::{BTreeMap, HashSet};

// Standings / Swiss grid / elimination bracket rendering + reveal machinery.
mod playoffs;
pub(crate) use playoffs::*;

// Page route components, shared UI components, free helpers, and the
// hydrate-only browser-storage glue — split out of this file. Re-exported
// flat so every module reaches the others (and the shared contexts here)
// through `crate::app::*`, the way `playoffs` already does.
pub(crate) mod helpers;
pub(crate) use helpers::*;
// Browser-storage glue is entirely client-side, so the module (and its
// re-export) only exist under hydrate; the server never references it.
#[cfg(feature = "hydrate")]
pub(crate) mod storage;
#[cfg(feature = "hydrate")]
pub(crate) use storage::*;
pub(crate) mod components;
pub(crate) use components::*;
pub(crate) mod pages;
pub(crate) use pages::*;

/// localStorage / cookie keys, centralized so a preference's read and write can't
/// silently drift onto different keys (and so the storage schema is greppable in
/// one place). Browser storage is client-only, so these are referenced only under
/// `hydrate`. The sport-mode key lives in [`SPORT_MODE_KEY`] (read on both sides).
#[cfg(feature = "hydrate")]
pub(crate) mod keys {
    pub const THEME: &str = "theme";
    pub const HOUR24: &str = "hour24";
    pub const SCORES: &str = "scores";
    pub const SUBS: &str = "subs";
    pub const STARRED: &str = "starred";
    pub const EXCLUDED: &str = "excluded";
    pub const REVEALED: &str = "revealed";
    pub const SECTIONS: &str = "sections";
    pub const RANGE: &str = "range";
    pub const GAMES: &str = "games";
    pub const LEAGUES: &str = "leagues";
    pub const ICONS: &str = "icons";
    /// Global reminder timers — comma-separated lead offsets in ms.
    pub const TIMERS: &str = "timers";
    /// Per-entry timer overrides — one `key\tms,ms,…` line per entry.
    pub const OVERRIDES: &str = "overrides";
}

#[must_use]
pub fn shell(options: LeptosOptions) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8" />
                <meta name="viewport" content="width=device-width, initial-scale=1" />
                <link rel="icon" href="data:," />
                // The VAPID public key (when Web Push is configured) so the client
                // can render the subscribe ★s on the first paint without a fetch.
                {initial_vapid().map(|k| view! { <meta name="pte-vapid" content=k /> })}
                // The config's default reminder lead (ms), so the client seeds the
                // global timers from it without a round-trip.
                <meta name="pte-lead-ms" content=initial_lead_ms().to_string() />

                // Emits <link rel="stylesheet"> with the content-hashed CSS name
                // (reads hash.txt via LeptosOptions). Must live here in the server
                // shell since LeptosOptions isn't available in the App body.
                <HashedStylesheet options=options.clone() id="leptos" />
                <AutoReload options=options.clone() />
                <HydrationScripts options />
                <MetaTags />
                // Apply the saved theme + icon + scores preferences before paint to
                // avoid a flash: CSS hides logos when data-icons="0", and the icons/
                // scores toggles show their on-state from data-icons/data-scores
                // (both default off — on only when explicitly stored "1").
                <script>
                    r#"(function(){try{var d=document.documentElement;var t=localStorage.getItem('theme')||'dark';d.setAttribute('data-theme',t);d.setAttribute('data-icons',localStorage.getItem('icons')==='1'?'1':'0');d.setAttribute('data-scores',localStorage.getItem('scores')==='1'?'1':'0');}catch(e){document.documentElement.setAttribute('data-theme','dark');}})();"#
                </script>
                // Web Push helper used by the reminder ★ buttons.
                <script>
                    r#"
                    function pteB64ToUint8(b64){var p='='.repeat((4-b64.length%4)%4);var s=(b64+p).replace(/-/g,'+').replace(/_/g,'/');var raw=atob(s);var a=new Uint8Array(raw.length);for(var i=0;i<raw.length;i++)a[i]=raw.charCodeAt(i);return a;}
                    window.pteSubscribe=async function(vapid){
                      if(!('serviceWorker' in navigator)||!('PushManager' in window)) throw new Error('unsupported');
                      var reg=await navigator.serviceWorker.register('/sw.js');
                      var perm=await Notification.requestPermission();
                      if(perm!=='granted') throw new Error('denied');
                      var sub=await reg.pushManager.getSubscription();
                      if(!sub){sub=await reg.pushManager.subscribe({userVisibleOnly:true,applicationServerKey:pteB64ToUint8(vapid)});}
                      var j=sub.toJSON();
                      return {endpoint:j.endpoint,p256dh:j.keys.p256dh,auth:j.keys.auth};
                    };
                    // Like pteSubscribe but never prompts or creates a subscription:
                    // returns the EXISTING one (or null). Used to reconcile on load
                    // without nagging users who haven't opted in.
                    window.pteExistingSub=async function(){
                      if(!('serviceWorker' in navigator)||!('PushManager' in window)) return null;
                      if(Notification.permission!=='granted') return null;
                      var reg=await navigator.serviceWorker.register('/sw.js');
                      var sub=await reg.pushManager.getSubscription();
                      if(!sub) return null;
                      var j=sub.toJSON();
                      return {endpoint:j.endpoint,p256dh:j.keys.p256dh,auth:j.keys.auth};
                    };
                    "#
                </script>
            </head>
            <body>
                <App />
            </body>
        </html>
    }
}

/// Global "reveal scores" preference (newtype so it doesn't collide with the
/// `RwSignal<bool>` used for the 24h setting in context).
#[derive(Clone, Copy)]
struct ShowScores(RwSignal<bool>);

/// Whether to show the venue-local time on every sport's row at once — clicking
/// any one time toggles it for all of them.
#[derive(Clone, Copy)]
struct ShowVenue(RwSignal<bool>);

/// Set of match uids whose scores are individually revealed (by clicking the
/// match's "Final" badge). Keyed by uid (`"{sport}-{id}"`) since a match id isn't
/// unique across games.
#[derive(Clone, Copy)]
struct RevealedMatches(RwSignal<HashSet<String>>);

/// Revealed standings/bracket-round sections, keyed by `"st:<tid>"` /
/// `"bk:<tid>:<round>"` so reveal state is per-round and shared by every page
/// that shows the same tournament's bracket.
#[derive(Clone, Copy)]
struct RevealedSections(RwSignal<HashSet<String>>);

/// The "end" time (ms since epoch) a page supplies for the section reveals on it —
/// when the thing those reveals are about is finished and won't change (an event's
/// last match, a GP weekend, a series' latest race). Recorded with each reveal so
/// it can be pruned ~a week past that end. Absent ⇒ section reveals fall back to
/// "now". Per-match (row) reveals use the row's own match time instead.
#[derive(Clone, Copy)]
pub(crate) struct RevealEnd(pub(crate) i64);

/// The reveal "end" to record for a section reveal on the current page: the
/// page-provided [`RevealEnd`], else the current time (a section with no provided
/// end is simply protected for a week, then ages out by inactivity).
#[cfg(feature = "hydrate")]
pub(crate) fn current_reveal_end() -> i64 {
    use_context::<RevealEnd>()
        .map(|r| r.0)
        .unwrap_or_else(now_ms)
}
#[cfg(not(feature = "hydrate"))]
pub(crate) fn current_reveal_end() -> i64 {
    use_context::<RevealEnd>().map(|r| r.0).unwrap_or(0)
}

/// Set of "kind|value" scopes the user is subscribed to (sport/event reminders).
#[derive(Clone, Copy)]
struct Subscribed(RwSignal<HashSet<String>>);

/// Match uids the user has opted out of even though a subscription covers them
/// (a per-match "un-notify" that doesn't drop the whole subscription).
#[derive(Clone, Copy)]
struct Excluded(RwSignal<HashSet<String>>);

/// The global reminder timers: lead offsets (ms before start) applied to every
/// subscription / starred match that hasn't set its own. Seeded from the config
/// default (15m) until the saved list loads.
#[derive(Clone, Copy)]
struct GlobalTimers(RwSignal<Vec<i64>>);

/// Per-entry timer overrides, keyed by a subscription's `sub_key` or a starred
/// match's uid. An entry present here ignores the global timers and uses its own
/// list instead.
#[derive(Clone, Copy)]
struct Overrides(RwSignal<BTreeMap<String, Vec<i64>>>);

/// Calendar-selected date range (start, end ISO dates); `None` = default view.
#[derive(Clone, Copy)]
struct DateRange(RwSignal<Option<(String, String)>>);

/// How many extra past days the "‹ show earlier days" control has revealed.
#[derive(Clone, Copy)]
struct EarlierDays(RwSignal<i64>);

/// Extra future days the "show later days ›" control has revealed beyond the
/// default window, for traditional sports (capped at `TRAD_FORWARD_MAX`).
#[derive(Clone, Copy)]
struct LaterDays(RwSignal<i64>);

/// Selected sport filters (slugs) and event filters (league names) — shared and
/// persisted so they survive navigating to a match and back.
#[derive(Clone, Copy)]
struct Games(RwSignal<HashSet<String>>);
#[derive(Clone, Copy)]
struct Leagues(RwSignal<HashSet<String>>);
/// The server's data-freshness time, shown in the header (None until a schedule
/// loads). A bump of `RefreshTrigger` makes the schedule refetch from the server.
#[derive(Clone, Copy)]
struct LastUpdated(RwSignal<Option<String>>);
#[derive(Clone, Copy)]
struct RefreshTrigger(RwSignal<u64>);

/// Top-level mode: `false` = esports, `true` = traditional sports (MLB). Switched
/// by the header toggle; persisted across navigation.
#[derive(Clone, Copy)]
struct SportMode(RwSignal<bool>);

#[component]
#[must_use]
pub fn App() -> impl IntoView {
    provide_meta_context();

    // Shared user preferences: 24h time and the detected IANA timezone, both
    // seeded from their cookies at render (server + hydrate) so the first paint —
    // and the schedule resource's key — already match the viewer. The
    // post-hydration effect confirms them from the browser and back-fills the
    // cookies, re-keying only if a seed was actually wrong.
    let hour24 = RwSignal::new(initial_hour24());
    let tz = RwSignal::new(initial_tz());
    // Reminder state: starred match ids, and the VAPID public key (None until
    // fetched / if Web Push isn't configured).
    let starred = RwSignal::new(HashSet::<String>::new());
    // Matches opted out of a covering subscription (per-match un-notify).
    let excluded = RwSignal::new(HashSet::<String>::new());
    // The VAPID public key, resolved for this render (env on the server, the
    // shell's <meta> on hydrate) so the subscribe ★s are present at first paint
    // instead of popping in after a round-trip. `None` => Web Push off, ★s hidden.
    let vapid = RwSignal::new(initial_vapid());
    // Spoiler control: a global reveal + a per-match reveal set.
    let show_scores = RwSignal::new(false);
    // Shared so clicking any one game's time reveals every venue time at once.
    let show_venue = RwSignal::new(false);
    let revealed = RwSignal::new(HashSet::<String>::new());
    let sections = RwSignal::new(HashSet::<String>::new());
    let subscribed = RwSignal::new(HashSet::<String>::new());
    // Reminder timers: the global list (seeded from the config default until the
    // saved one loads) and per-entry overrides.
    let global_timers = RwSignal::new(vec![initial_lead_ms()]);
    let overrides = RwSignal::new(BTreeMap::<String, Vec<i64>>::new());
    // History views: a calendar-selected range, and the "earlier days" expansion.
    let range = RwSignal::new(None::<(String, String)>);
    let earlier = RwSignal::new(0i64);
    let later = RwSignal::new(0i64);
    // Schedule filters (shared + persisted across navigation).
    // Seed the filters from the URL at render (server + hydrate) so a filtered
    // link paints already-filtered, with no flash of the full schedule.
    let games = RwSignal::new(initial_games());
    let leagues = RwSignal::new(initial_leagues());
    // Header data-freshness label + a manual-refresh trigger.
    let last_updated = RwSignal::new(None::<String>);
    let refresh_trigger = RwSignal::new(0u64);
    // Top-level sport mode (esports vs traditional). Resolved from the request
    // (URL + cookie) on the server so the first paint already shows the right mode
    // — the same value the client computes on hydrate, so no flash, no mismatch.
    let traditional = RwSignal::new(initial_sport_mode());
    provide_context(hour24);
    provide_context(tz);
    provide_context(starred);
    provide_context(Excluded(excluded));
    provide_context(vapid);
    provide_context(ShowScores(show_scores));
    provide_context(ShowVenue(show_venue));
    provide_context(RevealedMatches(revealed));
    provide_context(RevealedSections(sections));
    provide_context(Subscribed(subscribed));
    provide_context(GlobalTimers(global_timers));
    provide_context(Overrides(overrides));
    provide_context(DateRange(range));
    provide_context(EarlierDays(earlier));
    provide_context(LaterDays(later));
    provide_context(Games(games));
    provide_context(Leagues(leagues));
    provide_context(LastUpdated(last_updated));
    provide_context(RefreshTrigger(refresh_trigger));
    provide_context(SportMode(traditional));

    // After hydration, pick up the browser's timezone + saved preferences and
    // the push key. (Client-side only; the initial render uses the cookie-seeded
    // values above, so there's no hydration mismatch.) The schedule resource keys
    // on tz/hour24/range, and an `RwSignal::set` notifies even when the value is
    // unchanged, so each of those is guarded by a `get_untracked` compare —
    // otherwise the cookie-seeded common case would still refetch the whole
    // schedule right after hydration.
    Effect::new(move |_| {
        #[cfg(feature = "hydrate")]
        {
            if let Some(z) = detect_tz() {
                // Persist for the next SSR render; re-key only if the seed was wrong.
                write_cookie(TZ_COOKIE_KEY, &z);
                if tz.get_untracked() != z {
                    tz.set(z);
                }
            }
            if let Some(h) = load_hour24_pref() {
                write_cookie(HOUR24_COOKIE_KEY, if h { "1" } else { "0" });
                if hour24.get_untracked() != h {
                    hour24.set(h);
                }
            }
            show_scores.set(load_scores_pref());
            starred.set(load_starred());
            excluded.set(load_excluded());
            subscribed.set(load_subs());
            if let Some(t) = load_timers() {
                global_timers.set(t);
            }
            overrides.set(load_overrides());
            // Prune stale spoiler reveals (both >1wk past their entity's end and
            // >1wk untouched) as we load them, so the stored set can't grow forever.
            let reveal_now = now_ms();
            revealed.set(prune_and_load(keys::REVEALED, reveal_now));
            sections.set(prune_and_load(keys::SECTIONS, reveal_now));
            // Also in the schedule resource key, so guard it the same way.
            let saved_range = load_range();
            if range.get_untracked() != saved_range {
                range.set(saved_range);
            }
            // `games`/`leagues` are initialised by `FilterUrlSync` (URL query, else
            // localStorage) since it needs the router context.
            setup_scrollspy();
            // Reconcile the server's reminders for this browser with the just-loaded
            // local state, so the DB never drifts from localStorage (no-op unless
            // push is configured and already granted — it never prompts on load).
            reconcile_notifications(
                subscribed.get_untracked(),
                starred.get_untracked(),
                excluded.get_untracked(),
                global_timers.get_untracked(),
                overrides.get_untracked(),
                vapid.get_untracked(),
            );
        }
    });

    view! {
        <Title text="plaintextesports" />
        <Router>
            <FilterUrlSync />
            <div class="page">
                <SiteHeader />
                <main class="main">
                    <Routes fallback=|| view! { <p class="empty">"Page not found."</p> }>
                        <Route path=StaticSegment("") view=HomePage />
                        <Route path=(StaticSegment("day"), ParamSegment("date")) view=DayPage />
                        <Route
                            path=(StaticSegment("match"), ParamSegment("sport"), ParamSegment("id"))
                            view=MatchDetailPage
                        />
                        <Route
                            path=(StaticSegment("event"), ParamSegment("sport"), ParamSegment("league"))
                            view=EventPage
                        />
                        <Route
                            path=(StaticSegment("team"), ParamSegment("sport"), ParamSegment("name"))
                            view=TeamPage
                        />
                        <Route path=StaticSegment("notifications") view=NotificationsPage />
                        <Route path=StaticSegment("about") view=AboutPage />
                    </Routes>
                </main>
                <SiteFooter />
            </div>
        </Router>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{BracketMatch, DayGroup, LeagueGroup, Sport, TeamView};

    #[test]
    fn fmt_lead_picks_the_largest_whole_unit() {
        assert_eq!(fmt_lead(0), "at start");
        assert_eq!(fmt_lead(5 * 60_000), "5m");
        assert_eq!(fmt_lead(90 * 60_000), "90m"); // not a whole hour
        assert_eq!(fmt_lead(60 * 60_000), "1h");
        assert_eq!(fmt_lead(24 * 60 * 60_000), "1d");
        assert_eq!(fmt_lead(7 * 24 * 60 * 60_000), "1w");
    }

    #[test]
    fn norm_leads_clamps_sorts_dedups_and_caps() {
        // Out-of-range dropped, duplicates removed, sorted ascending.
        let v = norm_leads(vec![900_000, -5, 900_000, WEEK_MS + 1, 0, 60_000]);
        assert_eq!(v, vec![0, 60_000, 900_000]);
        // Capped at MAX_TIMERS.
        let many: Vec<i64> = (1..=20).map(|n| n * 60_000).collect();
        assert_eq!(norm_leads(many).len(), MAX_TIMERS);
    }

    #[test]
    fn lead_list_round_trips() {
        let v = vec![0, 900_000, 86_400_000];
        assert_eq!(parse_lead_list(&join_lead_list(&v)), v);
        // Empty / junk parses to nothing.
        assert!(parse_lead_list("").is_empty());
        assert!(parse_lead_list("x,,y").is_empty());
    }

    #[test]
    fn effective_leads_prefers_the_override() {
        let mut ov = BTreeMap::new();
        ov.insert("sport|cs2".to_string(), vec![60_000]);
        assert_eq!(effective_leads("sport|cs2", &ov, &[900_000]), vec![60_000]);
        // No override ⇒ the global list.
        assert_eq!(effective_leads("sport|lol", &ov, &[900_000]), vec![900_000]);
    }

    #[test]
    fn parse_uid_inverts_match_uid() {
        let uid = crate::types::match_uid(Sport::Lol, 12345);
        assert_eq!(parse_uid(&uid), Some((Sport::Lol, 12345)));
        // Motorsport (F1/WRC/WEC) still splits on the trailing id.
        let uid = crate::types::match_uid(Sport::Motorsport, 7);
        assert_eq!(parse_uid(&uid), Some((Sport::Motorsport, 7)));
        assert_eq!(parse_uid("not-a-real-uid"), None);
    }

    fn mv(league: &str) -> MatchView {
        MatchView {
            id: 1,
            sport: Sport::Lol,
            league: league.into(),
            series_name: String::new(),
            status: MatchStatus::Upcoming,
            clock_label: String::new(),
            date_label: String::new(),
            venue_label: String::new(),
            venue_name: String::new(),
            venue_location: String::new(),
            best_of: "Bo3".into(),
            team_a: TeamView {
                label: "A".into(),
                name: "A".into(),
                score: None,
                winner: false,
                logo: String::new(),
                abbrev: String::new(),
            },
            team_b: TeamView {
                label: "B".into(),
                name: "B".into(),
                score: None,
                winner: false,
                logo: String::new(),
                abbrev: String::new(),
            },
            event_url: String::new(),
            begin_at_ms: 0,
            row_href: None,
        }
    }

    fn sched(days: Vec<Vec<&str>>) -> ScheduleView {
        let days = days
            .into_iter()
            .enumerate()
            .map(|(i, leagues)| DayGroup {
                day_key: format!("d{i}"),
                day_label: format!("Day {i}"),
                leagues: leagues
                    .into_iter()
                    .map(|l| LeagueGroup {
                        league: l.into(),
                        series_name: String::new(),
                        event_url: String::new(),
                        bo: None,
                        matches: vec![mv(l)],
                    })
                    .collect(),
            })
            .collect();
        ScheduleView {
            days,
            ..Default::default()
        }
    }

    #[test]
    fn color_class_is_stable_and_in_range() {
        let a = league_color_class("Mid-Season Invitational");
        assert_eq!(a, league_color_class("Mid-Season Invitational"));
        let n: u32 = a.strip_prefix("lc-").unwrap().parse().unwrap();
        assert!(n < LEAGUE_COLORS);
    }

    #[test]
    fn calendar_date_math() {
        // 2026-06-21 is a Sunday (0); 2026-06-01 is a Monday (1).
        assert_eq!(weekday(2026, 6, 21), 0);
        assert_eq!(weekday(2026, 6, 1), 1);
        assert_eq!(days_in_month(2026, 6), 30);
        assert_eq!(days_in_month(2026, 2), 28);
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(month_name(6), "June");
    }

    #[test]
    fn iso_add_days_handles_boundaries() {
        assert_eq!(iso_add_days("2026-06-24", 2), "2026-06-26");
        assert_eq!(iso_add_days("2026-06-24", 0), "2026-06-24");
        // Month and year rollovers, forward and back.
        assert_eq!(iso_add_days("2026-06-30", 2), "2026-07-02");
        assert_eq!(iso_add_days("2026-06-01", -1), "2026-05-31");
        assert_eq!(iso_add_days("2026-12-31", 1), "2027-01-01");
        assert_eq!(iso_add_days("2027-01-01", -1), "2026-12-31");
        // Leap day.
        assert_eq!(iso_add_days("2024-02-28", 1), "2024-02-29");
        assert_eq!(iso_add_days("2024-03-01", -1), "2024-02-29");
    }

    #[test]
    fn trad_bounds_default_and_expanded() {
        // Default (earlier=0, later=0): today through today + TRAD_FORWARD_DAYS.
        assert_eq!(
            trad_day_bounds("2026-06-24", 0, 0),
            ("2026-06-24".into(), "2026-06-26".into())
        );
        // "show later days" extends the forward end, capped at TRAD_FORWARD_MAX.
        assert_eq!(trad_day_bounds("2026-06-24", 0, 4).1, "2026-06-30"); // +6
        assert_eq!(trad_day_bounds("2026-06-24", 0, 100).1, "2026-07-01"); // capped +7
                                                                           // "show earlier days" pushes the start back.
        assert_eq!(trad_day_bounds("2026-06-24", 3, 0).0, "2026-06-21");
    }

    #[test]
    fn bracket_series_stage_transitions() {
        let mut set = HashSet::new();
        let (bn, bs) = ("bn:9:0:0", "bs:9:0:0");
        assert_eq!(stage_of(&set, bn, bs), 0);
        apply_stage(&mut set, bn, bs, 1); // names
        assert_eq!(stage_of(&set, bn, bs), 1);
        assert!(set.contains(bn) && !set.contains(bs));
        apply_stage(&mut set, bn, bs, 2); // scores
        assert_eq!(stage_of(&set, bn, bs), 2);
        apply_stage(&mut set, bn, bs, 0); // hidden
        assert!(set.is_empty());
    }

    #[test]
    fn bracket_max_stage_caps_reveal() {
        let mk = |a: &str, b: &str, sa, sb| BracketMatch {
            team_a: a.into(),
            team_b: b.into(),
            score_a: sa,
            score_b: sb,
            ..Default::default()
        };
        assert_eq!(bm_max_stage(&mk("NAVI", "FaZe", Some(2), Some(1))), 2); // played
        assert_eq!(bm_max_stage(&mk("NAVI", "FaZe", None, None)), 1); // decided, unplayed
        assert_eq!(bm_max_stage(&mk("NAVI", "FaZe", Some(0), Some(0))), 1); // 0–0, not played
        assert_eq!(bm_max_stage(&mk("NAVI", "FaZe", Some(2), Some(0))), 2); // a real 2–0
        assert_eq!(bm_max_stage(&mk("TBD", "FaZe", None, None)), 0); // undecided → locked
    }

    /// A 4-team single-elim grid: 2 semis (played) → 1 final (played).
    fn semis_final_grid() -> Vec<Vec<BkCell>> {
        let cell = |r, i, max, feeders: &[(usize, usize)]| BkCell {
            bn: format!("bn:1:{r}:{i}"),
            bs: format!("bs:1:{r}:{i}"),
            max,
            floor: 0,
            feeders: feeders.to_vec(),
        };
        vec![
            vec![cell(0, 0, 2, &[]), cell(0, 1, 2, &[])],
            vec![cell(1, 0, 2, &[(0, 0), (0, 1)])],
        ]
    }

    #[test]
    fn bracket_feeder_auto_reveals_next_lineup() {
        let grid = semis_final_grid();
        let mut set = HashSet::new();
        // Nothing revealed → final lineup is hidden (feeders unscored).
        assert_eq!(
            compute_effective(&grid, &set, false),
            vec![vec![0, 0], vec![0]]
        );
        // Reveal both semis' scores → the final's lineup (stage 1) auto-appears.
        apply_stage(&mut set, "bn:1:0:0", "bs:1:0:0", 2);
        apply_stage(&mut set, "bn:1:0:1", "bs:1:0:1", 2);
        assert_eq!(
            compute_effective(&grid, &set, false),
            vec![vec![2, 2], vec![1]]
        );
    }

    #[test]
    fn bracket_series_click_reveals_contributing_matches_first() {
        let grid = semis_final_grid();
        let mut set = HashSet::new();
        // Clicking the final before its teams are known reveals the feeding semis'
        // scores; the final's lineup then auto-appears (names, no score yet).
        apply_bracket_op(&mut set, &grid, BkOp::Series(1, 0));
        assert_eq!(
            compute_effective(&grid, &set, false),
            vec![vec![2, 2], vec![1]]
        );
        // The next click reveals the final's own score.
        apply_bracket_op(&mut set, &grid, BkOp::Series(1, 0));
        assert_eq!(
            compute_effective(&grid, &set, false),
            vec![vec![2, 2], vec![2]]
        );
    }

    #[test]
    fn bracket_cascade_advances_round_by_round() {
        let grid = semis_final_grid();
        let mut set = HashSet::new();
        let eff = |set: &HashSet<String>| compute_effective(&grid, set, false);
        apply_bracket_op(&mut set, &grid, BkOp::Cascade); // semis → names
        assert_eq!(eff(&set), vec![vec![1, 1], vec![0]]);
        apply_bracket_op(&mut set, &grid, BkOp::Cascade); // semis → scores (final auto-names)
        assert_eq!(eff(&set), vec![vec![2, 2], vec![1]]);
        apply_bracket_op(&mut set, &grid, BkOp::Cascade); // final → scores
        assert_eq!(eff(&set), vec![vec![2, 2], vec![2]]);
        apply_bracket_op(&mut set, &grid, BkOp::Cascade); // all shown → reset
        assert_eq!(eff(&set), vec![vec![0, 0], vec![0]]);
        assert!(set.is_empty());
    }

    // Quarterfinals → semifinals → final (4 → 2 → 1), all best-of decided.
    fn qf_sf_final_grid() -> Vec<Vec<BkCell>> {
        let cell = |r, i, feeders: &[(usize, usize)]| BkCell {
            bn: format!("bn:1:{r}:{i}"),
            bs: format!("bs:1:{r}:{i}"),
            max: 2,
            floor: 0,
            feeders: feeders.to_vec(),
        };
        vec![
            vec![
                cell(0, 0, &[]),
                cell(0, 1, &[]),
                cell(0, 2, &[]),
                cell(0, 3, &[]),
            ],
            vec![cell(1, 0, &[(0, 0), (0, 1)]), cell(1, 1, &[(0, 2), (0, 3)])],
            vec![cell(2, 0, &[(1, 0), (1, 1)])],
        ]
    }

    #[test]
    fn bracket_hide_cascades_through_all_rounds() {
        let grid = qf_sf_final_grid();
        let mut set = HashSet::new();
        // Reveal every match's score.
        for row in &grid {
            for c in row {
                apply_stage(&mut set, &c.bn, &c.bs, 2);
            }
        }
        assert_eq!(
            compute_effective(&grid, &set, false),
            vec![vec![2, 2, 2, 2], vec![2, 2], vec![2]]
        );
        // Hide one quarterfinal's score: its fed semifinal can no longer show a
        // score (a team is unknown), and that cascades to the final too — not
        // just the semifinal. Both drop to their lineup stage (1).
        apply_stage(&mut set, "bn:1:0:0", "bs:1:0:0", 0);
        assert_eq!(
            compute_effective(&grid, &set, false),
            vec![vec![0, 2, 2, 2], vec![1, 2], vec![1]]
        );
    }

    #[test]
    fn cascade_normalizes_granular_reveals() {
        // Individually reveal one quarterfinal's score (granular), then use the
        // outer "Bracket" cascade: it pulls that match back into the even
        // progression (the whole round at names) instead of leaving it ahead.
        let grid = qf_sf_final_grid();
        let mut set = HashSet::new();
        apply_bracket_op(&mut set, &grid, BkOp::Series(0, 0)); // (0,0) → names
        apply_bracket_op(&mut set, &grid, BkOp::Series(0, 0)); // (0,0) → score
        assert_eq!(
            compute_effective(&grid, &set, false),
            vec![vec![2, 0, 0, 0], vec![0, 0], vec![0]]
        );
        // Cascade: the round becomes uniformly names — (0,0)'s score is dropped.
        apply_bracket_op(&mut set, &grid, BkOp::Cascade);
        assert_eq!(
            compute_effective(&grid, &set, false),
            vec![vec![1, 1, 1, 1], vec![0, 0], vec![0]]
        );
        assert!(
            !set.contains("bs:1:0:0"),
            "the granular score reveal is cleared"
        );
        // The next cascade reveals the whole round's scores together.
        apply_bracket_op(&mut set, &grid, BkOp::Cascade);
        assert_eq!(
            compute_effective(&grid, &set, false),
            vec![vec![2, 2, 2, 2], vec![1, 1], vec![0]]
        );
    }

    #[test]
    fn cascade_forgets_ahead_reveals() {
        // A branch shown into the middle (a semifinal + its feeders) while the
        // other quarterfinals stay hidden.
        let grid = qf_sf_final_grid();
        let mut set = HashSet::new();
        apply_stage(&mut set, "bn:1:0:0", "bs:1:0:0", 2);
        apply_stage(&mut set, "bn:1:0:1", "bs:1:0:1", 2);
        apply_stage(&mut set, "bn:1:1:0", "bs:1:1:0", 2);
        assert_eq!(
            compute_effective(&grid, &set, false),
            vec![vec![2, 2, 0, 0], vec![2, 0], vec![0]]
        );
        // The cascade evens the front round and drops the ahead semifinal.
        apply_bracket_op(&mut set, &grid, BkOp::Cascade);
        assert_eq!(
            compute_effective(&grid, &set, false),
            vec![vec![1, 1, 1, 1], vec![0, 0], vec![0]]
        );
        assert!(
            !set.contains("bs:1:1:0"),
            "the ahead semifinal reveal is forgotten"
        );
    }

    #[test]
    fn round_toggle_owns_later_rounds() {
        // Everything revealed (later rounds remembered from earlier clicks).
        let grid = qf_sf_final_grid();
        let mut set = HashSet::new();
        for row in &grid {
            for c in row {
                apply_stage(&mut set, &c.bn, &c.bs, 2);
            }
        }
        // Cycling the first round hides it and drops the later rounds with it.
        apply_bracket_op(&mut set, &grid, BkOp::Round(0));
        assert_eq!(
            compute_effective(&grid, &set, false),
            vec![vec![0, 0, 0, 0], vec![0, 0], vec![0]]
        );
        assert!(set.is_empty());
        // Re-revealing the round doesn't bring the later rounds' scores back —
        // only the next round's auto-revealed lineup follows.
        apply_bracket_op(&mut set, &grid, BkOp::Round(0)); // names
        apply_bracket_op(&mut set, &grid, BkOp::Round(0)); // scores
        assert_eq!(
            compute_effective(&grid, &set, false),
            vec![vec![2, 2, 2, 2], vec![1, 1], vec![0]]
        );
    }

    #[test]
    fn enc_dec_segment_round_trips() {
        // Includes names that encode to a trailing %XX (trailing space) and a
        // literal '%', to confirm the decoder handles the end-of-string case.
        for name in ["IEM Cologne", "Cologne ", "100% Pro", "A-B_C.D~E", "x%"] {
            assert_eq!(dec_segment(&enc_segment(name)), name, "round-trip {name:?}");
        }
    }

    fn names(items: &[&str]) -> HashSet<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn leagues_for_games_first_appearance_order() {
        let s = sched(vec![vec!["LCK", "LEC"], vec!["LCK", "LPL"]]);
        let names_of = |games: &HashSet<String>| -> Vec<String> {
            leagues_for_games(&s, games)
                .into_iter()
                .map(|(l, _)| l)
                .collect()
        };
        assert_eq!(names_of(&HashSet::new()), vec!["LCK", "LEC", "LPL"]);
        // `mv` matches are LoL, so the cs2 sport filter hides them all.
        assert!(names_of(&names(&["cs2"])).is_empty());
        assert_eq!(names_of(&names(&["lol"])), vec!["LCK", "LEC", "LPL"]);
        // Each chip carries its match's sport, for the sport-scoped subscribe key.
        assert!(leagues_for_games(&s, &HashSet::new())
            .iter()
            .all(|(_, sp)| *sp == Sport::Lol));
    }

    #[test]
    fn leagues_for_games_motorsport_follows_schedule_order() {
        // A motorsport league group whose chip carries Sport::Motorsport.
        let motor = |name: &str| {
            let mut m = mv(name);
            m.sport = Sport::Motorsport;
            LeagueGroup {
                league: name.into(),
                series_name: String::new(),
                event_url: String::new(),
                bo: None,
                matches: vec![m],
            }
        };
        let lol = |name: &str| LeagueGroup {
            league: name.into(),
            series_name: String::new(),
            event_url: String::new(),
            bo: None,
            matches: vec![mv(name)],
        };
        // The series appear in the schedule in this order (WRC, F1, WEC, MotoGP).
        let s = ScheduleView {
            days: vec![DayGroup {
                day_key: "d0".into(),
                day_label: "D0".into(),
                leagues: vec![
                    lol("LEC"),
                    motor("WRC"),
                    motor("F1"),
                    motor("WEC"),
                    motor("MotoGP"),
                ],
            }],
            ..Default::default()
        };
        let names: Vec<String> = leagues_for_games(&s, &HashSet::new())
            .into_iter()
            .map(|(l, _)| l)
            .collect();
        // The chips follow the schedule's first-appearance order exactly — no
        // separate tidy — so the filter bar matches the order leagues appear.
        assert_eq!(names, vec!["LEC", "WRC", "F1", "WEC", "MotoGP"]);
    }

    #[test]
    fn sub_key_round_trips_through_parse_sub_key() {
        // A whole-sport sub keys by slug alone; parsing resolves the sport back.
        let k = sub_key("sport", Sport::Cs2, "cs2");
        assert_eq!(k, "sport|cs2");
        assert_eq!(
            parse_sub_key(&k),
            ("sport".to_string(), Some(Sport::Cs2), "cs2".to_string())
        );

        // team/league/event fold the sport in but keep the bare name as the value
        // (what the server matches on), so the notifications page can rebuild the
        // sport-scoped page link without changing the server-side scope format.
        for (kind, value) in [
            ("team", "Detroit Tigers"),
            ("league", "MLB"),
            ("event", "World Cup"),
        ] {
            let k = sub_key(kind, Sport::Mlb, value);
            assert_eq!(k, format!("{kind}|mlb|{value}"));
            assert_eq!(
                parse_sub_key(&k),
                (kind.to_string(), Some(Sport::Mlb), value.to_string())
            );
        }

        // A name that itself contains '|' survives (splitn(3) keeps the tail whole).
        let k = sub_key("event", Sport::Cs2, "A|B Invitational");
        assert_eq!(
            parse_sub_key(&k),
            (
                "event".to_string(),
                Some(Sport::Cs2),
                "A|B Invitational".to_string()
            )
        );
    }

    #[test]
    fn filter_schedule_keeps_selected_events_and_drops_empty_days() {
        let s = sched(vec![vec!["LCK", "LEC"], vec!["LPL"]]);
        let f = filter_schedule(s, &HashSet::new(), &names(&["LCK", "LEC"]));
        assert_eq!(f.days.len(), 1);
        assert_eq!(f.days[0].leagues.len(), 2);
    }

    #[test]
    fn filter_schedule_empty_keeps_everything() {
        let s = sched(vec![vec!["LCK", "LEC"]]);
        let f = filter_schedule(s, &HashSet::new(), &HashSet::new());
        assert_eq!(f.days[0].leagues.len(), 2);
    }

    #[test]
    fn filter_schedule_games_and_events_are_anded() {
        let s = sched(vec![vec!["LCK", "LEC"]]);
        // LoL sport keeps both; cs2 drops everything (mv matches are LoL).
        assert_eq!(
            filter_schedule(s.clone(), &names(&["lol"]), &HashSet::new())
                .days
                .len(),
            1
        );
        assert!(
            filter_schedule(s.clone(), &names(&["cs2"]), &HashSet::new())
                .days
                .is_empty()
        );
        // lol AND only the LCK event → one group.
        let f = filter_schedule(s, &names(&["lol"]), &names(&["LCK"]));
        assert_eq!(f.days[0].leagues.len(), 1);
        assert_eq!(f.days[0].leagues[0].league, "LCK");
    }

    #[cfg(any(feature = "ssr", feature = "hydrate"))]
    #[test]
    fn resolve_sport_mode_precedence() {
        // An explicit ?mode wins outright.
        assert!(resolve_sport_mode(Some("sports"), None, None));
        assert!(!resolve_sport_mode(Some("esports"), None, Some("sports")));
        // Else infer from a known sport slug in ?g.
        assert!(resolve_sport_mode(None, Some("nhl"), None)); // traditional
        assert!(!resolve_sport_mode(None, Some("cs2"), Some("sports"))); // esports slug overrides cookie
        assert!(resolve_sport_mode(None, Some("cs2,mlb"), None)); // any traditional ⇒ sports
                                                                  // An unknown ?g (no known slug) falls through to the cookie.
        assert!(resolve_sport_mode(None, Some("bogus"), Some("sports")));
        // Else the cookie, defaulting to esports.
        assert!(resolve_sport_mode(None, None, Some("sports")));
        assert!(!resolve_sport_mode(None, None, Some("esports")));
        assert!(!resolve_sport_mode(None, None, None));
    }

    #[cfg(any(feature = "ssr", feature = "hydrate"))]
    #[test]
    fn query_param_extracts_first_match() {
        assert_eq!(
            query_param("mode=sports&g=nhl", "mode").as_deref(),
            Some("sports")
        );
        assert_eq!(
            query_param("mode=sports&g=nhl", "g").as_deref(),
            Some("nhl")
        );
        assert_eq!(query_param("g=cs2,lol", "g").as_deref(), Some("cs2,lol"));
        assert_eq!(query_param("mode=sports", "g"), None);
        assert_eq!(query_param("", "mode"), None);
    }
}
