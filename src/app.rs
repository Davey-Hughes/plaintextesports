use crate::server::{get_day, get_schedule};
use crate::types::{MatchStatus, MatchView, ScheduleView, TeamView};
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
            <ThemeToggle />
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
        <button class="theme-toggle" on:click=toggle>
            {move || if dark.get() { "light mode" } else { "dark mode" }}
        </button>
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
    let schedule = Resource::new(
        move || game.get(),
        |g| async move { get_schedule(g).await },
    );
    setup_autorefresh(schedule);

    view! {
        <GameTabs game set_game />
        <ScheduleSection resource=schedule show_nav=false />
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
    let schedule = Resource::new(
        move || (date(), game.get()),
        |(d, g)| async move { get_day(d, g).await },
    );
    setup_autorefresh(schedule);

    view! {
        <GameTabs game set_game />
        <ScheduleSection resource=schedule show_nav=true />
    }
}

#[component]
fn GameTabs(game: ReadSignal<String>, set_game: WriteSignal<String>) -> impl IntoView {
    let mk = move |label: &'static str, value: &'static str| {
        view! {
            <button
                class="tab"
                class:active=move || game.get() == value
                on:click=move |_| set_game.set(value.to_string())
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
fn ScheduleSection(
    resource: Resource<Result<ScheduleView, ServerFnError>>,
    show_nav: bool,
) -> impl IntoView {
    view! {
        <Suspense fallback=|| view! { <p class="loading">"loading…"</p> }>
            {move || {
                resource
                    .get()
                    .map(|res| match res {
                        Ok(s) => render_schedule(s, show_nav).into_any(),
                        Err(_) => {
                            view! { <p class="error">"Failed to load schedule."</p> }.into_any()
                        }
                    })
            }}
        </Suspense>
    }
}

fn render_schedule(s: ScheduleView, show_nav: bool) -> impl IntoView {
    let ScheduleView {
        live,
        days,
        fetched_label,
        stale,
        using_fixture,
        date_label,
        prev_date,
        next_date,
        ..
    } = s;

    let empty = live.is_empty() && days.iter().all(|d| d.matches.is_empty());

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

    let live_section = (!live.is_empty()).then(|| {
        view! {
            <section class="group">
                <h2 class="group-title live-title">"Live now"</h2>
                <div class="cards">{cards(live)}</div>
            </section>
        }
    });

    let day_sections = days
        .into_iter()
        .map(|d| {
            view! {
                <section class="group">
                    <h2 class="group-title">{d.day_label}</h2>
                    <div class="cards">{cards(d.matches)}</div>
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
        {live_section}
        {day_sections}
        {empty.then(|| view! { <p class="empty">"No tier-1 matches in this window."</p> })}
    }
}

fn cards(matches: Vec<MatchView>) -> impl IntoView {
    matches
        .into_iter()
        .map(|m| view! { <MatchCard m=m /> })
        .collect_view()
}

#[component]
fn MatchCard(m: MatchView) -> impl IntoView {
    let status_class = match m.status {
        MatchStatus::Live => "live",
        MatchStatus::Finished => "final",
        MatchStatus::Canceled => "canceled",
        MatchStatus::Upcoming => "upcoming",
    };

    let meta = if m.best_of.is_empty() {
        format!("{} · {}", m.game.label(), m.league)
    } else {
        format!("{} · {} · {}", m.game.label(), m.league, m.best_of)
    };

    let inner = view! {
        <div class="card-top">
            <span class="card-league">{meta}</span>
            <span class=format!("card-status {status_class}")>{m.time_label}</span>
        </div>
        <TeamRow team=m.team_a />
        <TeamRow team=m.team_b />
    };

    match m.stream_url {
        Some(url) => view! {
            <a class=format!("card {status_class}") href=url target="_blank" rel="noreferrer">
                {inner}
            </a>
        }
        .into_any(),
        None => view! { <div class=format!("card {status_class}")>{inner}</div> }.into_any(),
    }
}

#[component]
fn TeamRow(team: TeamView) -> impl IntoView {
    let score = team.score.map(|s| s.to_string()).unwrap_or_default();
    view! {
        <div class="team" class:winner=team.winner>
            <span class="team-name">{team.label}</span>
            <span class="team-score">{score}</span>
        </div>
    }
}
