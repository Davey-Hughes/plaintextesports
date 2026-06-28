//! The schedule view: filter tabs/chips, the day/league sections, the up-next
//! bar, and the match-row rendering + progressive-reveal machinery.
use crate::app::*;

/// Keeps the schedule filter (games + events) in sync with the URL query so a
/// filtered view is shareable — `?g=cs2&e=<event>`; no filter ⇒ a clean URL. On
/// load it initialises the filter from the query (falling back to the saved
/// preference). Renders nothing. Must live inside the `Router`.
#[component]
pub(crate) fn FilterUrlSync() -> impl IntoView {
    let games = use_context::<Games>().expect("games context").0;
    let leagues = use_context::<Leagues>().expect("leagues context").0;
    let traditional = use_context::<SportMode>().expect("sport mode context").0;
    let _ = (games, leagues, traditional);
    #[cfg(feature = "hydrate")]
    {
        use leptos_router::hooks::{use_location, use_navigate, use_query_map};
        use leptos_router::NavigateOptions;

        fn parse_set(v: &str) -> HashSet<String> {
            v.split(',')
                .filter(|p| !p.is_empty())
                .map(str::to_string)
                .collect()
        }

        let query = use_query_map();
        let location = use_location();
        let navigate = use_navigate();

        // Initialise once: a query param wins (a shared link), else the saved
        // preference. (`get_untracked` ⇒ this effect runs a single time.)
        Effect::new(move |_| {
            let q = query.get_untracked();
            let g_param = q.get("g").filter(|v| !v.is_empty());
            let e_param = q.get("e").filter(|v| !v.is_empty());
            // The URL filter is already applied at render (`initial_games`/
            // `initial_leagues`). Only restore the saved (localStorage) filter when
            // the URL carries NO filter at all; if it carries any, the URL is
            // authoritative — so a shared/refreshed ?e=… link isn't polluted by a
            // stale saved sport filter, and nothing changes post-hydration (no flash
            // and no hydration churn).
            if g_param.is_none() && e_param.is_none() {
                games.set(load_str_set(keys::GAMES));
                leagues.set(load_str_set(keys::LEAGUES));
            }
            // Sport mode is already resolved at render (`initial_sport_mode`); re-
            // apply the URL's view here so an in-app navigation to a ?mode/?g link
            // still lands in the right mode. Same precedence, same value on load.
            if let Some(m) = q.get("mode").filter(|v| !v.is_empty()) {
                traditional.set(m == "sports");
            } else if let Some(v) = &g_param {
                let slugs = parse_set(v);
                if slugs.iter().any(|s| Sport::from_filter(s).is_some()) {
                    traditional.set(
                        slugs
                            .iter()
                            .any(|s| Sport::from_filter(s).is_some_and(Sport::traditional)),
                    );
                }
            }
        });

        // Mirror the active filter into the query, but only on the schedule pages
        // (home / day) where the filter applies — never touch other pages' URLs.
        Effect::new(move |_| {
            let path = location.pathname.get();
            let g = games.get();
            let l = leagues.get();
            let trad = traditional.get();
            if !(path == "/" || path.is_empty() || path.starts_with("/day")) {
                return;
            }
            let enc = |set: &HashSet<String>| {
                let mut v: Vec<String> = set.iter().cloned().collect();
                v.sort();
                v.iter()
                    .map(|s| {
                        js_sys::encode_uri_component(s)
                            .as_string()
                            .unwrap_or_default()
                    })
                    .collect::<Vec<_>>()
                    .join(",")
            };
            let mut parts = Vec::new();
            // esports is the default, so only the sports mode is spelled out.
            if trad {
                parts.push("mode=sports".to_string());
            }
            if !g.is_empty() {
                parts.push(format!("g={}", enc(&g)));
            }
            if !l.is_empty() {
                parts.push(format!("e={}", enc(&l)));
            }
            let want = if parts.is_empty() {
                String::new()
            } else {
                format!("?{}", parts.join("&"))
            };
            // Skip a redundant navigation (also breaks any feedback loop).
            if location.search.get_untracked() == want {
                return;
            }
            let url = format!("{path}{want}");
            navigate(
                &url,
                NavigateOptions {
                    replace: true,
                    scroll: false,
                    ..Default::default()
                },
            );
        });
    }
}

/// Standings + bracket shown beneath the schedule when exactly one event is
/// selected (ambiguous for multiple, so only a single selection shows it).
#[component]
pub(crate) fn EventSection(leagues: RwSignal<HashSet<String>>) -> impl IntoView {
    // Only when exactly one event is selected; then show all of its stages
    // (every Swiss/group standings + playoff bracket), like the event page.
    let stages = Resource::new(
        move || {
            leagues.with(|s| {
                if s.len() == 1 {
                    s.iter().next().cloned().unwrap_or_default()
                } else {
                    String::new()
                }
            })
        },
        |lg| async move {
            if lg.is_empty() {
                Ok(Vec::new())
            } else {
                get_event_stages_by_league(lg).await
            }
        },
    );
    view! {
        <Suspense>
            {move || {
                stages
                    .get()
                    .and_then(Result::ok)
                    .filter(|v| !v.is_empty())
                    .map(|v| view! { <EventStages stages=v /> })
            }}
        </Suspense>
    }
}

/// Bottom-of-schedule control for traditional sports: a "show later days ›"
/// expander that reveals more future days, hidden once the forward window hits
/// `TRAD_FORWARD_MAX`. Renders nothing in esports mode.
#[component]
pub(crate) fn LaterControl() -> impl IntoView {
    let traditional = use_context::<SportMode>().expect("sport mode context").0;
    let later = use_context::<LaterDays>().expect("later context").0;
    let on_later = move |_| later.update(|n| *n += 2);
    move || {
        let at_cap =
            crate::types::TRAD_FORWARD_DAYS + later.get() >= crate::types::TRAD_FORWARD_MAX;
        (traditional.get() && !at_cap).then(|| {
            view! {
                <div class="history-bar history-bar-later">
                    <button class="linkish" on:click=on_later>"show later days ›"</button>
                </div>
            }
        })
    }
}

