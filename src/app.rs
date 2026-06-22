use crate::server::{get_day, get_schedule};
use crate::types::{MatchStatus, MatchView, ScheduleView};
use leptos::prelude::*;
use leptos_meta::{provide_meta_context, MetaTags, Stylesheet, Title};
use leptos_router::{
    components::{Route, Router, Routes, A},
    hooks::use_params,
    params::Params,
    ParamSegment, StaticSegment,
};

#[must_use]
pub fn shell(options: LeptosOptions) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8" />
                <meta name="viewport" content="width=device-width, initial-scale=1" />
                <link rel="icon" href="data:," />
                <link rel="preload" r#as="style" href="/pkg/plaintextesports.css" />
                <AutoReload options=options.clone() />
                <HydrationScripts options />
                <MetaTags />
                // Apply the saved theme before paint to avoid a flash.
                <script>
                    r#"(function(){try{var t=localStorage.getItem('theme')||'dark';document.documentElement.setAttribute('data-theme',t);}catch(e){document.documentElement.setAttribute('data-theme','dark');}})();"#
                </script>
            </head>
            <body>
                <App />
            </body>
        </html>
    }
}

#[component]
#[must_use]
pub fn App() -> impl IntoView {
    provide_meta_context();

    // Shared user preferences: 24h time (default on) and the detected IANA
    // timezone ("" => server default until the browser reports one).
    let hour24 = RwSignal::new(true);
    let tz = RwSignal::new(String::new());
    provide_context(hour24);
    provide_context(tz);

    // After hydration, pick up the browser's timezone + saved hour preference.
    // (Runs client-side only; the initial SSR/hydrate render uses the defaults
    // above, so there's no hydration mismatch.)
    Effect::new(move |_| {
        #[cfg(feature = "hydrate")]
        {
            if let Some(z) = detect_tz() {
                tz.set(z);
            }
            if let Some(h) = load_hour24_pref() {
                hour24.set(h);
            }
        }
    });

    view! {
        <Stylesheet id="leptos" href="/pkg/plaintextesports.css" />
        <Title text="plaintextesports" />
        <Router>
            <div class="page">
                <SiteHeader />
                <main class="main">
                    <Routes fallback=|| view! { <p class="empty">"Page not found."</p> }>
                        <Route path=StaticSegment("") view=HomePage />
                        <Route path=(StaticSegment("day"), ParamSegment("date")) view=DayPage />
                    </Routes>
                </main>
                <SiteFooter />
            </div>
        </Router>
    }
}

#[component]
fn SiteHeader() -> impl IntoView {
    view! {
        <header class="header">
            <div class="brand">
                <A href="/">"plaintextesports"</A>
            </div>
            <div class="toggles">
                <HourToggle />
                <ThemeToggle />
            </div>
        </header>
    }
}

#[component]
fn SiteFooter() -> impl IntoView {
    view! {
        <footer class="footer">
            <span>"tier-1 cs2 + lol schedules"</span>
            <span class="sep">" · "</span>
            <span>"data via PandaScore"</span>
        </footer>
    }
}

#[component]
fn ThemeToggle() -> impl IntoView {
    // Default to dark; sync to the real value after mount (client only).
    let (dark, set_dark) = signal(true);

    Effect::new(move |_| {
        #[cfg(feature = "hydrate")]
        {
            if let Some(win) = web_sys::window() {
                let is_dark = win
                    .local_storage()
                    .ok()
                    .flatten()
                    .and_then(|s| s.get_item("theme").ok().flatten())
                    .is_none_or(|t| t != "light");
                set_dark.set(is_dark);
            }
        }
    });

    let toggle = move |_| {
        let next = !dark.get_untracked();
        set_dark.set(next);
        #[cfg(feature = "hydrate")]
        {
            let theme = if next { "dark" } else { "light" };
            if let Some(win) = web_sys::window() {
                if let Some(root) = win.document().and_then(|d| d.document_element()) {
                    let _ = root.set_attribute("data-theme", theme);
                }
                if let Ok(Some(storage)) = win.local_storage() {
                    let _ = storage.set_item("theme", theme);
                }
            }
        }
    };

    view! {
        <button class="toggle" on:click=toggle>
            {move || if dark.get() { "light mode" } else { "dark mode" }}
        </button>
    }
}

