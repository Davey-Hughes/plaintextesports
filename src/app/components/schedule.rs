//! The schedule view: filter tabs/chips, the day/league sections, the up-next
//! bar, and the match-row rendering + progressive-reveal machinery.
use crate::app::*;
use std::collections::HashMap;

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
        use leptos_router::NavigateOptions;
        use leptos_router::hooks::{use_location, use_navigate, use_query_map};

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
        // Transition, not Suspense: `stages` re-keys when the single-event filter
        // changes, so keep the current standings/bracket on screen while the next
        // event's load instead of flashing the panel out and back in.
        <Transition>
            {move || {
                stages
                    .get()
                    .and_then(Result::ok)
                    .filter(|v| !v.is_empty())
                    .map(|v| view! { <EventStages stages=v /> })
            }}
        </Transition>
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
    leagues: Vec<(String, Option<Sport>)>,
    selected: RwSignal<HashSet<String>>,
) -> impl IntoView {
    // Remember the sport of a league when its chip is selected, so the chip (and
    // its ★) survives the league's events leaving the windowed view.
    let sports = use_context::<SelectedSports>().map(|s| s.0);
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
            let is_active = move || selected.with(|s| s.contains(&sel_name));
            // The ★ only when we know the league's sport — a persistent chip whose
            // events have left the window (and was never seen this session) renders
            // label-only, still clickable to clear.
            let star = sport.map(|sp| {
                view! { <SubscribeStar kind="league" sport=sp value=name.clone() /> }
            });
            view! {
                <span class=format!("chip chip-with-star {lc}") class:active=is_active>
                    <button
                        class="chip-label"
                        on:click=move |_| {
                            selected
                                .update(|s| {
                                    if !s.remove(&click_name) {
                                        s.insert(click_name.clone());
                                        // Remember the sport so the chip survives the
                                        // league's events leaving the window.
                                        if let (Some(sp), Some(ss)) = (sport, sports) {
                                            ss.update(|m| {
                                                m.insert(click_name.clone(), sp);
                                            });
                                        }
                                    }
                                });
                            #[cfg(feature = "hydrate")]
                            save_str_set(keys::LEAGUES, &selected.get_untracked());
                        }
                    >
                        {name}
                    </button>
                    {star}
                </span>
            }
        })
        .collect_view();

    view! { <div class="chips">{chips}</div> }
}

/// One day ready for the homepage's keyed `<For>`: the `DayGroup` plus a
/// content-sensitive `key`. The key changes whenever anything that affects the
/// day's render does — its own match set/scores/statuses, its position (first day
/// hosts the "earlier" control), or the cross-day "editions with an upcoming
/// session" set (a head ★ on a finished day depends on a later day) — so `<For>`
/// re-renders exactly the days that changed on a filter toggle or 60s refresh and
/// reuses the rest, instead of rebuilding the whole list.
struct PreparedDay {
    key: String,
    is_first: bool,
    today_key: String,
    editions: std::sync::Arc<HashSet<(Sport, String, String)>>,
    day: DayGroup,
}

/// The competition chips for the current view and whether to show them. Borrows
/// the schedule (no clone), shared by the chip row and the day filter so both
/// agree on when the chip selection applies.
fn chip_state(
    s: &ScheduleView,
    games_set: &HashSet<String>,
    trad: bool,
) -> (Vec<(String, Sport)>, bool) {
    let available = leagues_for_games(s, games_set);
    let has_sub = s
        .days
        .iter()
        .flat_map(|d| &d.leagues)
        .flat_map(|lg| &lg.matches)
        .any(|m| {
            (games_set.is_empty() || games_set.contains(m.sport.slug()))
                && m.sport.has_sub_leagues()
        });
    // A game filter being active shows the chip(s) even for a single competition
    // (e.g. baseball → just MLB), so the competition you've narrowed to is always
    // named on screen rather than vanishing.
    let show_chips = if trad {
        !available.is_empty() && (available.len() > 1 || has_sub || !games_set.is_empty())
    } else {
        !available.is_empty()
    };
    (available, show_chips)
}

