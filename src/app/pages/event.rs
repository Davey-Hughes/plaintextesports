//! The event page and its stage combo / stage-list rendering.
use crate::app::*;

#[derive(Params, PartialEq, Clone)]
pub(crate) struct EventParams {
    /// The sport slug segment, e.g. `"cs2"` — scopes the event but the lookup is
    /// still keyed by `league` name alone.
    sport: String,
    league: String,
}

/// An event's stages rendered as labelled sections — each a Swiss/group stage
/// (with a bracket/list toggle) and/or a playoff bracket. One toggle drives
/// every Swiss stage. Shared by the event page and the front-page single-event
/// filter.
/// One stage's standings/bracket combo: a Swiss stage shows a buchholz-grid /
/// ranking-list toggle (sharing `swiss_grid` so sibling stages switch together);
/// every stage also shows its playoff bracket. Used by the event page (one per
/// stage) and the match detail page (its single stage).
#[component]
pub(crate) fn EventStageCombo(
    event: EventInfo,
    /// Shared grid/list toggle so multiple stages flip together.
    swiss_grid: RwSignal<bool>,
    /// `match_id -> (day_key, clock_label)` so brackets can date rounds and show
    /// per-match times; empty where the schedule isn't on the page.
    #[prop(optional)]
    times: std::collections::HashMap<i64, (String, String)>,
) -> impl IntoView {
    let times = StoredValue::new(times);
    // Switching keeps the clicked tab pinned in place (the views differ in
    // height), so the page doesn't jump.
    let set_view = move |e: leptos::ev::MouseEvent, grid: bool| {
        #[cfg(feature = "hydrate")]
        {
            use wasm_bindgen::JsCast;
            let anchor = e
                .current_target()
                .and_then(|t| t.dyn_into::<web_sys::Element>().ok());
            let before = anchor
                .as_ref()
                .map(|el| el.get_bounding_client_rect().top());
            swiss_grid.set(grid);
            if let (Some(el), Some(before)) = (anchor, before) {
                request_animation_frame(move || {
                    let after = el.get_bounding_client_rect().top();
                    if let Some(w) = web_sys::window() {
                        w.scroll_by_with_x_and_y(0.0, after - before);
                    }
                });
            }
        }
        #[cfg(not(feature = "hydrate"))]
        {
            let _ = &e;
            swiss_grid.set(grid);
        }
    };
    let EventInfo {
        tournament_id,
        stage,
        sport,
        standings,
        rounds,
        swiss,
        ..
    } = event;
    let bracket_only = standings.is_empty();
    let has_swiss = !swiss.is_empty();
    // A stage carrying a bracket or Swiss grid is wide (it spans both columns of
    // the stages grid); a plain standings table is narrow and pairs up two-across.
    let wide = !rounds.is_empty() || has_swiss;
    let label = (!stage.is_empty()).then(|| view! { <h2 class="stage-head">{stage}</h2> });
    // A Swiss stage gets a grid/list toggle; otherwise just the standings
    // (+ any playoff bracket).
    let tabs = has_swiss.then(|| {
        view! {
            <div class="swiss-tabs">
                <button
                    class="swiss-tab"
                    class:active=move || swiss_grid.get()
                    on:click=move |e| set_view(e, true)
                >
                    "bracket"
                </button>
                <button
                    class="swiss-tab"
                    class:active=move || !swiss_grid.get()
                    on:click=move |e| set_view(e, false)
                >
                    "list"
                </button>
            </div>
        }
    });
    // Stack the grid and the list in one cell so the taller one fixes the
    // height — switching tabs never changes the page height. The inactive
    // view is hidden but still occupies its space.
    let grid_view = has_swiss.then(|| {
        view! {
            <div class="swiss-view" class:swiss-view-off=move || !swiss_grid.get()>
                <SwissBracket rounds=swiss tournament_id sport />
            </div>
        }
    });
    let list_view = view! {
        <div class="swiss-view" class:swiss-view-off=move || has_swiss && swiss_grid.get()>
            <StandingsTable rows=standings tournament_id sport />
        </div>
    };
    view! {
        <div class="event-extra spy" class:event-extra-wide=wide id=format!("stage-{tournament_id}")>
            <div class="stage-bar">{label}</div>
            // The grid/list toggle sits at the top-right of the views, on the
            // "Bracket"/"Standings" title row (just above its underline).
            <div class="swiss-views">
                {tabs}
                {grid_view}
                {list_view}
            </div>
            <Bracket rounds=rounds tournament_id sport bracket_only times=times.get_value() />
        </div>
    }
}