#[component]
fn HourToggle() -> impl IntoView {
    let hour24 = use_context::<RwSignal<bool>>().expect("hour24 context");
    let toggle = move |_| {
        let next = !hour24.get_untracked();
        hour24.set(next);
        #[cfg(feature = "hydrate")]
        save_hour24_pref(next);
    };
    view! {
        <button class="toggle" on:click=toggle>
            {move || if hour24.get() { "24h" } else { "12h" }}
        </button>
    }
}

#[cfg(feature = "hydrate")]
fn detect_tz() -> Option<String> {
    use wasm_bindgen::JsValue;
    let fmt = js_sys::Intl::DateTimeFormat::new(&js_sys::Array::new(), &js_sys::Object::new());
    let opts = fmt.resolved_options();
    js_sys::Reflect::get(&opts, &JsValue::from_str("timeZone"))
        .ok()
        .and_then(|v| v.as_string())
        .filter(|s| !s.is_empty())
}

#[cfg(feature = "hydrate")]
fn load_hour24_pref() -> Option<bool> {
    let storage = web_sys::window()?.local_storage().ok().flatten()?;
    let v = storage.get_item("hour24").ok().flatten()?;
    Some(v != "0")
}

#[cfg(feature = "hydrate")]
fn save_hour24_pref(hour24: bool) {
    if let Some(win) = web_sys::window() {
        if let Ok(Some(storage)) = win.local_storage() {
            let _ = storage.set_item("hour24", if hour24 { "1" } else { "0" });
        }
    }
}

/// Refetch the schedule periodically so a left-open tab stays current.
fn setup_autorefresh(resource: Resource<Result<ScheduleView, ServerFnError>>) {
    Effect::new(move |_| {
        #[cfg(feature = "hydrate")]
        {
            use std::time::Duration;
            set_interval(move || resource.refetch(), Duration::from_secs(60));
        }
        let _ = resource;
    });
}

#[component]
fn HomePage() -> impl IntoView {
    let (game, set_game) = signal(String::from("all"));
    let (league, set_league) = signal(String::new());
    let hour24 = use_context::<RwSignal<bool>>().expect("hour24 context");
    let tz = use_context::<RwSignal<String>>().expect("tz context");
    let schedule = Resource::new(
        move || (game.get(), tz.get(), hour24.get()),
        |(g, z, h)| async move { get_schedule(g, z, h).await },
    );
    setup_autorefresh(schedule);

    view! {
        <GameTabs game set_game set_league />
        <ScheduleSection resource=schedule league set_league show_nav=false />
    }
}

#[derive(Params, PartialEq, Clone)]
struct DayParams {
    date: String,
}

#[component]
fn DayPage() -> impl IntoView {
    let params = use_params::<DayParams>();
    let date = move || params.get().ok().map(|p| p.date).unwrap_or_default();
    let (game, set_game) = signal(String::from("all"));
    let (league, set_league) = signal(String::new());
    let hour24 = use_context::<RwSignal<bool>>().expect("hour24 context");
    let tz = use_context::<RwSignal<String>>().expect("tz context");
    let schedule = Resource::new(
        move || (date(), game.get(), tz.get(), hour24.get()),
        |(d, g, z, h)| async move { get_day(d, g, z, h).await },
    );
    setup_autorefresh(schedule);

    view! {
        <GameTabs game set_game set_league />
        <ScheduleSection resource=schedule league set_league show_nav=true />
    }
}

#[component]
fn GameTabs(
    game: ReadSignal<String>,
    set_game: WriteSignal<String>,
    set_league: WriteSignal<String>,
) -> impl IntoView {
    let mk = move |label: &'static str, value: &'static str| {
        view! {
            <button
                class="tab"
                class:active=move || game.get() == value
                on:click=move |_| {
                    set_game.set(value.to_string());
                    // Reset the event filter: leagues differ per game.
                    set_league.set(String::new());
                }
            >
                {label}
            </button>
        }
    };

    view! {
        <div class="tabs">
            {mk("All", "all")}
            {mk("CS2", "cs2")}
            {mk("LoL", "lol")}
        </div>
    }
}

