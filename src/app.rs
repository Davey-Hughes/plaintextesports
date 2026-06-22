use crate::server::{get_day, get_schedule, get_site};
use crate::types::{Game, MatchStatus, MatchView, ScheduleView};
use leptos::prelude::*;
use std::collections::HashSet;
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

/// Set of event/league names whose scores are individually revealed.
#[derive(Clone, Copy)]
struct RevealedEvents(RwSignal<HashSet<String>>);

/// Set of "kind|value" scopes the user is subscribed to (game/event reminders).
#[derive(Clone, Copy)]
struct Subscribed(RwSignal<HashSet<String>>);

#[component]
#[must_use]
pub fn App() -> impl IntoView {
    provide_meta_context();

    // Shared user preferences: 24h time (default on) and the detected IANA
    // timezone ("" => server default until the browser reports one).
    let hour24 = RwSignal::new(true);
    let tz = RwSignal::new(String::new());
    // Reminder state: starred match ids, and the VAPID public key (None until
    // fetched / if Web Push isn't configured).
    let starred = RwSignal::new(HashSet::<i64>::new());
    let vapid = RwSignal::new(None::<String>);
    // Spoiler control: a global reveal + a per-event reveal set.
    let show_scores = RwSignal::new(false);
    let revealed = RwSignal::new(HashSet::<String>::new());
    let subscribed = RwSignal::new(HashSet::<String>::new());
    provide_context(hour24);
    provide_context(tz);
    provide_context(starred);
    provide_context(vapid);
    provide_context(ShowScores(show_scores));
    provide_context(RevealedEvents(revealed));
    provide_context(Subscribed(subscribed));

    // After hydration, pick up the browser's timezone + saved preferences and
    // the push key. (Client-side only; the initial render uses the defaults
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
            show_scores.set(load_scores_pref());
            starred.set(load_starred());
            subscribed.set(load_subs());
            leptos::task::spawn_local(async move {
                if let Ok(k) = crate::server::get_vapid_key().await {
                    vapid.set(k);
                }
            });
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
                <ScoresToggle />
                <HourToggle />
                <ThemeToggle />
            </div>
        </header>
    }
}