/// The chip list for the row: every available league (with its sport), then a chip
/// for each *selected* league that isn't in the current window — using its
/// remembered sport when known (full chip with ★), else `None` (label-only). So a
/// selected filter never disappears just because its events scrolled out of view.
fn chips_with_selected(
    available: &[(String, Sport)],
    selected: &HashSet<String>,
    sports: &HashMap<String, Sport>,
) -> Vec<(String, Option<Sport>)> {
    let mut out: Vec<(String, Option<Sport>)> = available
        .iter()
        .map(|(n, s)| (n.clone(), Some(*s)))
        .collect();
    for name in selected {
        if !available.iter().any(|(n, _)| n == name) {
            out.push((name.clone(), sports.get(name).copied()));
        }
    }
    // Fixed order: by game type (the canonical `Sport::ALL` / tab order), then league
    // name alphabetically (case-insensitive). A chip with an unknown sport — a
    // persisted out-of-window selection never seen this session — sorts last.
    let rank = |s: &Option<Sport>| {
        s.and_then(|sp| Sport::ALL.iter().position(|x| *x == sp))
            .unwrap_or(usize::MAX)
    };
    out.sort_by(|(an, asp), (bn, bsp)| {
        rank(asp)
            .cmp(&rank(bsp))
            .then_with(|| an.to_lowercase().cmp(&bn.to_lowercase()))
    });
    out
}

/// Order-independent fingerprint of the editions-with-upcoming set.
fn editions_signature(e: &HashSet<(Sport, String, String)>) -> u64 {
    use std::hash::{Hash, Hasher};
    e.iter()
        .map(|x| {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            x.hash(&mut h);
            h.finish()
        })
        .fold(0u64, |a, b| a ^ b)
}

/// Fingerprint of a day's render-affecting content: its leagues and, per match,
/// the id + the mutable status/scores the 60s refresh changes, plus the formatted
/// time labels. The labels are re-formatted when the viewer flips the 12h/24h
/// clock (or changes timezone) — both re-key the schedule resource — so they must
/// be in the key, else the keyed `<For>` reuses the row and the on-screen times
/// go stale.
fn day_content_hash(d: &DayGroup) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for lg in &d.leagues {
        lg.league.hash(&mut h);
        lg.series_name.hash(&mut h);
        for m in &lg.matches {
            m.id.hash(&mut h);
            std::mem::discriminant(&m.status).hash(&mut h);
            m.team_a.score.hash(&mut h);
            m.team_b.score.hash(&mut h);
            // Time-format/timezone-dependent display strings (the reveal-on-click
            // venue time included, so it too refreshes after a toggle).
            m.clock_label.hash(&mut h);
            m.venue_label.hash(&mut h);
        }
    }
    h.finish()
}

/// Apply the chip filter and turn the result into keyed [`PreparedDay`]s for the
/// homepage's `<For>`. Only the selections that correspond to a visible chip are
/// applied, so switching to a single-competition tab can't silently empty the view
/// (a stale selection from another sport is ignored) while a lone chip still
/// narrows.
/// Split a viewer-tz date label "Weekday, Month D" into (weekday, month, day).
fn parse_date_label(s: &str) -> Option<(&str, &str, &str)> {
    let (wd, md) = s.split_once(", ")?;
    let (mo, d) = md.rsplit_once(' ')?;
    Some((wd, mo, d))
}