#[component]
fn LeagueChips(
    leagues: Vec<String>,
    selected: ReadSignal<String>,
    set_league: WriteSignal<String>,
) -> impl IntoView {
    let all_active = move || selected.get().is_empty();
    let chips = leagues
        .into_iter()
        .map(|name| {
            let lc = league_color_class(&name);
            let sel_name = name.clone();
            let click_name = name.clone();
            let is_active = move || selected.get() == sel_name;
            view! {
                <button
                    class=format!("chip {lc}")
                    class:active=is_active
                    on:click=move |_| set_league.set(click_name.clone())
                >
                    {name}
                </button>
            }
        })
        .collect_view();

    view! {
        <div class="chips">
            <button
                class="chip"
                class:active=all_active
                on:click=move |_| set_league.set(String::new())
            >
                "All events"
            </button>
            {chips}
        </div>
    }
}

#[component]
fn ScheduleSection(
    resource: Resource<Result<ScheduleView, ServerFnError>>,
    league: ReadSignal<String>,
    set_league: WriteSignal<String>,
    show_nav: bool,
) -> impl IntoView {
    view! {
        <Suspense fallback=|| view! { <p class="loading">"loading…"</p> }>
            {move || {
                resource
                    .get()
                    .map(|res| match res {
                        Ok(s) => {
                            let leagues = distinct_leagues(&s);
                            let filtered = filter_by_league(s, &league.get());
                            // Only offer event chips when there's more than one.
                            let chips = (leagues.len() > 1)
                                .then(move || {
                                    view! {
                                        <LeagueChips
                                            leagues=leagues
                                            selected=league
                                            set_league=set_league
                                        />
                                    }
                                });
                            view! {
                                {chips}
                                {render_schedule(filtered, show_nav)}
                            }
                                .into_any()
                        }
                        Err(_) => {
                            view! { <p class="error">"Failed to load schedule."</p> }.into_any()
                        }
                    })
            }}
        </Suspense>
    }
}

/// Distinct league names across all days, in first-appearance order.
fn distinct_leagues(s: &ScheduleView) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for day in &s.days {
        for lg in &day.leagues {
            if !out.iter().any(|l| l == &lg.league) {
                out.push(lg.league.clone());
            }
        }
    }
    out
}

/// Keep only the named league (empty = keep all); drop days left empty.
fn filter_by_league(mut s: ScheduleView, league: &str) -> ScheduleView {
    if league.is_empty() {
        return s;
    }
    for day in &mut s.days {
        day.leagues.retain(|lg| lg.league == league);
    }
    s.days.retain(|d| !d.leagues.is_empty());
    s
}

/// Number of palette colors leagues are hashed into (see `.lc-*` in the SCSS).
const LEAGUE_COLORS: u32 = 10;

/// Stable color class for a league name. Uses FNV-1a so the server and client
/// (WASM) compute the identical class — no hydration mismatch — and each event
/// keeps the same color across days.
fn league_color_class(name: &str) -> String {
    let mut h: u32 = 2_166_136_261;
    for b in name.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(16_777_619);
    }
    format!("lc-{}", h % LEAGUE_COLORS)
}