#[component]
pub(crate) fn EventStages(
    stages: Vec<EventInfo>,
    /// `match_id -> (day_key, clock_label)` so brackets can date rounds and show
    /// per-match times; empty where the schedule isn't on the page.
    #[prop(optional)]
    times: std::collections::HashMap<i64, (String, String)>,
) -> impl IntoView {
    let times = StoredValue::new(times);
    // One toggle drives every stage.
    let swiss_grid = RwSignal::new(true);
    // Pure standings stages (a group/division table — no bracket or Swiss grid)
    // group under one shared "Standings" heading, each labelled by its group or
    // division name only, rather than repeating "Standings" for every one. Richer
    // stages (brackets, Swiss) keep their own heading and reveal.
    let (groups, others): (Vec<EventInfo>, Vec<EventInfo>) = stages
        .into_iter()
        .partition(|e| !e.standings.is_empty() && e.rounds.is_empty() && e.swiss.is_empty());
    let standings = (!groups.is_empty()).then(|| view! { <StandingsBlock groups /> });
    // Lay the stages out in a grid so plain standings tables pair up two-across on
    // desktop (they're far narrower than the page); bracket/Swiss stages span the
    // full width. Collapses to a single column on narrow viewports.
    let combos = others
        .into_iter()
        .map(move |e| view! { <EventStageCombo event=e swiss_grid times=times.get_value() /> })
        .collect_view();
    view! { <div class="event-stages">{standings}{combos}</div> }
}

/// The event's group/division standings under one shared "Standings" heading, each
/// table labelled by its group/division name only. The single toggle reveals or
/// hides them all together.
#[component]
fn StandingsBlock(groups: Vec<EventInfo>) -> impl IntoView {
    // One reveal for the whole block, keyed off the first group's id (stable).
    let key = format!(
        "st:{}",
        groups.first().map(|e| e.tournament_id).unwrap_or(0)
    );
    let (revealed, toggle) = section_reveal(key);
    let cells = groups
        .into_iter()
        .map(move |e| {
            let EventInfo {
                tournament_id,
                stage,
                sport,
                standings,
                ..
            } = e;
            view! {
                <div class="event-extra spy standings-cell" id=format!("stage-{tournament_id}")>
                    <h2 class="stage-head">{stage}</h2>
                    <StandingsTable rows=standings sport shared=revealed />
                </div>
            }
        })
        .collect_view();
    view! {
        <div class="event-extra event-extra-wide standings-lead">
            <div class="section-head">
                <button
                    class="section-title section-toggle"
                    class:on=move || revealed.get()
                    title=move || if revealed.get() { "Hide the standings" } else { "Show the standings" }
                    aria-expanded=move || if revealed.get() { "true" } else { "false" }
                    on:click=toggle
                >
                    "Standings"
                </button>
                {move || (!revealed.get()).then(|| view! { <span class="section-hint">"hidden"</span> })}
            </div>
        </div>
        {cells}
    }
}