/// The span label for a chained day group, from its first and last match's
/// viewer-tz date labels ("Friday, July 3"). One date → the plain label; across
/// days → a span ("Friday–Saturday, July 3–4", or across a month change
/// "Friday, June 30 – Saturday, July 1"). Mirrors the server-side day label.
fn span_from_date_labels(first: &str, last: &str) -> String {
    if last.is_empty() || first == last {
        return first.to_string();
    }
    if first.is_empty() {
        return last.to_string();
    }
    match (parse_date_label(first), parse_date_label(last)) {
        (Some((wd1, mo1, d1)), Some((wd2, mo2, d2))) if mo1 == mo2 => {
            format!("{wd1}–{wd2}, {mo1} {d1}–{d2}")
        }
        (Some((wd1, mo1, d1)), Some((wd2, mo2, d2))) => {
            format!("{wd1}, {mo1} {d1} – {wd2}, {mo2} {d2}")
        }
        _ => format!("{first} – {last}"),
    }
}

/// Merge a single event's day groups into broadcast "nights": consecutive matches
/// closer than [`crate::types::CHAIN_GAP_MS`] share a group even across midnight,
/// so a late slate that spills past 12 in the viewer's zone reads as one night —
/// the same chaining the event page does server-side. Only called when the
/// schedule is narrowed to one event, so every match is the same edition and each
/// output group is a single `LeagueGroup`.
fn chain_single_event_days(days: Vec<DayGroup>) -> Vec<DayGroup> {
    let Some(template) = days.iter().flat_map(|d| &d.leagues).next().cloned() else {
        return days;
    };
    // Flatten to (source day_key, source date label, match) in time order.
    let mut all: Vec<(String, String, MatchView)> = Vec::new();
    for d in days {
        for lg in d.leagues {
            for m in lg.matches {
                all.push((d.day_key.clone(), d.day_label.clone(), m));
            }
        }
    }
    all.sort_by_key(|(_, _, m)| m.begin_at_ms);

    let mk_day = |run: Vec<(String, String, MatchView)>| -> DayGroup {
        let day_key = run.first().map(|(k, _, _)| k.clone()).unwrap_or_default();
        let first_label = run.first().map(|(_, l, _)| l.clone()).unwrap_or_default();
        let last_label = run.last().map(|(_, l, _)| l.clone()).unwrap_or_default();
        let matches: Vec<MatchView> = run.into_iter().map(|(_, _, m)| m).collect();
        let bo = {
            let first = matches
                .first()
                .map(|m| m.best_of.clone())
                .unwrap_or_default();
            (!first.is_empty() && matches.iter().all(|m| m.best_of == first)).then_some(first)
        };
        DayGroup {
            day_key,
            day_label: span_from_date_labels(&first_label, &last_label),
            leagues: vec![crate::types::LeagueGroup {
                league: template.league.clone(),
                series_name: template.series_name.clone(),
                event_url: template.event_url.clone(),
                bo,
                matches,
            }],
        }
    };

    let mut out: Vec<DayGroup> = Vec::new();
    let mut run: Vec<(String, String, MatchView)> = Vec::new();
    let mut prev_ms: Option<i64> = None;
    for item in all {
        if prev_ms.is_some_and(|p| item.2.begin_at_ms - p >= crate::types::CHAIN_GAP_MS) {
            out.push(mk_day(std::mem::take(&mut run)));
        }
        prev_ms = Some(item.2.begin_at_ms);
        run.push(item);
    }
    if !run.is_empty() {
        out.push(mk_day(run));
    }
    out
}