fn render_schedule(s: ScheduleView, show_nav: bool) -> impl IntoView {
    let ScheduleView {
        days,
        fetched_label,
        stale,
        using_fixture,
        date_label,
        prev_date,
        next_date,
        ..
    } = s;

    let empty = days.iter().all(|d| d.leagues.is_empty());

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

    let day_sections = days
        .into_iter()
        .map(|d| {
            let leagues = d
                .leagues
                .into_iter()
                .map(|lg| {
                    let lc = league_color_class(&lg.league);
                    let show_bo = lg.bo.is_none();
                    let header = match &lg.bo {
                        Some(bo) => format!("{} · {}", lg.league, bo),
                        None => lg.league.clone(),
                    };
                    let rows = lg
                        .matches
                        .into_iter()
                        .map(|m| view! { <MatchRow m=m show_bo=show_bo /> })
                        .collect_view();
                    view! {
                        <div class=format!("league {lc}")>
                            <h3 class="league-title">{header}</h3>
                            <div class="rows">{rows}</div>
                        </div>
                    }
                })
                .collect_view();
            view! {
                <section class="day">
                    <h2 class="day-title">{d.day_label}</h2>
                    {leagues}
                </section>
            }
        })
        .collect_view();

    let fixture_note = using_fixture
        .then(|| "demo data — set PANDASCORE_TOKEN for live schedules · ".to_string())
        .unwrap_or_default();
    let stale_note = stale
        .then(|| "data may be stale · ".to_string())
        .unwrap_or_default();

    view! {
        {nav}
        <div class="status-line">{fixture_note}{stale_note} "loaded " {fetched_label}</div>
        {day_sections}
        {empty.then(|| view! { <p class="empty">"No tier-1 matches in this window."</p> })}
    }
}

#[component]
fn MatchRow(m: MatchView, show_bo: bool) -> impl IntoView {
    let status_class = match m.status {
        MatchStatus::Live => "live",
        MatchStatus::Finished => "final",
        MatchStatus::Canceled => "canceled",
        MatchStatus::Upcoming => "upcoming",
    };
    let badge = match m.status {
        MatchStatus::Live => "LIVE",
        MatchStatus::Finished => "Final",
        MatchStatus::Canceled => "Canc.",
        MatchStatus::Upcoming => "",
    };

    let scored = matches!((m.team_a.score, m.team_b.score), (Some(_), Some(_)));
    let mid = match (m.team_a.score, m.team_b.score) {
        (Some(a), Some(b)) => format!("{a} – {b}"),
        _ => "vs".to_string(),
    };
    let mid_class = if scored { "row-mid scored" } else { "row-mid" };
    let bo = if show_bo { m.best_of } else { String::new() };

    let inner = view! {
        <span class="row-time">{m.clock_label}</span>
        <span class="row-team row-a" class:winner=m.team_a.winner>{m.team_a.label}</span>
        <span class=mid_class>{mid}</span>
        <span class="row-team row-b" class:winner=m.team_b.winner>{m.team_b.label}</span>
        <span class="row-meta">
            <span class=format!("row-badge {status_class}")>{badge}</span>
            <span class="row-bo">{bo}</span>
        </span>
    };

    match m.stream_url {
        Some(url) => view! {
            <a class=format!("row {status_class}") href=url target="_blank" rel="noreferrer">
                {inner}
            </a>
        }
        .into_any(),
        None => view! { <div class=format!("row {status_class}")>{inner}</div> }.into_any(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{DayGroup, Game, LeagueGroup, TeamView};

    fn mv(league: &str) -> MatchView {
        MatchView {
            id: 1,
            game: Game::Lol,
            league: league.into(),
            tier: "S".into(),
            status: MatchStatus::Upcoming,
            clock_label: String::new(),
            best_of: "Bo3".into(),
            team_a: TeamView {
                label: "A".into(),
                score: None,
                winner: false,
            },
            team_b: TeamView {
                label: "B".into(),
                score: None,
                winner: false,
            },
            stream_url: None,
            begin_at_ms: 0,
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
    fn distinct_leagues_in_first_appearance_order() {
        let s = sched(vec![vec!["LCK", "LEC"], vec!["LCK", "LPL"]]);
        assert_eq!(distinct_leagues(&s), vec!["LCK", "LEC", "LPL"]);
    }

    #[test]
    fn filter_by_league_keeps_only_match_and_drops_empty_days() {
        let s = sched(vec![vec!["LCK", "LEC"], vec!["LPL"]]);
        let f = filter_by_league(s, "LCK");
        assert_eq!(f.days.len(), 1);
        assert_eq!(f.days[0].leagues.len(), 1);
        assert_eq!(f.days[0].leagues[0].league, "LCK");
    }

    #[test]
    fn filter_by_league_empty_keeps_everything() {
        let s = sched(vec![vec!["LCK", "LEC"]]);
        let f = filter_by_league(s, "");
        assert_eq!(f.days[0].leagues.len(), 2);
    }
}