/// Internal event page: the event's standings/bracket, its full schedule, and a
/// link out to the event's Liquipedia/official page.
#[component]
pub(crate) fn EventPage() -> impl IntoView {
    let params = use_params::<EventParams>();
    let league = move || {
        params
            .get()
            .ok()
            .map(|p| dec_segment(&p.league))
            .unwrap_or_default()
    };
    let hour24 = use_context::<RwSignal<bool>>().expect("hour24 context");
    let tz = use_context::<RwSignal<String>>().expect("tz context");
    // The earlier/later expanders + sport mode drive the windowed view of a
    // traditional-sport event (e.g. MLB), which caps its forward horizon the way
    // the homepage does instead of dumping the whole season.
    let earlier = use_context::<EarlierDays>().expect("earlier context").0;
    let later = use_context::<LaterDays>().expect("later context").0;
    let sport_mode = use_context::<SportMode>().expect("sport mode context").0;
    let schedule = Resource::new(
        move || (league(), tz.get(), hour24.get()),
        |(lg, z, h)| async move { get_event_schedule(lg, z, h).await },
    );
    let stages = Resource::new(league, |lg| async move {
        if lg.is_empty() {
            Ok(Vec::new())
        } else {
            get_event_stages(lg).await
        }
    });
    // An F1 GP event page shows each finished session's full finishing order,
    // fetched on demand once the schedule reveals which (season, round) it is.
    let f1_results = Resource::new(
        move || {
            schedule
                .get()
                .and_then(Result::ok)
                .and_then(|s| f1_season_round(&s))
        },
        |sr| async move {
            match sr {
                Some((season, round)) => get_f1_results(season, round).await.unwrap_or_default(),
                None => Vec::new(),
            }
        },
    );
    // The championship standings as of this GP's round (same source key as the
    // results), shown below them on an F1 event page.
    let f1_standings = Resource::new(
        move || {
            schedule
                .get()
                .and_then(Result::ok)
                .and_then(|s| f1_season_round(&s))
        },
        |sr| async move {
            match sr {
                Some((season, round)) => get_f1_standings(season, round).await.unwrap_or_default(),
                None => F1Standings::default(),
            }
        },
    );
    // WRC/WEC/MotoGP event pages show the series' current championship standings
    // (from the ocblacktop poller cache); empty for F1 / non-motorsport events.
    let motor_standings = Resource::new(
        move || {
            schedule
                .get()
                .and_then(Result::ok)
                .map(|s| motor_series(&s))
                .unwrap_or_default()
        },
        |lg| async move {
            if lg.is_empty() {
                MotorStandings::default()
            } else {
                get_motor_standings(lg).await.unwrap_or_default()
            }
        },
    );
    // WRC/WEC/MotoGP: each finished session's results (and the WRC overall),
    // shown inline like an F1 GP's. Keyed on the edition name, and only fetched
    // for an actual motorsport series (empty edition string → F1 / non-motor).
    let motor_results = Resource::new(
        move || {
            schedule
                .get()
                .and_then(Result::ok)
                .filter(|s| !motor_series(s).is_empty())
                .map(|_| league())
                .unwrap_or_default()
        },
        |ev| async move {
            if ev.is_empty() {
                Vec::new()
            } else {
                get_motor_results(ev).await.unwrap_or_default()
            }
        },
    );
    setup_autorefresh(schedule);

    // The pinned "up next" bar lives inside the content subtree that gets rebuilt
    // wholesale on every refetch. Hoist its scroll-visibility signals up here (the
    // page body runs once, the subtree doesn't) so a refreshed bar mounts in its
    // last-known state rather than flashing back to visible. See `UpNextSeen`.
    provide_context(UpNextSeen {
        day: RwSignal::new(false),
        foot: RwSignal::new(false),
    });

    // Keep the global sport mode in sync with the event being viewed, so the
    // header toggle, footer, and the windowing controls agree with the data
    // (you reach an MLB event from MLB mode, but a direct link should match too).
    // Client-only (Effects don't run on SSR), and it never persists the choice.
    Effect::new(move |_| {
        if let Some(Ok(s)) = schedule.get() {
            sport_mode.set(schedule_is_traditional(&s));
        }
    });

    // Render the whole page from inside one async boundary (awaiting both
    // resources), mirroring the match-detail page — so nothing reactive sits at
    // the synchronous top level to throw off hydration. Transition, not
    // Suspense: the header button and 60s auto-refresh refetch the schedule
    // resource in place, and Transition keeps the current page on screen while
    // it reloads (Suspense would drop back to the fallback and flash).
    view! {
        <Transition fallback=|| {
            view! {
                <article class="detail">
                    <A href="/">"← schedule"</A>
                    <p class="loading">"loading…"</p>
                </article>
            }
        }>
            {move || {
                let sched = schedule.get();
                let stage_list = stages.get().and_then(Result::ok).unwrap_or_default();
                let lg_name = league();
                match sched {
                    Some(Ok(mut s)) => {
                        // The event's last match is the reveal "end" for its
                        // standings/bracket reveals — they prune ~a week past it.
                        let reveal_end = s
                            .days
                            .iter()
                            .flat_map(|d| &d.leagues)
                            .flat_map(|lg| &lg.matches)
                            .map(|m| m.begin_at_ms)
                            .max()
                            .unwrap_or(0);
                        provide_context(RevealEnd(reveal_end));
                        // The route segment is already the full edition name.
                        let title = lg_name.clone();
                        // Reveal-key prefix for this edition's result sections.
                        let motor_key_prefix = format!("motorres:{title}");
                        // The sport this event belongs to (all its matches share
                        // one). Computed before the schedule is windowed below, with
                        // the standings as a fallback for when there are no matches
                        // to read it from (e.g. an off-season team sport).
                        let event_sport = s
                            .days
                            .iter()
                            .flat_map(|d| &d.leagues)
                            .flat_map(|lg| &lg.matches)
                            .map(|m| m.sport)
                            .next()
                            .or_else(|| stage_list.first().map(|e| e.sport));
                        // The event's (short league, series) from any of its groups,
                        // read before windowing. A non-empty series marks a specific
                        // edition (an F1 GP, an esports split) — subscribed to on its
                        // own; an empty series is a whole league/season, subscribed
                        // to via the league scope. Decides the header ★ below.
                        let (event_league, event_series) = s
                            .days
                            .iter()
                            .flat_map(|d| &d.leagues)
                            .next()
                            .map(|lg| (lg.league.clone(), lg.series_name.clone()))
                            .unwrap_or_else(|| (title.clone(), String::new()));
                        // The event's external (Liquipedia/official) link, pulled
                        // from any of its league groups.
                        let url = s
                            .days
                            .iter()
                            .flat_map(|d| &d.leagues)
                            .map(|lg| lg.event_url.clone())
                            .find(|u| !u.is_empty());
                        let link = url
                            .map(|u| {
                                view! {
                                    <p class="event-link">
                                        <a href=u target="_blank" rel="noreferrer">
                                            "event page ↗"
                                        </a>
                                    </p>
                                }
                            });
                        // A motorsport series' official live-stream link(s) (no
                        // per-event broadcast data exists, so it's per-series; F1
                        // lists both its US and rest-of-world services).
                        let watch_opts = motorsport_watch(&motorsport_league(&s));
                        let watch = (!watch_opts.is_empty()).then(|| {
                            let links = watch_opts
                                .iter()
                                .enumerate()
                                .map(|(i, (name, url))| {
                                    view! {
                                        {(i > 0).then(|| view! { <span class="sep">" · "</span> })}
                                        <a href=*url target="_blank" rel="noreferrer">
                                            {format!("{name} ↗")}
                                        </a>
                                    }
                                })
                                .collect_view();
                            view! {
                                <p class="event-link event-watch">"watch · "{links}</p>
                            }
                        });
                        // Each event match's (day, time), so the bracket can date its
                        // rounds and show per-match times from the same data.
                        let mut times: std::collections::HashMap<i64, (String, String)> =
                            std::collections::HashMap::new();
                        for d in &s.days {
                            for lg in &d.leagues {
                                for m in &lg.matches {
                                    times.insert(m.id, (d.day_key.clone(), m.clock_label.clone()));
                                }
                            }
                        }
                        // Jump links to each named stage section, for a long event
                        // page with several stages + a bracket.
                        let nav_items: Vec<(String, String)> = stage_list
                            .iter()
                            .filter(|e| !e.stage.is_empty())
                            .map(|e| (format!("stage-{}", e.tournament_id), e.stage.clone()))
                            .collect();
                        let nav = (!nav_items.is_empty()).then(|| {
                            view! {
                                <nav class="event-nav">
                                    <span class="event-nav-label">"jump to"</span>
                                    <a href="#sched">"schedule"</a>
                                    {nav_items
                                        .into_iter()
                                        .map(|(anchor, name)| {
                                            view! { <a href=format!("#{anchor}")>{name}</a> }
                                        })
                                        .collect_view()}
                                </nav>
                            }
                        });
                        // Traditional sports (MLB) cap their forward horizon like
                        // the homepage: window the days to [today - earlier,
                        // today + later-extended] and show the earlier/later
                        // expanders. Reading earlier/later here re-windows when
                        // they change. Esports events show their full history.
                        let windowed = schedule_needs_window(&s);
                        if windowed {
                            let (lo, hi) =
                                trad_day_bounds(&s.today_key, earlier.get(), later.get());
                            s.days.retain(|d| {
                                d.day_key.as_str() >= lo.as_str()
                                    && d.day_key.as_str() <= hi.as_str()
                            });
                        }
                        // A dotted rule between the schedule and the brackets below —
                        // only when both are present. An empty schedule ("No matches
                        // in this window.") would otherwise leave a stray line above
                        // the standings. Computed after windowing, which can empty it.
                        let sep = (!stage_list.is_empty() && !s.days.is_empty())
                            .then(|| view! { <hr class="section-sep" /> });
                        // Read push here (not at the top) so the ★s appear once the
                        // VAPID key resolves after hydration — this closure re-runs.
                        let push = use_context::<RwSignal<Option<String>>>()
                            .is_some_and(|v| v.get().is_some());
                        // (season, round) for an F1 GP — keys its results' reveal.
                        let f1_sr = f1_season_round(&s);
                        // The series ("WRC"/"WEC") if this is one — for its standings.
                        let motor_league = motor_series(&s);
                        // An event page carries a header ★ only while it still has an
                        // upcoming session — a reminder can't fire on a finished or
                        // live one (same rule as the per-match ★). A wholly-finished
                        // or only-live event shows the bare title. When the ★ does
                        // show, it's scoped to what the page is about: a single-league
                        // team sport (MLB/NHL/NBA/NFL) subscribes to the whole sport —
                        // its league is 1:1 with the sport, the same scope/key as the
                        // sport tab's ★ (its schedule drops the per-day league ★). A
                        // specific edition (an F1 GP, a WEC/WRC round, an esports split
                        // — anything with a series) subscribes to just that edition. A
                        // whole league/season (no series) uses the league scope, in
                        // lock-step with the home page's league ★. The schedule's
                        // per-day league ★s are suppressed on the event page, so this
                        // one stands in for them.
                        let star_sport = event_sport.unwrap_or_default();
                        let trad_single = event_sport
                            .filter(|sp| sp.traditional() && !sp.has_sub_leagues());
                        let title_head = if !event_has_upcoming(&s) {
                            view! { <h1 class="detail-title">{title.clone()}</h1> }.into_any()
                        } else if let Some(sp) = trad_single {
                            view! {
                                <div class="event-title-head">
                                    <SubscribeStar kind="sport" sport=sp value=sp.slug().to_string() />
                                    <h1 class="detail-title">{title.clone()}</h1>
                                </div>
                            }
                            .into_any()
                        } else if event_series.is_empty() {
                            view! {
                                <div class="event-title-head">
                                    <SubscribeStar kind="league" sport=star_sport value=event_league.clone() />
                                    <h1 class="detail-title">{title.clone()}</h1>
                                </div>
                            }
                            .into_any()
                        } else {
                            view! {
                                <div class="event-title-head">
                                    <SubscribeStar kind="event" sport=star_sport value=title.clone() />
                                    <h1 class="detail-title">{title.clone()}</h1>
                                </div>
                            }
                            .into_any()
                        };
                        // F1 GP pages get their own jump-to nav (schedule + each
                        // session that has results + standings), built from the
                        // results as they load. Empty for non-F1 events.
                        let f1_nav = move || {
                            f1_sr?;
                            let sessions: Vec<String> = f1_results
                                .get()
                                .unwrap_or_default()
                                .into_iter()
                                .map(|r| r.session)
                                .collect();
                            let has_standings = f1_standings
                                .get()
                                .is_some_and(|s| !s.drivers.is_empty() || !s.constructors.is_empty());
                            if sessions.is_empty() && !has_standings {
                                return None;
                            }
                            let links = sessions
                                .into_iter()
                                .map(|s| {
                                    let anchor = f1_anchor(&s);
                                    view! { <a href=format!("#{anchor}")>{s.to_lowercase()}</a> }
                                })
                                .collect_view();
                            Some(view! {
                                <nav class="event-nav">
                                    <span class="event-nav-label">"jump to"</span>
                                    <a href="#sched">"schedule"</a>
                                    {links}
                                    {has_standings
                                        .then(|| view! { <a href="#f1sec-standings">"standings"</a> })}
                                </nav>
                            })
                        };
                        // WRC/WEC/MotoGP event pages get a jump-to nav (schedule +
                        // each result section + standings), built as results load.
                        // Empty for F1 / non-motorsport editions.
                        let motor_nav = {
                            let motor_league = motor_league.clone();
                            move || {
                                if motor_league.is_empty() {
                                    return None;
                                }
                                let titles: Vec<String> = motor_results
                                    .get()
                                    .unwrap_or_default()
                                    .into_iter()
                                    .map(|r| r.title)
                                    .collect();
                                let has_standings = motor_standings
                                    .get()
                                    .is_some_and(|s| !s.tables.is_empty());
                                if titles.is_empty() && !has_standings {
                                    return None;
                                }
                                let links = titles
                                    .into_iter()
                                    .map(|t| {
                                        let anchor = motor_anchor(&t);
                                        view! { <a href=format!("#{anchor}")>{t.to_lowercase()}</a> }
                                    })
                                    .collect_view();
                                Some(view! {
                                    <nav class="event-nav">
                                        <span class="event-nav-label">"jump to"</span>
                                        <a href="#sched">"schedule"</a>
                                        {links}
                                        {has_standings
                                            .then(|| view! { <a href="#motorsec-standings">"standings"</a> })}
                                    </nav>
                                })
                            }
                        };
                        view! {
                            <article class="detail">
                                <A href="/">"← schedule"</A>
                                {title_head}
                                {link}
                                {watch}
                                {nav}
                                {f1_nav}
                                {motor_nav}
                                <div id="sched" class="spy">
                                    {render_schedule(s, false, push, true, windowed)}
                                </div>
                                {sep}
                                <EventStages stages=stage_list times=times />
                                // F1: each finished session's full finishing order
                                // (empty for non-F1 events / upcoming races).
                                {move || {
                                    let (season, round) = f1_sr.unwrap_or((0, 0));
                                    f1_results
                                        .get()
                                        .filter(|r| !r.is_empty())
                                        .map(move |results| {
                                            view! { <F1Results results=results season=season round=round /> }
                                        })
                                }}
                                // F1: the championship standings as of this round
                                // (empty for non-F1 events / before any race).
                                {move || {
                                    let (season, round) = f1_sr.unwrap_or((0, 0));
                                    f1_standings
                                        .get()
                                        .filter(|s| {
                                            !s.drivers.is_empty() || !s.constructors.is_empty()
                                        })
                                        .map(move |standings| {
                                            view! {
                                                <F1StandingsView
                                                    standings=standings
                                                    season=season
                                                    round=round
                                                />
                                            }
                                        })
                                }}
                                // WRC/WEC/MotoGP: each finished session's results
                                // (and the WRC overall), inline like F1's. Empty
                                // for F1 / non-motorsport / not-yet-finished events.
                                {move || {
                                    let key_prefix = motor_key_prefix.clone();
                                    motor_results
                                        .get()
                                        .filter(|r| !r.is_empty())
                                        .map(move |results| {
                                            view! {
                                                <MotorResultsView results=results key_prefix=key_prefix />
                                            }
                                        })
                                }}
                                // WRC/WEC: the series' championship tables.
                                {move || {
                                    let lg = motor_league.clone();
                                    if lg.is_empty() {
                                        return None;
                                    }
                                    motor_standings
                                        .get()
                                        .filter(|s| !s.tables.is_empty())
                                        .map(move |standings| {
                                            view! { <MotorStandingsView standings=standings league=lg /> }
                                        })
                                }}
                                // TFT: the lobby standings + final placements, side
                                // by side (from the Liquipedia poller cache; nothing
                                // for non-TFT events / before results exist).
                                <TftEventResults event=Signal::derive(move || {
                                    let lg = league();
                                    if lg.starts_with("TFT") { lg } else { String::new() }
                                }) />
                            </article>
                        }
                            .into_any()
                    }
                    _ => {
                        view! {
                            <article class="detail">
                                <A href="/">"← schedule"</A>
                                <p class="error">"Failed to load event."</p>
                            </article>
                        }
                            .into_any()
                    }
                }
            }}
        </Transition>
    }
}