/// Top-of-schedule control: a "‹ show earlier days" expander by default, or the
/// active calendar range with a "clear" when one is selected.
#[component]
pub(crate) fn EarlierControl() -> impl IntoView {
    let range = use_context::<DateRange>().expect("range context").0;
    let earlier = use_context::<EarlierDays>().expect("earlier context").0;
    let later = use_context::<LaterDays>().expect("later context").0;

    let on_earlier = move |_| earlier.update(|n| *n += 3);
    let on_reset = move |_| earlier.set(0);
    let on_clear = move |_| {
        earlier.set(0);
        later.set(0);
        range.set(None);
        #[cfg(feature = "hydrate")]
        save_range(None);
    };

    view! {
        <div class="history-bar">
            {move || {
                if let Some((s, e)) = range.get() {
                    view! {
                        <span class="history-label">{format!("{s} → {e}")}</span>
                        <button class="linkish" on:click=on_clear>"clear"</button>
                    }
                        .into_any()
                } else if earlier.get() > 0 {
                    view! {
                        <button class="linkish" on:click=on_earlier>"‹ show earlier days"</button>
                        <button class="linkish" on:click=on_reset>"reset"</button>
                    }
                        .into_any()
                } else {
                    view! {
                        <button class="linkish" on:click=on_earlier>"‹ show earlier days"</button>
                    }
                        .into_any()
                }
            }}
        </div>
    }
}

/// A row of filter tabs: the esports games (CS2/LoL) when `traditional_row` is
/// false, or the traditional sports (MLB/NHL/.../F1) when true. Each tab carries
/// a subscribe ★ for the whole sport and clicks to (de)select it; the row hides
/// itself in the other top-level mode. The `games`/`leagues` filter sets are
/// shared across both rows, so a selection narrows the schedule.
#[component]
pub(crate) fn FilterTabs(
    games: RwSignal<HashSet<String>>,
    leagues: RwSignal<HashSet<String>>,
    traditional_row: bool,
) -> impl IntoView {
    let traditional = use_context::<SportMode>().expect("sport mode context").0;
    // Filters are multi-select and additive: no selection means "all". Click a
    // sport to include it; click again to drop it.
    let toggle = move |value: &'static str| {
        games.update(|g| {
            if !g.remove(value) {
                g.insert(value.to_string());
            }
        });
        #[cfg(feature = "hydrate")]
        save_str_set(keys::GAMES, &games.get_untracked());
    };
    // A sport filter tab with an embedded ★ that subscribes to the whole sport.
    // The label clicks to filter; the ★ clicks to (un)subscribe.
    let with_star = move |sport: Sport, label: &'static str, value: &'static str| {
        view! {
            <span class="tab tab-with-star" class:active=move || games.with(|g| g.contains(value))>
                <button class="tab-label" on:click=move |_| toggle(value)>
                    {label}
                </button>
                <SubscribeStar kind="sport" sport=sport value=value.to_string() />
            </span>
        }
    };
    // A "clear" appears once any sport/event filter is active and resets them all.
    let any_active = move || games.with(|g| !g.is_empty()) || leagues.with(|l| !l.is_empty());
    let clear = move |_| {
        games.update(HashSet::clear);
        leagues.update(HashSet::clear);
        #[cfg(feature = "hydrate")]
        {
            save_str_set(keys::GAMES, &games.get_untracked());
            save_str_set(keys::LEAGUES, &leagues.get_untracked());
        }
    };
    // This row's games, in canonical order; the row hides when the active mode
    // isn't this row's (the two rows are exact inverses).
    let tabs = Sport::ALL
        .iter()
        .filter(|g| g.traditional() == traditional_row)
        .map(|g| with_star(*g, g.label(), g.slug()))
        .collect_view();
    let hidden = move || traditional.get() != traditional_row;
    view! {
        <div class="tabs" class:tabs-off=hidden>
            <div class="tabs-list">{tabs}</div>
            {move || {
                any_active()
                    .then(|| view! { <button class="filter-clear" on:click=clear>"clear"</button> })
            }}
        </div>
    }
}

#[component]
pub(crate) fn LeagueChips(
    leagues: Vec<(String, Sport)>,
    selected: RwSignal<HashSet<String>>,
) -> impl IntoView {
    // Multi-select: no active chip means all events. Click a chip's label to
    // include its league in the filter; click again to drop it. The embedded ★
    // subscribes to the whole chip via the `league` scope. For a single-edition
    // league like MLB that's the same match set as its "MLB" event, so the chip
    // ★ and the home league-header ★ share one key (`league|mlb|MLB`) and stay
    // in lock-step; for F1/WEC it spans every round, distinct from one GP's event.
    let chips = leagues
        .into_iter()
        .map(|(name, sport)| {
            let lc = league_color_class(&name);
            let sel_name = name.clone();
            let click_name = name.clone();
            let sub_value = name.clone();
            let is_active = move || selected.with(|s| s.contains(&sel_name));
            view! {
                <span class=format!("chip chip-with-star {lc}") class:active=is_active>
                    <button
                        class="chip-label"
                        on:click=move |_| {
                            selected
                                .update(|s| {
                                    if !s.remove(&click_name) {
                                        s.insert(click_name.clone());
                                    }
                                });
                            #[cfg(feature = "hydrate")]
                            save_str_set(keys::LEAGUES, &selected.get_untracked());
                        }
                    >
                        {name}
                    </button>
                    <SubscribeStar kind="league" sport=sport value=sub_value />
                </span>
            }
        })
        .collect_view();

    view! { <div class="chips">{chips}</div> }
}