fn prepare_days(
    s: ScheduleView,
    games_set: &HashSet<String>,
    leagues_sel: &HashSet<String>,
    trad: bool,
) -> Vec<PreparedDay> {
    let (available, _) = chip_state(&s, games_set, trad);
    let available: HashSet<String> = available.into_iter().map(|(l, _)| l).collect();
    // An empty result ⇒ no league constraint (i.e. "all"); otherwise keep only the
    // selected leagues that are actually on offer for the current games.
    let leagues_filter: HashSet<String> = leagues_sel
        .iter()
        .filter(|l| available.contains(*l))
        .cloned()
        .collect();
    let mut filtered = filter_schedule(s, games_set, &leagues_filter);
    // Narrowed to a single event? Chain its calendar days into broadcast nights
    // (a late slate that crosses midnight reads as one), like the event page. Only
    // for a single edition, so the gap rule is unambiguous.
    let single_event = {
        let mut eds = filtered
            .days
            .iter()
            .flat_map(|d| &d.leagues)
            .map(|lg| (lg.league.as_str(), lg.series_name.as_str()));
        eds.next()
            .map(|first| eds.all(|e| e == first))
            .unwrap_or(false)
    };
    if single_event {
        filtered.days = chain_single_event_days(std::mem::take(&mut filtered.days));
    }
    let today_key = filtered.today_key.clone();
    // Editions with an upcoming session anywhere in the (filtered) window — drives
    // the head ★, which can sit on a finished day of a still-live edition.
    let editions: HashSet<(Sport, String, String)> = filtered
        .days
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
    let ed_sig = editions_signature(&editions);
    let editions = std::sync::Arc::new(editions);
    filtered
        .days
        .into_iter()
        .enumerate()
        .map(|(i, day)| PreparedDay {
            key: format!(
                "{}|{}|{:x}|{:x}",
                day.day_key,
                i == 0,
                day_content_hash(&day),
                ed_sig
            ),
            is_first: i == 0,
            today_key: today_key.clone(),
            editions: editions.clone(),
            day,
        })
        .collect()
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
    // Show ★ reminder buttons only when Web Push is configured. `vapid` is seeded
    // at render (from the shell <meta>), so this is settled before the first paint.
    let push =
        use_context::<RwSignal<Option<String>>>().is_some_and(|v| v.get_untracked().is_some());
    view! {
        // Transition (not Suspense) so re-keying the resource — e.g. "show
        // earlier days" extends the range — keeps the current schedule on screen
        // while the next loads, instead of blanking the page (which flashed the
        // footer up to the top).
        <Transition fallback=|| view! { <p class="loading">"loading…"</p> }>
            // The frame — chips, single-day nav, and the freshness/demo notices.
            // Reruns on any change (cheap: no rows), and reads the resource inside
            // the Transition so loading/refetch behaves as before.
            {move || {
                let trad = traditional.get();
                match resource.get() {
                    Some(Ok(s)) => {
                        let games_set = games.get();
                        let (available, mut show_chips) = chip_state(&s, &games_set, trad);
                        // Keep a chip for every selected league, even one whose
                        // events have left the window, so the filter never vanishes
                        // until you clear it (and the row stays shown while selected).
                        let sel = leagues.get();
                        let sel_sports = use_context::<SelectedSports>()
                            .map(|s| s.0.get())
                            .unwrap_or_default();
                        let chip_list = chips_with_selected(&available, &sel, &sel_sports);
                        show_chips = show_chips || !sel.is_empty();
                        if let Some(LastUpdated(lu)) = use_context::<LastUpdated>() {
                            lu.set(Some(s.fetched_label.clone()));
                        }
                        let nav = show_nav.then(|| {
                            let prev = s.prev_date.clone().unwrap_or_default();
                            let next = s.next_date.clone().unwrap_or_default();
                            let label = s.date_label.clone().unwrap_or_default();
                            view! {
                                <nav class="day-nav">
                                    <A href=format!("/day/{prev}")>"‹ prev"</A>
                                    <span class="day-nav-label">{label}</span>
                                    <A href=format!("/day/{next}")>"next ›"</A>
                                </nav>
                            }
                        });
                        let mut notes: Vec<&str> = Vec::new();
                        if s.demo_forced {
                            notes.push("demo mode (forced)");
                        } else if s.using_fixture {
                            notes.push("demo data — set PANDASCORE_TOKEN for live schedules");
                        }
                        if s.stale {
                            notes.push("data may be stale");
                        }
                        let note = notes.join(" · ");
                        view! {
                            {if show_chips {
                                view! { <LeagueChips leagues=chip_list selected=leagues /> }
                                    .into_any()
                            } else if !games_set.is_empty() {
                                // A game filter is active but matched no competition
                                // — keep a blank chip row (its reserved min-height)
                                // so the page doesn't shift between a populated and
                                // an empty filtered result.
                                view! { <div class="chips"></div> }.into_any()
                            } else {
                                ().into_any()
                            }}
                            {nav}
                            {(!note.is_empty())
                                .then(|| view! { <div class="status-line">{note}</div> })}
                        }
                            .into_any()
                    }
                    Some(Err(_)) => {
                        view! { <p class="error">"Failed to load schedule."</p> }.into_any()
                    }
                    None => ().into_any(),
                }
            }}
            // The day list: a keyed <For> so a filter toggle / 60s refresh diffs
            // (re-rendering only the days whose content key changed) instead of
            // rebuilding every row. `each` reads the resource inside the Transition.
            <For
                each=move || {
                    let trad = traditional.get();
                    match resource.get() {
                        Some(Ok(s)) => prepare_days(s, &games.get(), &leagues.get(), trad),
                        _ => Vec::new(),
                    }
                }
                key=|pd| pd.key.clone()
                children=move |pd| {
                    let today = StoredValue::new(pd.today_key);
                    render_day(pd.day, pd.is_first, today, &pd.editions, false, !show_nav, push)
                }
            />
            // Empty-state message + the homepage's earlier/later reveal controls.
            {move || {
                let trad = traditional.get();
                match resource.get() {
                    Some(Ok(s)) => {
                        let empty = prepare_days(s, &games.get(), &leagues.get(), trad).is_empty();
                        // The homepage (no nav) reveals earlier/later days; the
                        // single-day view uses prev/next nav instead.
                        let show_earlier = !show_nav;
                        view! {
                            {(show_earlier && empty).then(|| view! { <EarlierControl /> })}
                            {empty
                                .then(|| {
                                    view! {
                                        <p class="empty">
                                            {move || {
                                                if traditional.get() {
                                                    "No matches in this window."
                                                } else {
                                                    "No tier-1 matches in this window."
                                                }
                                            }}
                                        </p>
                                    }
                                })}
                            {show_earlier.then(|| view! { <LaterControl /> })}
                        }
                            .into_any()
                    }
                    _ => ().into_any(),
                }
            }}
        </Transition>
        // When exactly one esports event is selected, show its standings/bracket.
        {move || (!traditional.get()).then(|| view! { <EventSection leagues=leagues /> })}
        // Likewise, a single traditional league shows its division/group tables.
        <TraditionalStandings league=trad_standings_league />
        // F1 has no league table — when narrowed to it, show the current
        // drivers' + constructors' championship standings instead. Transition
        // (not Suspense) so re-keying on the season signal keeps the table up
        // while the next loads instead of blanking it.
        <Transition>
            {move || {
                let season = f1_home_season.get()?;
                let s = f1_home_standings.get()?;
                (!s.drivers.is_empty() || !s.constructors.is_empty()).then(|| {
                    let round = s.round;
                    view! { <F1StandingsView standings=s season=season round=round /> }
                })
            }}
        </Transition>
        // WRC/WEC get their championship tables (from ocblacktop) the same way.
        <Transition>
            {move || {
                let lg = motor_home_league.get();
                if lg.is_empty() {
                    return None;
                }
                let s = motor_home_standings.get()?;
                (!s.tables.is_empty()).then(|| view! { <MotorStandingsView standings=s league=lg /> })
            }}
        </Transition>
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
    // Order is first-appearance across the (chronological, sport-grouped) days —
    // the exact same iteration the schedule renders, so the chips read in the same
    // order the leagues appear as you scroll.
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

/// Persists the up-next bar's "scrolled past" visibility across the event/team
/// page's per-refetch subtree rebuilds. Those pages rebuild their whole content
/// subtree when the schedule resource resolves, which re-creates [`UpNextBar`]
/// with fresh signals — defaulting to *visible* until its IntersectionObservers
/// re-fire a frame later, so a scrolled-past (hidden) bar would blink back on
/// every 60s refresh / manual refresh. Providing these signals from the page
/// (which is itself *not* rebuilt) lets a fresh bar mount in the last-known state
/// instead of flashing.
#[derive(Clone, Copy)]
pub(crate) struct UpNextSeen {
    pub(crate) day: RwSignal<bool>,
    pub(crate) foot: RwSignal<bool>,
}

/// A compact "up next" bar pinned to the bottom of the event page, previewing
/// the current/next day's matches. Clicking it scrolls to that day in the list;
/// it hides once that day scrolls into view, so it never duplicates what's shown.
#[component]
pub(crate) fn UpNextBar(day: DayGroup) -> impl IntoView {
    let anchor = format!("day-{}", day.day_key);
    // The bar hides when its target day scrolls into view (you're already there)
    // or when the footer does (so it never covers the footer at the page bottom).
    // Prefer the page-provided signals (event/team pages) so this state survives
    // the page's per-refetch subtree rebuild — a fresh bar then mounts already in
    // its hidden state instead of flashing visible for a frame. See [`UpNextSeen`].
    let UpNextSeen {
        day: day_seen,
        foot: foot_seen,
    } = use_context::<UpNextSeen>().unwrap_or_else(|| UpNextSeen {
        day: RwSignal::new(false),
        foot: RwSignal::new(false),
    });
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
        use wasm_bindgen::JsCast;
        use wasm_bindgen::closure::Closure;
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
            render_day(
                d,
                idx == 0,
                today_key,
                &editions_with_upcoming,
                event_mode,
                show_earlier,
                push,
            )
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

/// Render one day's `<section>`: its heading (a share link on the homepage, with
/// the "show earlier" control on the first day), then its league groups, each a
/// titled header (with the edition ★) over status-grouped row runs. Extracted from
/// [`render_schedule`] so the live homepage can drive the day list with a keyed
/// `<For>` (diffing across filter toggles and the 60s refresh) while the static
/// event/team/day pages keep mapping it straight through.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_day(
    d: DayGroup,
    is_first: bool,
    today_key: StoredValue<String>,
    editions_with_upcoming: &HashSet<(Sport, String, String)>,
    event_mode: bool,
    show_earlier: bool,
    push: bool,
) -> AnyView {
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
                && lg
                    .matches
                    .iter()
                    .all(|m| matches!(m.status, MatchStatus::Finished));
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
    let head = if is_first && show_earlier {
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
    .into_any()
}

/// A click handler that toggles whether match `uid`'s score is revealed in the
/// shared [`RevealedMatches`] set (and persists the set). Stops propagation +
/// prevents the default so the click doesn't fall through to an enclosing row
/// link (a no-op on a standalone reveal button). `None` `revealed` ⇒ inert.
pub(crate) fn reveal_toggler(
    revealed: Option<RwSignal<HashSet<String>>>,
    uid: String,
    end_ms: i64,
) -> impl Fn(leptos::ev::MouseEvent) + Clone {
    // Only consumed by the hydrate-only record write below.
    #[cfg(not(feature = "hydrate"))]
    let _ = end_ms;
    let global = use_context::<ShowScores>().map(|s| s.0);
    let flash = use_context::<FlashScores>().map(|f| f.0);
    move |ev: leptos::ev::MouseEvent| {
        ev.prevent_default();
        ev.stop_propagation();
        // A hide can't take effect while the global reveal is on — flash the
        // "scores" button instead.
        if hide_deflected_to_global(global, flash) {
            return;
        }
        if let Some(r) = revealed {
            let was = r.with_untracked(|s| s.contains(&uid));
            r.update(|s| {
                if was {
                    s.remove(&uid);
                } else {
                    s.insert(uid.clone());
                }
            });
            // Record/clear the localStorage reveal record so it carries the match's
            // end (for pruning) and a fresh touch timestamp.
            #[cfg(feature = "hydrate")]
            if was {
                remove_reveal(keys::REVEALED, &uid);
            } else {
                touch_reveals(keys::REVEALED, &[(uid.clone(), end_ms)], now_ms());
            }
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
pub(crate) fn row_reveal(
    muid: &str,
    end_ms: i64,
) -> (Memo<bool>, impl Fn(leptos::ev::MouseEvent) + Clone + use<>) {
    let global = use_context::<ShowScores>().map(|s| s.0);
    let revealed = use_context::<RevealedMatches>().map(|r| r.0);
    let reveal = {
        let muid = muid.to_string();
        Memo::new(move |_| {
            global.is_some_and(|g| g.get())
                || revealed.is_some_and(|r| r.with(|set| set.contains(&muid)))
        })
    };
    // Viewing counts as interaction: if this row is a stored reveal being rendered,
    // refresh its touch (and re-capture its end) once on mount, so anything you
    // still look at stays revealed. Only the (few) rows actually in the revealed
    // set need this, so gate the effect's creation on that — a large schedule
    // otherwise spins up a reactive node per row at hydration for nothing. The
    // condition is read untracked (the original effect did too), so it's settled
    // once at mount, not re-evaluated.
    #[cfg(feature = "hydrate")]
    {
        let uid = muid.to_string();
        if revealed.is_some_and(|r| r.with_untracked(|s| s.contains(&uid))) {
            Effect::new(move |_| {
                touch_reveals(keys::REVEALED, &[(uid.clone(), end_ms)], now_ms());
            });
        }
    }
    let toggle = reveal_toggler(revealed, muid.to_string(), end_ms);
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
    // The match's own start time is its reveal "end" (matches are short; a week's
    // grace absorbs the duration).
    let (reveal, toggle_reveal) = row_reveal(&muid, m.begin_at_ms);
    // The bo rides beside the badge (only when present, so the reveal underline
    // doesn't extend into an empty cell on uniform events). Score-less rows
    // (upcoming, or a live match still at the 0-0 placeholder) get a static meta.
    let meta_view = reveal_meta(
        reveal,
        toggle_reveal,
        m.status,
        status_class,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{LeagueGroup, TeamView};
    use std::collections::{HashMap, HashSet};

    fn dv(ms: i64, date_label: &str) -> MatchView {
        MatchView {
            id: ms,
            sport: Sport::Lol,
            league: "MSI".into(),
            series_name: "2026".into(),
            status: MatchStatus::Upcoming,
            clock_label: String::new(),
            date_label: date_label.into(),
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
            league_url: String::new(),
            begin_at_ms: ms,
            row_href: None,
        }
    }

    fn day(key: &str, label: &str, ms: &[(i64, &str)]) -> DayGroup {
        DayGroup {
            day_key: key.into(),
            day_label: label.into(),
            leagues: vec![LeagueGroup {
                league: "MSI".into(),
                series_name: "2026".into(),
                event_url: String::new(),
                bo: Some("Bo3".into()),
                matches: ms.iter().map(|(m, l)| dv(*m, l)).collect(),
            }],
        }
    }

    #[test]
    fn span_label_collapses_and_spans() {
        assert_eq!(
            span_from_date_labels("Friday, July 3", "Friday, July 3"),
            "Friday, July 3"
        );
        assert_eq!(
            span_from_date_labels("Friday, July 3", "Saturday, July 4"),
            "Friday–Saturday, July 3–4"
        );
        assert_eq!(
            span_from_date_labels("Friday, June 30", "Saturday, July 1"),
            "Friday, June 30 – Saturday, July 1"
        );
    }

    #[test]
    fn chain_single_event_merges_a_night_across_midnight() {
        let h = 3_600_000i64;
        let fri8 = 1_000_000_000_000i64;
        // Calendar-grouped input: Fri {8pm}, Sat {1am, 8pm}.
        let days = vec![
            day("2026-07-03", "Friday, July 3", &[(fri8, "Friday, July 3")]),
            day(
                "2026-07-04",
                "Saturday, July 4",
                &[
                    (fri8 + 5 * h, "Saturday, July 4"),  // Sat 1am (5h after Fri 8pm)
                    (fri8 + 24 * h, "Saturday, July 4"), // Sat 8pm (19h after Sat 1am)
                ],
            ),
        ];
        let out = chain_single_event_days(days);
        assert_eq!(out.len(), 2, "Fri8pm+Sat1am chain; Sat8pm is a new night");
        assert_eq!(out[0].leagues[0].matches.len(), 2);
        assert_eq!(out[0].day_key, "2026-07-03");
        assert_eq!(out[0].day_label, "Friday–Saturday, July 3–4");
        assert_eq!(out[1].leagues[0].matches.len(), 1);
        assert_eq!(out[1].day_label, "Saturday, July 4");
    }

    #[test]
    fn chips_with_selected_orders_by_sport_then_name_and_keeps_selected() {
        // Mixed sports + out of order; one selected league is in the window (IEM),
        // one is out of window with a remembered sport (BLAST → Cs2), one out of
        // window with an unknown sport (Ghost → None).
        let available = vec![
            ("Worlds".to_string(), Sport::Lol),
            ("IEM".to_string(), Sport::Cs2),
            ("LCK".to_string(), Sport::Lol),
        ];
        let selected: HashSet<String> = ["IEM".into(), "BLAST".into(), "Ghost".into()]
            .into_iter()
            .collect();
        let mut sports = HashMap::new();
        sports.insert("BLAST".to_string(), Sport::Cs2); // remembered from a click
        let out = chips_with_selected(&available, &selected, &sports);
        let names: Vec<&str> = out.iter().map(|(n, _)| n.as_str()).collect();
        // Grouped by game type in Sport::ALL order (CS2 before LoL), alphabetical
        // within each; an out-of-window selection (BLAST) is sorted in; an
        // unknown-sport one (Ghost) sorts last; no duplicate for IEM.
        assert_eq!(names, vec!["BLAST", "IEM", "LCK", "Worlds", "Ghost"]);
        let by: HashMap<String, Option<Sport>> = out.iter().cloned().collect();
        assert_eq!(by.get("BLAST"), Some(&Some(Sport::Cs2)));
        assert_eq!(by.get("Ghost"), Some(&None));
    }

    /// A one-match day differing only in its formatted start time(s).
    fn day_with_time(clock: &str, venue: &str) -> DayGroup {
        let team = |label: &str| TeamView {
            label: label.into(),
            name: label.into(),
            score: None,
            winner: false,
            logo: String::new(),
            abbrev: String::new(),
        };
        DayGroup {
            day_key: "2026-06-21".into(),
            day_label: "Sunday, June 21".into(),
            leagues: vec![LeagueGroup {
                league: "LCK".into(),
                series_name: String::new(),
                event_url: String::new(),
                bo: None,
                matches: vec![MatchView {
                    id: 1,
                    sport: Sport::Lol,
                    league: "LCK".into(),
                    series_name: String::new(),
                    status: MatchStatus::Upcoming,
                    clock_label: clock.into(),
                    date_label: String::new(),
                    venue_label: venue.into(),
                    venue_name: String::new(),
                    venue_location: String::new(),
                    best_of: "Bo3".into(),
                    team_a: team("A"),
                    team_b: team("B"),
                    league_url: String::new(),
                    begin_at_ms: 0,
                    row_href: None,
                }],
            }],
        }
    }

    #[test]
    fn day_content_hash_reflects_time_format() {
        // Flipping the 12h/24h clock reformats every row's time (e.g. "6:00 PM" →
        // "18:00"). The homepage `<For>` re-renders a day only when its content
        // hash changes, so the hash MUST fold in the formatted time — otherwise
        // toggling the clock leaves the on-screen times stale (the reported bug).
        let twelve = day_content_hash(&day_with_time("6:00 PM", "6:00 PM EDT"));
        let twenty_four = day_content_hash(&day_with_time("18:00", "18:00 EDT"));
        assert_ne!(
            twelve, twenty_four,
            "the formatted time must affect the day content hash"
        );
    }
}