#[component]
fn SiteFooter() -> impl IntoView {
    let site = Resource::new(|| (), |()| async { get_site().await });
    view! {
        <footer class="footer">
            <span>"tier-1 cs2 + lol schedules"</span>
            <span class="sep">" · "</span>
            <span>"data via PandaScore"</span>
            <Suspense>
                {move || {
                    site.get()
                        .and_then(Result::ok)
                        .map(|s| {
                            let copyright = s
                                .copyright
                                .map(|c| view! { <span class="sep">" · "</span><span>{c}</span> });
                            let links = s
                                .links
                                .into_iter()
                                .map(|l| {
                                    view! {
                                        <span class="sep">" · "</span>
                                        <a href=l.url target="_blank" rel="noreferrer">
                                            {l.label}
                                        </a>
                                    }
                                })
                                .collect_view();
                            view! { {copyright}{links} }
                        })
                }}
            </Suspense>
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

#[component]
fn ScoresToggle() -> impl IntoView {
    let show = use_context::<ShowScores>().expect("show_scores context").0;
    let toggle = move |_| {
        let next = !show.get_untracked();
        show.set(next);
        #[cfg(feature = "hydrate")]
        save_scores_pref(next);
    };
    view! {
        <button class="toggle" on:click=toggle>
            {move || if show.get() { "hide scores" } else { "show scores" }}
        </button>
    }
}

/// Per-event score reveal button (shown on a league header that has results).
#[component]
fn EventScoreToggle(league: String) -> impl IntoView {
    let global = use_context::<ShowScores>().map(|s| s.0);
    let revealed = use_context::<RevealedEvents>().expect("revealed context").0;
    let l_active = league.clone();
    let is_on = Memo::new(move |_| revealed.with(|s| s.contains(&l_active)));
    let l_click = league;
    let on_click = move |_| {
        revealed.update(|s| {
            if s.contains(&l_click) {
                s.remove(&l_click);
            } else {
                s.insert(l_click.clone());
            }
        });
    };
    // When the global reveal is on, everything is shown — hide the per-event button.
    let hidden = move || global.is_some_and(|g| g.get());
    view! {
        <button
            class="event-score"
            class:on=move || is_on.get()
            class:event-hidden=hidden
            on:click=on_click
        >
            {move || if is_on.get() { "hide score" } else { "show score" }}
        </button>
    }
}

/// ★ toggle to subscribe to a whole game (`kind="game"`, value "cs2"/"lol") or
/// event (`kind="league"`, value = league name). Used inside a game filter tab
/// and in event headers. Hidden unless push is on.
#[component]
fn SubscribeStar(kind: &'static str, value: String) -> impl IntoView {
    let subscribed = use_context::<Subscribed>().expect("subscribed context").0;
    let vapid = use_context::<RwSignal<Option<String>>>().expect("vapid context");
    let key = format!("{kind}|{value}");
    let key_active = key.clone();
    let is_on = Memo::new(move |_| subscribed.with(|s| s.contains(&key_active)));
    let hidden = move || vapid.with(|v| v.is_none());

    let on_click = move |_| {
        let now_on = subscribed.with_untracked(|s| s.contains(&key));
        subscribed.update(|s| {
            if now_on {
                s.remove(&key);
            } else {
                s.insert(key.clone());
            }
        });
        #[cfg(feature = "hydrate")]
        {
            let keys: Vec<String> = subscribed.with_untracked(|s| s.iter().cloned().collect());
            subscribe_scope(kind.to_string(), value.clone(), !now_on, keys, vapid.get_untracked());
        }
        #[cfg(not(feature = "hydrate"))]
        {
            let _ = (now_on, &value, vapid);
        }
    };

    view! {
        <button
            class="sub-star"
            class:on=move || is_on.get()
            class:event-hidden=hidden
            on:click=on_click
            title="Notify me about this"
            aria-label="Subscribe to notifications"
        >
            {move || if is_on.get() { "★" } else { "☆" }}
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

#[cfg(feature = "hydrate")]
fn load_scores_pref() -> bool {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item("scores").ok().flatten())
        .is_some_and(|v| v == "1")
}

#[cfg(feature = "hydrate")]
fn save_scores_pref(show: bool) {
    if let Some(win) = web_sys::window() {
        if let Ok(Some(storage)) = win.local_storage() {
            let _ = storage.set_item("scores", if show { "1" } else { "0" });
        }
    }
}

#[cfg(feature = "hydrate")]
fn save_subs(keys: &[String]) {
    if let Some(win) = web_sys::window() {
        if let Ok(Some(storage)) = win.local_storage() {
            let _ = storage.set_item("subs", &keys.join("\n"));
        }
    }
}

#[cfg(feature = "hydrate")]
fn load_subs() -> HashSet<String> {
    let mut out = HashSet::new();
    if let Some(win) = web_sys::window() {
        if let Ok(Some(storage)) = win.local_storage() {
            if let Ok(Some(v)) = storage.get_item("subs") {
                for part in v.split('\n').filter(|p| !p.is_empty()) {
                    out.insert(part.to_string());
                }
            }
        }
    }
    out
}

/// Persist the subscribed set and (un)register the game/event subscription.
#[cfg(feature = "hydrate")]
fn subscribe_scope(kind: String, value: String, subscribing: bool, keys: Vec<String>, vapid: Option<String>) {
    save_subs(&keys);
    let Some(vapid) = vapid else { return };
    leptos::task::spawn_local(async move {
        let sub = match pte_subscribe(&vapid).await {
            Ok(s) => s,
            Err(_) => return,
        };
        let (Some(endpoint), Some(p256dh), Some(auth)) = (
            reflect_str(&sub, "endpoint"),
            reflect_str(&sub, "p256dh"),
            reflect_str(&sub, "auth"),
        ) else {
            return;
        };
        if subscribing {
            let _ = crate::server::add_subscription(crate::types::SubscribeReq {
                sub: crate::types::PushSub {
                    endpoint,
                    p256dh,
                    auth,
                },
                kind,
                value,
            })
            .await;
        } else {
            let _ = crate::server::remove_subscription(endpoint, kind, value).await;
        }
    });
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
    // A plain filter tab (used for "All", which has no game to subscribe to).
    let plain = move |label: &'static str, value: &'static str| {
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
    // A game filter tab with an embedded ★ that subscribes to the whole game.
    // The label clicks to filter; the ★ clicks to (un)subscribe.
    let with_star = move |label: &'static str, value: &'static str| {
        view! {
            <span class="tab tab-with-star" class:active=move || game.get() == value>
                <button
                    class="tab-label"
                    on:click=move |_| {
                        set_game.set(value.to_string());
                        set_league.set(String::new());
                    }
                >
                    {label}
                </button>
                <SubscribeStar kind="game" value=value.to_string() />
            </span>
        }
    };

    view! {
        <div class="tabs">
            {plain("All", "all")}
            {with_star("CS2", "cs2")}
            {with_star("LoL", "lol")}
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
                            // Show ★ reminder buttons only when Web Push is configured.
                            let push = use_context::<RwSignal<Option<String>>>()
                                .is_some_and(|v| v.get().is_some());
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
                                {render_schedule(filtered, show_nav, push)}
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

fn render_schedule(s: ScheduleView, show_nav: bool, push: bool) -> impl IntoView {
    let ScheduleView {
        days,
        fetched_label,
        stale,
        using_fixture,
        demo_forced,
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
                    let event_url = lg.event_url;
                    // Only events with results need a per-event reveal button.
                    let has_scores = lg
                        .matches
                        .iter()
                        .any(|m| m.team_a.score.is_some() && m.team_b.score.is_some());
                    let league_name = lg.league.clone();
                    let sub_value = lg.league.clone();
                    let rows = lg
                        .matches
                        .into_iter()
                        .map(|m| view! { <MatchRow m=m show_bo=show_bo push=push /> })
                        .collect_view();
                    let score_toggle =
                        has_scores.then(|| view! { <EventScoreToggle league=league_name /> });
                    view! {
                        <div class=format!("league {lc}")>
                            <div class="league-head">
                                <h3 class="league-title">
                                    <a href=event_url target="_blank" rel="noreferrer">{header}</a>
                                </h3>
                                <span class="event-controls">
                                    <SubscribeStar kind="league" value=sub_value />
                                    {score_toggle}
                                </span>
                            </div>
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

    let fixture_note = if demo_forced {
        "demo mode (forced) · ".to_string()
    } else if using_fixture {
        "demo data — set PANDASCORE_TOKEN for live schedules · ".to_string()
    } else {
        String::new()
    };
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

/// Lead time before a match start to fire its reminder.
const REMIND_LEAD_MS: i64 = 10 * 60 * 1000;

/// Everything the reminder for a match needs (built from a `MatchView`).
/// Fields are consumed only in the `hydrate` build (the ★ click handler).
#[derive(Clone)]
#[cfg_attr(not(feature = "hydrate"), allow(dead_code))]
struct ReminderInfo {
    id: i64,
    game: Game,
    league: String,
    notify_at_ms: i64,
    title: String,
    body: String,
    url: String,
}

#[component]
fn MatchRow(m: MatchView, show_bo: bool, push: bool) -> impl IntoView {
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

    let sa = m.team_a.score;
    let sb = m.team_b.score;
    let has = sa.is_some() && sb.is_some();
    let win_a = m.team_a.winner;
    let win_b = m.team_b.winner;
    let bo = if show_bo { m.best_of } else { String::new() };

    let info = ReminderInfo {
        id: m.id,
        game: m.game,
        league: m.league.clone(),
        notify_at_ms: m.begin_at_ms - REMIND_LEAD_MS,
        title: format!("{} vs {}", m.team_a.label, m.team_b.label),
        body: format!("{} · {}", m.league, m.clock_label),
        url: if m.event_url.is_empty() {
            "/".to_string()
        } else {
            m.event_url.clone()
        },
    };

    // Scores are spoilers: reveal only when the global toggle is on or this
    // event is individually revealed.
    let global = use_context::<ShowScores>().map(|s| s.0);
    let revealed = use_context::<RevealedEvents>().map(|r| r.0);
    let league = m.league.clone();
    let reveal = Memo::new(move |_| {
        global.is_some_and(|g| g.get())
            || revealed.is_some_and(|r| r.with(|set| set.contains(&league)))
    });

    let inner = view! {
        <span class="row-time">{m.clock_label}</span>
        <span class="row-team row-a" class:winner=move || reveal.get() && win_a>
            {m.team_a.label}
        </span>
        <span class="row-mid" class:scored=move || reveal.get() && has>
            {move || {
                if reveal.get() && has {
                    format!("{} – {}", sa.unwrap_or(0), sb.unwrap_or(0))
                } else {
                    "vs".to_string()
                }
            }}
        </span>
        <span class="row-team row-b" class:winner=move || reveal.get() && win_b>
            {m.team_b.label}
        </span>
        <span class="row-meta">
            <span class=format!("row-badge {status_class}")>{badge}</span>
            <span class="row-bo">{bo}</span>
        </span>
    };

    // The body is the (optional) stream link; `display: contents` lets its
    // children participate in the row grid alongside the ★ button.
    let body = match m.stream_url {
        Some(url) => view! {
            <a class="row-body" href=url target="_blank" rel="noreferrer">
                {inner}
            </a>
        }
        .into_any(),
        None => view! { <span class="row-body">{inner}</span> }.into_any(),
    };

    if push {
        // Only upcoming matches are remindable; others get an empty placeholder
        // so the grid columns stay aligned.
        let star = if matches!(m.status, MatchStatus::Upcoming) {
            view! { <StarButton id=m.id info=info /> }.into_any()
        } else {
            view! { <span class="star star-empty"></span> }.into_any()
        };
        view! {
            <div class=format!("row has-star {status_class}")>
                {star}
                {body}
            </div>
        }
        .into_any()
    } else {
        view! { <div class=format!("row {status_class}")>{body}</div> }.into_any()
    }
}

#[component]
fn StarButton(id: i64, info: ReminderInfo) -> impl IntoView {
    let starred = use_context::<RwSignal<HashSet<i64>>>().expect("starred context");
    let subscribed = use_context::<Subscribed>().expect("subscribed context").0;
    let vapid = use_context::<RwSignal<Option<String>>>().expect("vapid context");
    // The ★ lights up if this match is starred individually *or* a higher-level
    // subscription (the whole game, or this event) already covers it — so the
    // star reflects everything you'll be notified about.
    let game_key = format!("game|{}", info.game.slug());
    let league_key = format!("league|{}", info.league);
    let is_on = Memo::new(move |_| {
        starred.with(|s| s.contains(&id))
            || subscribed.with(|s| s.contains(&game_key) || s.contains(&league_key))
    });

    let on_click = move |_| {
        let now_on = starred.get_untracked().contains(&id);
        starred.update(|s| {
            if now_on {
                s.remove(&id);
            } else {
                s.insert(id);
            }
        });
        #[cfg(feature = "hydrate")]
        {
            let ids: Vec<i64> = starred.get_untracked().iter().copied().collect();
            persist_and_sync(info.clone(), !now_on, ids, vapid.get_untracked());
        }
        #[cfg(not(feature = "hydrate"))]
        {
            let _ = (now_on, &info, vapid);
        }
    };

    view! {
        <button
            class="star"
            class:on=move || is_on.get()
            on:click=on_click
            aria-label="Toggle reminder"
        >
            {move || if is_on.get() { "★" } else { "☆" }}
        </button>
    }
}

#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen]
extern "C" {
    // Defined in the shell <script>: registers the SW, requests permission,
    // subscribes, and returns `{ endpoint, p256dh, auth }`.
    #[wasm_bindgen(catch, js_name = pteSubscribe)]
    async fn pte_subscribe(vapid: &str) -> Result<wasm_bindgen::JsValue, wasm_bindgen::JsValue>;
}

#[cfg(feature = "hydrate")]
fn reflect_str(obj: &wasm_bindgen::JsValue, key: &str) -> Option<String> {
    js_sys::Reflect::get(obj, &wasm_bindgen::JsValue::from_str(key))
        .ok()?
        .as_string()
}

#[cfg(feature = "hydrate")]
fn save_starred(ids: &[i64]) {
    if let Some(win) = web_sys::window() {
        if let Ok(Some(storage)) = win.local_storage() {
            let csv = ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
            let _ = storage.set_item("starred", &csv);
        }
    }
}

#[cfg(feature = "hydrate")]
fn load_starred() -> HashSet<i64> {
    let mut out = HashSet::new();
    if let Some(win) = web_sys::window() {
        if let Ok(Some(storage)) = win.local_storage() {
            if let Ok(Some(csv)) = storage.get_item("starred") {
                for part in csv.split(',') {
                    if let Ok(id) = part.trim().parse::<i64>() {
                        out.insert(id);
                    }
                }
            }
        }
    }
    out
}

/// Persist the starred set and (un)register the reminder on the server.
#[cfg(feature = "hydrate")]
fn persist_and_sync(info: ReminderInfo, starring: bool, ids: Vec<i64>, vapid: Option<String>) {
    save_starred(&ids);
    let Some(vapid) = vapid else { return };
    leptos::task::spawn_local(async move {
        let sub = match pte_subscribe(&vapid).await {
            Ok(s) => s,
            Err(_) => return,
        };
        let (Some(endpoint), Some(p256dh), Some(auth)) = (
            reflect_str(&sub, "endpoint"),
            reflect_str(&sub, "p256dh"),
            reflect_str(&sub, "auth"),
        ) else {
            return;
        };
        if starring {
            let req = crate::types::ReminderReq {
                sub: crate::types::PushSub {
                    endpoint,
                    p256dh,
                    auth,
                },
                match_id: info.id,
                game: info.game,
                league: info.league,
                notify_at_ms: info.notify_at_ms,
                title: info.title,
                body: info.body,
                url: info.url,
            };
            let _ = crate::server::add_reminder(req).await;
        } else {
            let _ = crate::server::remove_reminder(endpoint, info.id).await;
        }
    });
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
            event_url: String::new(),
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