#[component]
pub(crate) fn ScheduleSection(
    resource: Resource<Result<ScheduleView, ServerFnError>>,
    games: RwSignal<HashSet<String>>,
    leagues: RwSignal<HashSet<String>>,
    show_nav: bool,
) -> impl IntoView {
    let traditional = use_context::<SportMode>().expect("sport mode context").0;
    // In traditional mode, the single league the filters narrow the view to (its
    // tab, or a single soccer chip), mirroring the esports single-event bracket,
    // plus the F1 season when narrowed to F1. Empty/None when ambiguous (several
    // leagues) or in esports mode.
    //
    // Deriving these reads the schedule resource, which must happen inside an
    // effect — a bare memo reading a resource warns in hydrate about reading
    // outside <Suspense>. The effect writes plain signals the standings below key
    // off, and only on change so a background schedule refresh doesn't churn them.
    let single_league = RwSignal::new(String::new());
    let f1_home_season = RwSignal::new(None::<i64>);
    Effect::new(move |_| {
        let (league, season) = if !traditional.get() {
            (String::new(), None)
        } else if let Some(Ok(s)) = resource.get() {
            let available: Vec<String> = leagues_for_games(&s, &games.get())
                .into_iter()
                .map(|(l, _)| l)
                .collect();
            let selected = leagues.get();
            let effective: Vec<String> = if selected.is_empty() {
                available
            } else {
                available
                    .into_iter()
                    .filter(|l| selected.contains(l))
                    .collect()
            };
            match effective.as_slice() {
                [only] if only == "F1" => (
                    "F1".to_string(),
                    f1_season_round(&s).map(|(season, _)| season),
                ),
                [only] => (only.clone(), None),
                _ => (String::new(), None),
            }
        } else {
            (String::new(), None)
        };
        if single_league.get_untracked() != league {
            single_league.set(league);
        }
        if f1_home_season.get_untracked() != season {
            f1_home_season.set(season);
        }
    });
    // The team sports show their division/group tables; the motorsport series
    // (F1/WRC/WEC/MotoGP) have no such tables — they get championship standings
    // instead (handled below), so they're excluded here.
    let trad_standings_league = Memo::new(move |_| {
        let l = single_league.get();
        if matches!(l.as_str(), "F1" | "WRC" | "WEC" | "MotoGP") {
            String::new()
        } else {
            l
        }
    });
    // WRC/WEC/MotoGP (the non-F1 motorsport series) — their championship tables
    // from the ocblacktop poller cache, when the filters narrow to one of them.
    let motor_home_league = Memo::new(move |_| {
        let l = single_league.get();
        if matches!(l.as_str(), "WRC" | "WEC" | "MotoGP") {
            l
        } else {
            String::new()
        }
    });
    let motor_home_standings = Resource::new(
        move || motor_home_league.get(),
        |lg| async move {
            if lg.is_empty() {
                MotorStandings::default()
            } else {
                get_motor_standings(lg).await.unwrap_or_default()
            }
        },
    );
    // Round 0 has no standings, so `fetch_standings` falls back to the latest
    // completed round — i.e. the current championship picture. `f1_home_season`
    // is set by the effect above.
    let f1_home_standings = Resource::new(
        move || f1_home_season.get(),
        |season| async move {
            match season {
                Some(season) => get_f1_standings(season, 0).await.unwrap_or_default(),
                None => F1Standings::default(),
            }
        },
    );
    view! {
        // Transition (not Suspense) so re-keying the resource — e.g. "show
        // earlier days" extends the range — keeps the current schedule on screen
        // while the next loads, instead of blanking the page (which flashed the
        // footer up to the top).
        <Transition fallback=|| view! { <p class="loading">"loading…"</p> }>
            {move || {
                let trad = traditional.get();
                resource
                    .get()
                    .map(|res| match res {
                        Ok(s) => {
                            let games_set = games.get();
                            // Competition chips within the current view. In esports
                            // these are the CS2/LoL events; in sports mode they're
                            // shown when the view spans more than one league (e.g.
                            // soccer's Premier League + World Cup), or when the sport
                            // sub-categorises by league (soccer, motorsport → its F1
                            // series), so a plain single-league sport (MLB, …) doesn't
                            // get a chip that just repeats its tab.
                            let available = leagues_for_games(&s, &games_set);
                            let has_sub = s
                                .days
                                .iter()
                                .flat_map(|d| &d.leagues)
                                .flat_map(|lg| &lg.matches)
                                .any(|m| {
                                    (games_set.is_empty() || games_set.contains(m.sport.slug()))
                                        && m.sport.has_sub_leagues()
                                });
                            let show_chips = if trad {
                                !available.is_empty() && (available.len() > 1 || has_sub)
                            } else {
                                !available.is_empty()
                            };
                            // Only apply the chip filter when chips are on screen, so
                            // a remembered selection can't silently empty a single-
                            // competition sport when you switch to its tab.
                            let leagues_filter =
                                if show_chips || !trad { leagues.get() } else { HashSet::new() };
                            let filtered = filter_schedule(s, &games_set, &leagues_filter);
                            // Show ★ reminder buttons only when Web Push is configured.
                            let push = use_context::<RwSignal<Option<String>>>()
                                .is_some_and(|v| v.get().is_some());
                            view! {
                                {show_chips
                                    .then(|| view! { <LeagueChips leagues=available selected=leagues /> })}
                                {render_schedule(filtered, show_nav, push, false, false)}
                            }
                                .into_any()
                        }
                        Err(_) => {
                            view! { <p class="error">"Failed to load schedule."</p> }.into_any()
                        }
                    })
            }}
        </Transition>
        // When exactly one esports event is selected, show its standings/bracket.
        {move || (!traditional.get()).then(|| view! { <EventSection leagues=leagues /> })}
        // Likewise, a single traditional league shows its division/group tables.
        <TraditionalStandings league=trad_standings_league />
        // F1 has no league table — when narrowed to it, show the current
        // drivers' + constructors' championship standings instead.
        <Suspense>
            {move || {
                let season = f1_home_season.get()?;
                let s = f1_home_standings.get()?;
                (!s.drivers.is_empty() || !s.constructors.is_empty()).then(|| {
                    let round = s.round;
                    view! { <F1StandingsView standings=s season=season round=round /> }
                })
            }}
        </Suspense>
        // WRC/WEC get their championship tables (from ocblacktop) the same way.
        <Suspense>
            {move || {
                let lg = motor_home_league.get();
                if lg.is_empty() {
                    return None;
                }
                let s = motor_home_standings.get()?;
                (!s.tables.is_empty()).then(|| view! { <MotorStandingsView standings=s league=lg /> })
            }}
        </Suspense>
    }
}

