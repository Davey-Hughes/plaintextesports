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
                        <div class="league">
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