/// Distinct league names among groups whose sport passes the filter (empty set =
/// all games), in first-appearance order. Drives the event chip list, so the
/// chips reflect the selected games.
pub(crate) fn leagues_for_games(s: &ScheduleView, games: &HashSet<String>) -> Vec<(String, Sport)> {
    let mut out: Vec<(String, Sport)> = Vec::new();
    for day in &s.days {
        for lg in &day.leagues {
            let Some(sport) = lg.matches.first().map(|m| m.sport) else {
                continue;
            };
            let game_ok = games.is_empty() || games.contains(sport.slug());
            if game_ok && !out.iter().any(|(l, _)| l == &lg.league) {
                out.push((lg.league.clone(), sport));
            }
        }
    }
    // Tidy the motorsport series chips into a deliberate order (F1 · MotoGP · WEC ·
    // WRC) within the slots they already occupy, leaving every other chip in its
    // first-appearance position.
    let motor_slots: Vec<usize> = out
        .iter()
        .enumerate()
        .filter(|(_, (_, sp))| *sp == Sport::Motorsport)
        .map(|(i, _)| i)
        .collect();
    let mut motor: Vec<(String, Sport)> = motor_slots.iter().map(|&i| out[i].clone()).collect();
    motor.sort_by_key(|(l, _)| crate::types::motorsport_league_rank(l));
    for (&slot, val) in motor_slots.iter().zip(motor) {
        out[slot] = val;
    }
    out
}

/// Keep only groups matching the selected games AND events (empty set = no
/// constraint on that axis); drop days left empty.
pub(crate) fn filter_schedule(
    mut s: ScheduleView,
    games: &HashSet<String>,
    leagues: &HashSet<String>,
) -> ScheduleView {
    if games.is_empty() && leagues.is_empty() {
        return s;
    }
    for day in &mut s.days {
        day.leagues.retain(|lg| {
            let game_ok = games.is_empty()
                || lg
                    .matches
                    .first()
                    .is_some_and(|m| games.contains(m.sport.slug()));
            let league_ok = leagues.is_empty() || leagues.contains(&lg.league);
            game_ok && league_ok
        });
    }
    s.days.retain(|d| !d.leagues.is_empty());
    s
}

/// A compact "up next" bar pinned to the bottom of the event page, previewing
/// the current/next day's matches. Clicking it scrolls to that day in the list;
/// it hides once that day scrolls into view, so it never duplicates what's shown.
#[component]
pub(crate) fn UpNextBar(day: DayGroup) -> impl IntoView {
    let anchor = format!("day-{}", day.day_key);
    // The bar hides when its target day scrolls into view (you're already there)
    // or when the footer does (so it never covers the footer at the page bottom).
    let day_seen = RwSignal::new(false);
    let foot_seen = RwSignal::new(false);
    // Flatten the day's matches (time + teams), capped so the bar stays slim.
    const CAP: usize = 4;
    let all: Vec<MatchView> = day.leagues.into_iter().flat_map(|lg| lg.matches).collect();
    // "Up next" while the day still has unplayed matches; "Latest" once it's a
    // finished event and we're pointing at its most recent day instead.
    let upcoming = all
        .iter()
        .any(|m| matches!(m.status, MatchStatus::Live | MatchStatus::Upcoming));
    let label = format!(
        "{} · {}",
        if upcoming {
            "▸ Up next"
        } else {
            "▸ Latest"
        },
        day.day_label
    );
    let more = all.len().saturating_sub(CAP);
    let rows = all
        .into_iter()
        .take(CAP)
        .map(|m| {
            let muid = crate::types::match_uid(m.sport, m.id);
            let mpath = crate::types::match_path(m.sport, m.id);
            let sep = versus_sep(m.sport);
            // F1 (single-entity) has no opponent: show just the session label, with
            // no "at"/"vs" separator and no empty second team.
            let teams = if m.sport.single_entity() {
                view! {
                    <span class="upnext-team upnext-solo">{m.team_a.label}</span>
                }
                .into_any()
            } else {
                view! {
                    <span class="upnext-team">{m.team_a.label}</span>
                    <span class="upnext-vs">{sep}</span>
                    <span class="upnext-team">{m.team_b.label}</span>
                }
                .into_any()
            };
            // A match scrolls to (and flashes) its row in the schedule when it's on
            // the page — same as a bracket name — and otherwise opens its page.
            // Stop propagation so the bar's own jump-to-day doesn't also fire.
            view! {
                <a
                    class="upnext-match"
                    href=mpath
                    on:click={
                        let muid = muid.clone();
                        move |e: leptos::ev::MouseEvent| {
                            e.stop_propagation();
                            #[cfg(feature = "hydrate")]
                            if flash_match_row(&muid) {
                                e.prevent_default();
                            }
                            #[cfg(not(feature = "hydrate"))]
                            let _ = &muid;
                        }
                    }
                >
                    <span class="upnext-time">{m.clock_label}</span>
                    {teams}
                </a>
            }
        })
        .collect_view();

    let jump = {
        let anchor = anchor.clone();
        move |_| {
            let _ = &anchor;
            #[cfg(feature = "hydrate")]
            if let Some(el) = web_sys::window()
                .and_then(|w| w.document())
                .and_then(|d| d.get_element_by_id(&anchor))
            {
                el.scroll_into_view();
            }
        }
    };

    #[cfg(feature = "hydrate")]
    {
        use wasm_bindgen::closure::Closure;
        use wasm_bindgen::JsCast;
        Effect::new(move |_| {
            let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
                return;
            };
            // Observe an element; mirror its visibility into `sig`. The observer
            // keeps firing as long as its callback is alive; leak it for the
            // page's lifetime (web-sys handles can't ride on_cleanup, which
            // requires Send + Sync).
            let observe = |el: web_sys::Element, sig: RwSignal<bool>| {
                let cb = Closure::<dyn FnMut(js_sys::Array)>::new(move |entries: js_sys::Array| {
                    let first = entries
                        .get(0)
                        .unchecked_into::<web_sys::IntersectionObserverEntry>();
                    sig.set(first.is_intersecting());
                });
                if let Ok(obs) = web_sys::IntersectionObserver::new(cb.as_ref().unchecked_ref()) {
                    obs.observe(&el);
                }
                cb.forget();
            };
            if let Some(el) = doc.get_element_by_id(&anchor) {
                observe(el, day_seen);
            }
            if let Some(el) = doc.get_element_by_id("site-footer") {
                observe(el, foot_seen);
            }
        });
    }

    view! {
        <div
            class="upnext"
            class:upnext-hidden=move || day_seen.get() || foot_seen.get()
            on:click=jump
            title="Jump to these matches"
        >
            <div class="upnext-head">{label}</div>
            <div class="upnext-list">
                {rows}
                {(more > 0)
                    .then(|| view! { <span class="upnext-more">{format!("+{more} more")}</span> })}
            </div>
        </div>
    }
}

pub(crate) fn render_schedule(
    s: ScheduleView,
    show_nav: bool,
    push: bool,
    event_mode: bool,
    windowed: bool,
) -> impl IntoView {
    let ScheduleView {
        days,
        today_key,
        fetched_label,
        stale,
        using_fixture,
        demo_forced,
        date_label,
        prev_date,
        next_date,
        ..
    } = s;
    let today_key = StoredValue::new(today_key);

    let empty = days.iter().all(|d| d.leagues.is_empty());
    // The "‹ show earlier days" / "show later days ›" controls ride the schedule
    // on the homepage (no nav, not an event) and on a windowed event page (a
    // traditional sport, which caps its forward horizon like the homepage). The
    // single-day view has prev/next nav instead.
    let show_earlier = (!show_nav && !event_mode) || windowed;
    let has_days = !days.is_empty();

    let nav = show_nav.then(|| {
        let prev = prev_date.unwrap_or_default();
        let next = next_date.unwrap_or_default();
        let label = date_label.unwrap_or_default();
        view! {
            <nav class="day-nav">
                <A href=format!("/day/{prev}")>"‹ prev"</A>
                <span class="day-nav-label">{label}</span>
                <A href=format!("/day/{next}")>"next ›"</A>
            </nav>
        }
    });

    // On the event page, pick the "current/next" day — the earliest day that
    // still has an unfinished (live/upcoming) match, else the most recent day —
    // to surface in a pinned "up next" bar so it's reachable without scrolling
    // past the whole history.
    let upnext_day: Option<DayGroup> = event_mode
        .then(|| {
            days.iter()
                .position(|d| {
                    d.leagues
                        .iter()
                        .flat_map(|lg| &lg.matches)
                        .any(|m| matches!(m.status, MatchStatus::Live | MatchStatus::Upcoming))
                })
                .or_else(|| days.len().checked_sub(1))
                .and_then(|i| days.get(i).cloned())
        })
        .flatten();

    // A league head's ★ tracks the whole EVENT EDITION (an F1 GP weekend, an IEM
    // tournament — keyed by sport + league + series), not a single day of it. So
    // a day whose own sessions are all finished still shows the ★ while a later
    // day of the same edition is upcoming (Friday practice is done but Sunday's
    // race isn't). Collect every edition with an upcoming match anywhere in the
    // loaded window; a head whose edition is absent is wholly over → grey bar.
    let editions_with_upcoming: HashSet<(Sport, String, String)> = days
        .iter()
        .flat_map(|d| &d.leagues)
        .filter(|lg| {
            lg.matches
                .iter()
                .any(|m| matches!(m.status, MatchStatus::Upcoming))
        })
        .map(|lg| {
            let sp = lg.matches.first().map(|m| m.sport).unwrap_or_default();
            (sp, lg.league.clone(), lg.series_name.clone())
        })
        .collect();

    let day_sections = days
        .into_iter()
        .enumerate()
        .map(|(idx, d)| {
            let leagues = d
                .leagues
                .into_iter()
                .map(|lg| {
                    // The best-of always shows per row (right of each match); the
                    // event title no longer repeats it.
                    let show_bo = true;
                    let league_name = lg.league.clone();
                    // The group's sport (a league is single-sport) scopes the event
                    // URL and the subscribe key. Read from any of its matches.
                    let sport = lg.matches.first().map(|m| m.sport).unwrap_or_default();
                    // Title the group with the full event name (league + edition,
                    // e.g. "IEM Katowice"); the subscribe key stays the short
                    // league name, while the full name keys the event link.
                    let series = lg.series_name.clone();
                    // The ★ shows while the EDITION (not just this day) still has an
                    // upcoming session to be reminded about; a wholly-finished edition
                    // shows the grey bar. Keyed the same way as the edition set above.
                    let lg_has_upcoming = editions_with_upcoming.contains(&(
                        sport,
                        lg.league.clone(),
                        lg.series_name.clone(),
                    ));
                    // A "wholly over" group: every row finished AND the edition has no
                    // upcoming session (so the header carries no ★). It gets one
                    // continuous grey rail spanning the header *and* the rows (see
                    // `.league-over`), instead of a blank header above the rows' bar.
                    let lg_over = !lg_has_upcoming
                        && !lg.matches.is_empty()
                        && lg.matches.iter().all(|m| matches!(m.status, MatchStatus::Finished));
                    // On the event page the page title already names the event, so we
                    // drop the per-day league header, the colour-coded bar, and show
                    // the best-of per row instead.
                    let league_class = if event_mode {
                        "league league-plain".to_string()
                    } else {
                        let mut c = format!("league {}", league_color_class(&lg.league));
                        if lg_over {
                            c.push_str(" league-over");
                        }
                        c
                    };
                    let head = (!event_mode).then(move || {
                        let display = full_event_name(&league_name, &series);
                        // The full edition name keys its event page, so distinct
                        // editions get distinct URLs.
                        let event_href = event_path(sport, &display);
                        // The ★ subscribes to the whole competition (`league|<name>`).
                        // For a single-edition league like MLB/NHL/NBA/NFL this is the
                        // same match set — and the same key — as its filter chip's ★,
                        // so the two stay in lock-step; for F1/WEC it spans every round.
                        // When the edition is wholly over the ★ is suppressed and the
                        // header just keeps a blank slot the width of the ★ (a
                        // transparent ☆), so the title stays aligned with the starred
                        // groups without drawing anything. The slot is only reserved
                        // when push is on (the ★ is hidden entirely otherwise).
                        let star = if lg_has_upcoming {
                            view! {
                                <SubscribeStar kind="league" sport=sport value=league_name.clone() />
                            }
                            .into_any()
                        } else if push {
                            // Invisible stand-in with the IDENTICAL box to a real ★
                            // (same <button class="sub-star">), so the title lines up
                            // exactly as in starred groups on every browser. A plain
                            // <span> ☆ renders in a different font/width than the
                            // <button> ☆ (notably on Firefox), which nudged the title
                            // off the time column below.
                            view! {
                                <button
                                    class="sub-star league-head-spacer"
                                    aria-hidden="true"
                                    tabindex="-1"
                                >
                                    "☆"
                                </button>
                            }
                            .into_any()
                        } else {
                            ().into_any()
                        };
                        view! {
                            <div class="league-head">
                                {star}
                                <h3 class="league-title">
                                    <A href=event_href>{display}</A>
                                </h3>
                            </div>
                        }
                    });
                    // Group consecutive rows by their side-bar kind so each run of
                    // finished (grey) or live (accent) rows draws ONE continuous bar
                    // (on the wrapping `.bar-run`) instead of per-row `.row-bar`
                    // segments — segments could misalign or show overlap seams at
                    // fractional zoom (Chrome/Firefox). Upcoming/other rows form a
                    // bar-less run (their ☆/placeholder stays per-row).
                    #[derive(PartialEq, Clone, Copy)]
                    enum BarKind {
                        Final,
                        Live,
                        None,
                    }
                    let bar_kind = |s: MatchStatus| match s {
                        MatchStatus::Finished => BarKind::Final,
                        MatchStatus::Live => BarKind::Live,
                        _ => BarKind::None,
                    };
                    let mut runs: Vec<(BarKind, Vec<_>)> = Vec::new();
                    for m in lg.matches {
                        let k = bar_kind(m.status);
                        match runs.last_mut() {
                            Some((rk, ms)) if *rk == k => ms.push(m),
                            _ => runs.push((k, vec![m])),
                        }
                    }
                    let rows = runs
                        .into_iter()
                        .map(|(k, ms)| {
                            let inner = ms
                                .into_iter()
                                .map(|m| view! { <MatchRow m=m show_bo=show_bo push=push /> })
                                .collect_view();
                            let cls = match k {
                                BarKind::Final => "bar-run run-final",
                                BarKind::Live => "bar-run run-live",
                                BarKind::None => "bar-run",
                            };
                            view! { <div class=cls>{inner}</div> }
                        })
                        .collect_view();
                    view! {
                        <div class=league_class>
                            {head}
                            <div class="rows">{rows}</div>
                        </div>
                    }
                })
                .collect_view();
            // Grey a past day's heading, highlight today's (keys sort by date).
            let day_cls = today_key.with_value(|t| {
                if t.is_empty() {
                    "day-title"
                } else if d.day_key.as_str() < t.as_str() {
                    "day-title day-past"
                } else if d.day_key == *t {
                    "day-title day-today"
                } else {
                    "day-title"
                }
            });
            // On the homepage the date heading doubles as a share link: clicking it
            // writes #day-<date> to the URL (handy for the last days, which you
            // can't scroll far enough for the scrollspy to reach).
            let key = d.day_key.clone();
            // When venue time is shown, annotate each heading so it's clear the
            // times below are venue-local (the dates can differ from this heading).
            let show_venue = use_context::<ShowVenue>().map(|s| s.0);
            let venue_shown = move || show_venue.is_some_and(|sv| sv.get());
            let heading = view! {
                <h2
                    class=day_cls
                    class:day-share=!event_mode
                    title=(!event_mode).then_some("Link to this day")
                    on:click=move |_| {
                        #[cfg(feature = "hydrate")]
                        if !event_mode {
                            if let Some(h) = web_sys::window().and_then(|w| w.history().ok()) {
                                let _ = h.replace_state_with_url(
                                    &wasm_bindgen::JsValue::NULL,
                                    "",
                                    Some(&format!("#day-{key}")),
                                );
                            }
                        }
                        #[cfg(not(feature = "hydrate"))]
                        let _ = &key;
                    }
                >
                    {d.day_label}
                    {move || {
                        venue_shown()
                            .then(|| view! { <span class="day-venue-note">"venue time"</span> })
                    }}
                </h2>
            };
            // The first day carries the "show earlier days" control beside its date.
            let head = if idx == 0 && show_earlier {
                view! { <div class="day-head">{heading}<EarlierControl /></div> }.into_any()
            } else {
                heading.into_any()
            };
            view! {
                <section class="day" class:spy=!event_mode id=format!("day-{}", d.day_key)>
                    {head}
                    {leagues}
                </section>
            }
        })
        .collect_view();

    // The header shows the server's data-freshness time; here we keep only the
    // warning notices (demo / stale), when there are any.
    if let Some(LastUpdated(lu)) = use_context::<LastUpdated>() {
        lu.set(Some(fetched_label));
    }
    let mut notes: Vec<&str> = Vec::new();
    if demo_forced {
        notes.push("demo mode (forced)");
    } else if using_fixture {
        notes.push("demo data — set PANDASCORE_TOKEN for live schedules");
    }
    if stale {
        notes.push("data may be stale");
    }
    let note = notes.join(" · ");

    view! {
        {nav}
        {(!note.is_empty()).then(|| view! { <div class="status-line">{note}</div> })}
        // With no days to host it inline, the control stands alone above the notice.
        {(show_earlier && !has_days).then(|| view! { <EarlierControl /> })}
        {day_sections}
        {empty
            .then(|| {
                // "tier-1" is an esports framing; the traditional leagues already
                // imply their scope. An empty *event* page is a traditional
                // off-season one (its schedule has no matches to infer the mode
                // from), and on the homepage the sport toggle says which it is.
                let traditional = use_context::<SportMode>().map(|m| m.0);
                view! {
                    <p class="empty">
                        {move || {
                            if event_mode || traditional.is_some_and(|t| t.get()) {
                                "No matches in this window."
                            } else {
                                "No tier-1 matches in this window."
                            }
                        }}
                    </p>
                }
            })}
        // Traditional sports cap the forward window; let the reader reveal more
        // (LaterControl self-hides in esports mode and at the 1-week cap).
        {show_earlier.then(|| view! { <LaterControl /> })}
        {upnext_day.map(|day| view! { <UpNextBar day=day /> })}
    }
}

/// A click handler that toggles whether match `uid`'s score is revealed in the
/// shared [`RevealedMatches`] set (and persists the set). Stops propagation +
/// prevents the default so the click doesn't fall through to an enclosing row
/// link (a no-op on a standalone reveal button). `None` `revealed` ⇒ inert.
pub(crate) fn reveal_toggler(
    revealed: Option<RwSignal<HashSet<String>>>,
    uid: String,
) -> impl Fn(leptos::ev::MouseEvent) + Clone {
    move |ev: leptos::ev::MouseEvent| {
        ev.prevent_default();
        ev.stop_propagation();
        if let Some(r) = revealed {
            r.update(|s| {
                if !s.remove(&uid) {
                    s.insert(uid.clone());
                }
            });
            #[cfg(feature = "hydrate")]
            save_revealed(&r.get_untracked());
        }
    }
}

/// A schedule/series row's time cell. `primary` is the viewer-tz label shown by
/// default; when `venue_label` is non-empty the cell becomes clickable to *swap*
/// the whole label to the venue-local time (the shared [`ShowVenue`] toggle, so
/// clicking any one reveals them all), rather than appending it — the row width
/// stays put. `base_class` is the cell class without the toggle modifier;
/// `show_title`/`hide_title` are the hover hints for the two states.
pub(crate) fn venue_time_cell(
    base_class: &'static str,
    primary: String,
    venue_label: String,
    show_title: &'static str,
    hide_title: &'static str,
) -> AnyView {
    if venue_label.is_empty() {
        return view! { <span class=base_class>{primary}</span> }.into_any();
    }
    let show_venue = use_context::<ShowVenue>().map(|s| s.0);
    let toggle_venue = move |ev: leptos::ev::MouseEvent| {
        // Don't let the click fall through to the row's match link.
        ev.prevent_default();
        ev.stop_propagation();
        if let Some(sv) = show_venue {
            sv.update(|v| *v = !*v);
        }
    };
    let shown = move || show_venue.is_some_and(|sv| sv.get());
    let title = move || if shown() { hide_title } else { show_title };
    view! {
        <span
            class=format!("{base_class} row-time-tz")
            class:on=shown
            title=title
            on:click=toggle_venue
        >
            {move || if shown() { venue_label.clone() } else { primary.clone() }}
        </span>
    }
    .into_any()
}

// ----- Shared schedule/series row pieces ------------------------------------
// MatchRow and SeriesRow render the same kind of row (time · teams · score/meta ·
// status bar) and shared all of the spoiler-reveal machinery below verbatim;
// these helpers are the single definition both call, so the reveal logic lives in
// one place.

/// A row's spoiler-reveal state, keyed on the match uid: a memo that's true when
/// the global toggle is on or this row was individually revealed, plus the click
/// handler that toggles just this row.
pub(crate) fn row_reveal(muid: &str) -> (Memo<bool>, impl Fn(leptos::ev::MouseEvent) + Clone) {
    let global = use_context::<ShowScores>().map(|s| s.0);
    let revealed = use_context::<RevealedMatches>().map(|r| r.0);
    let reveal = {
        let muid = muid.to_string();
        Memo::new(move |_| {
            global.is_some_and(|g| g.get())
                || revealed.is_some_and(|r| r.with(|set| set.contains(&muid)))
        })
    };
    let toggle = reveal_toggler(revealed, muid.to_string());
    (reveal, toggle)
}

/// A row's status/score meta cell. When `revealable` (the row has a score hidden
/// behind the spoiler), the badge cluster is the clickable reveal control;
/// otherwise it's static. `bo` is the optional best-of label beside the badge
/// (matches only).
pub(crate) fn reveal_meta(
    reveal: Memo<bool>,
    toggle: impl Fn(leptos::ev::MouseEvent) + 'static,
    status: MatchStatus,
    status_class: &str,
    badge: &'static str,
    bo: Option<String>,
    revealable: bool,
) -> AnyView {
    let noun = if matches!(status, MatchStatus::Live) {
        "live score"
    } else {
        "final score"
    };
    let show_title = format!("Show the {noun}");
    let hide_title = format!("Hide the {noun}");
    let badge_cls = format!("row-badge {status_class}");
    let bo_span = bo
        .filter(|s| !s.is_empty())
        .map(|b| view! { <span class="row-bo">{b}</span> });
    if revealable {
        view! {
            <span class="row-meta">
                <span
                    class="reveal-meta"
                    class:on=move || reveal.get()
                    title=move || if reveal.get() { hide_title.clone() } else { show_title.clone() }
                    on:click=toggle
                >
                    <span class=badge_cls>{badge}</span>
                    {bo_span}
                </span>
            </span>
        }
        .into_any()
    } else {
        view! {
            <span class="row-meta">
                <span class=badge_cls>{badge}</span>
                {bo_span}
            </span>
        }
        .into_any()
    }
}

/// The leading status bar of a row: the live/final side bar, or an empty
/// placeholder so the column stays reserved (the layout never shifts).
pub(crate) fn status_bar(status: MatchStatus) -> AnyView {
    match status {
        MatchStatus::Live => {
            view! { <span class="row-bar live" aria-label="Live"></span> }.into_any()
        }
        MatchStatus::Finished => {
            view! { <span class="row-bar final" aria-label="Final"></span> }.into_any()
        }
        _ => view! { <span class="row-lead-empty"></span> }.into_any(),
    }
}

/// The "team A — score/sep — team B" middle of a two-sided row, with the reactive
/// winner/loser/scored classes. `a`/`b` are the rendered team cells; `sep` is the
/// unrevealed middle ("vs"/"at"/…).
#[allow(clippy::too_many_arguments)]
pub(crate) fn versus_row(
    reveal: Memo<bool>,
    win_a: bool,
    win_b: bool,
    has: bool,
    a: AnyView,
    b: AnyView,
    score_a: Option<i64>,
    score_b: Option<i64>,
    sep: String,
) -> impl IntoView {
    view! {
        <span
            class="row-team row-a"
            class:winner=move || reveal.get() && win_a
            class:loser=move || reveal.get() && win_b
        >
            {a}
        </span>
        <span class="row-mid" class:scored=move || reveal.get() && has>
            {move || {
                if reveal.get() && has {
                    format!("{} \u{2013} {}", score_a.unwrap_or(0), score_b.unwrap_or(0))
                } else {
                    sep.clone()
                }
            }}
        </span>
        <span
            class="row-team row-b"
            class:winner=move || reveal.get() && win_b
            class:loser=move || reveal.get() && win_a
        >
            {b}
        </span>
    }
}

#[component]
pub(crate) fn MatchRow(m: MatchView, show_bo: bool, push: bool) -> impl IntoView {
    let status_class = m.status.row_class();
    let badge = m.status.badge();

    let sa = m.team_a.score;
    let sb = m.team_b.score;
    let has = sa.is_some() && sb.is_some();
    let win_a = m.team_a.winner;
    let win_b = m.team_b.winner;
    let sep = versus_sep(m.sport);
    let bo = if show_bo { m.best_of } else { String::new() };

    // Scores are spoilers: reveal only when the global toggle is on or this
    // match was individually revealed (by clicking its "Final · Bo3" cluster).
    let muid = crate::types::match_uid(m.sport, m.id);
    // Most rows link to their own /match page; a collapsed WRC "Day N" row instead
    // points at that day on the event page (where its stages are listed).
    let row_href = m
        .row_href
        .clone()
        .unwrap_or_else(|| crate::types::match_path(m.sport, m.id));
    let (reveal, toggle_reveal) = row_reveal(&muid);
    // The bo rides beside the badge (only when present, so the reveal underline
    // doesn't extend into an empty cell on uniform events). Score-less rows
    // (upcoming, or a live match still at the 0-0 placeholder) get a static meta.
    let meta_view = reveal_meta(
        reveal,
        toggle_reveal,
        m.status,
        &status_class,
        badge,
        Some(bo),
        has,
    );

    // The time can toggle to also show the local time at the venue (MLB stadiums
    // carry a timezone; esports don't, so there it stays a plain label). The
    // toggle is shared, so clicking any one time reveals every venue time at
    // once. Clicking stops the row's navigation, like the score reveal.
    let time_view = venue_time_cell(
        "row-time",
        m.clock_label,
        m.venue_label,
        "Click to show the local time at each venue",
        "Click to hide the local time at each venue",
    );

    // F1 (and any single-entity sport) has no opponent: the one label is the
    // session (e.g. "Race"), spanning the team columns with no "vs"/score middle.
    let inner = if m.sport.single_entity() {
        view! {
            {time_view}
            <span class="row-team row-solo">{team_cell(m.team_a.label, m.team_a.abbrev)}</span>
            {meta_view}
        }
        .into_any()
    } else {
        view! {
            {time_view}
            {versus_row(
                reveal,
                win_a,
                win_b,
                has,
                team_cell(m.team_a.label, m.team_a.abbrev),
                team_cell(m.team_b.label, m.team_b.abbrev),
                sa,
                sb,
                sep.to_string(),
            )}
            {meta_view}
        }
        .into_any()
    };

    // The whole row links to the match detail page; `display: contents` lets its
    // children participate in the row grid alongside the ★ button. The score
    // reveal inside (`reveal-meta`) prevents this navigation when clicked.
    let body = view! {
        <a class="row-body" href=row_href>
            {inner}
        </a>
    };

    // The leading column is always reserved, so the layout never shifts when push
    // support (the ★) resolves after hydration — `push` only controls whether the
    // reminder ★ is shown, not the grid. The column holds the ★ (upcoming, push
    // enabled), or the shared status bar (the live/final side bar or an empty
    // placeholder).
    let lead = match m.status {
        MatchStatus::Upcoming if push => view! {
            <StarButton
                id=m.id
                sport=m.sport
                league=m.league.clone()
                team_a=m.team_a.name.clone()
                team_b=m.team_b.name.clone()
            />
        }
        .into_any(),
        _ => status_bar(m.status),
    };
    view! {
        <div class=format!("row has-star {status_class}") id=format!("m-{muid}")>
            {lead}
            {body}
        </div>
    }
    .into_any()
}
