use crate::server::{
    get_day, get_event_info, get_match_detail, get_range, get_schedule, get_site,
};
use crate::types::{
    BracketRound, EventInfo, Game, MatchDetail, MatchStatus, MatchView, ScheduleView, StandingRow,
    StreamView,
};
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

/// Set of match ids whose scores are individually revealed (by clicking the
/// match's "Final" badge).
#[derive(Clone, Copy)]
struct RevealedMatches(RwSignal<HashSet<i64>>);

/// Set of "kind|value" scopes the user is subscribed to (game/event reminders).
#[derive(Clone, Copy)]
struct Subscribed(RwSignal<HashSet<String>>);

/// Calendar-selected date range (start, end ISO dates); `None` = default view.
#[derive(Clone, Copy)]
struct DateRange(RwSignal<Option<(String, String)>>);

/// How many extra past days the "‹ show earlier days" control has revealed.
#[derive(Clone, Copy)]
struct EarlierDays(RwSignal<i64>);

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
    // Spoiler control: a global reveal + a per-match reveal set.
    let show_scores = RwSignal::new(false);
    let revealed = RwSignal::new(HashSet::<i64>::new());
    let subscribed = RwSignal::new(HashSet::<String>::new());
    // History views: a calendar-selected range, and the "earlier days" expansion.
    let range = RwSignal::new(None::<(String, String)>);
    let earlier = RwSignal::new(0i64);
    provide_context(hour24);
    provide_context(tz);
    provide_context(starred);
    provide_context(vapid);
    provide_context(ShowScores(show_scores));
    provide_context(RevealedMatches(revealed));
    provide_context(Subscribed(subscribed));
    provide_context(DateRange(range));
    provide_context(EarlierDays(earlier));

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
            range.set(load_range());
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
                        <Route path=(StaticSegment("match"), ParamSegment("id")) view=MatchDetailPage />
                        <Route path=StaticSegment("about") view=AboutPage />
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
                <CalendarPicker />
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
            <A href="/about">"about"</A>
            <span class="sep">" · "</span>
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
fn AboutPage() -> impl IntoView {
    view! {
        <article class="about">
            <h1>"about"</h1>
            <p>
                "plaintextesports is a fast, no-frills schedule for "
                <strong>"tier-1 Counter-Strike 2 and League of Legends"</strong>
                " — only the top events, no lower-tier noise. Match times are shown in your "
                "own timezone, and your light/dark and 12h/24h choices are remembered."
            </p>

            <h2>"seeing scores"</h2>
            <p>
                "Scores are hidden by default so you can browse finished matches without "
                "spoilers. There are two ways to reveal them:"
            </p>
            <ul>
                <li>
                    "Click a match's " <span class="kbd">"Final"</span> " (or "
                    <span class="kbd">"LIVE"</span>
                    ") label to reveal just that match's score (click again to hide it)."
                </li>
                <li>
                    "Use the " <span class="kbd">"show scores"</span>
                    " toggle at the top to reveal every score at once."
                </li>
            </ul>

            <h2>"getting notifications"</h2>
            <p>
                "You can get a browser notification shortly before a match starts. Tap a "
                "star to follow something; the first time, your browser will ask permission "
                "to show notifications."
            </p>
            <ul>
                <li>
                    <span class="kbd">"☆"</span>
                    " on a match — remind me about just this match."
                </li>
                <li>
                    <span class="kbd">"☆"</span>
                    " next to a game tab (CS2 / LoL) — notify me about every match in that game."
                </li>
                <li>
                    <span class="kbd">"☆"</span>
                    " in an event's header — notify me about every match in that event."
                </li>
            </ul>
            <p>
                "A filled " <span class="kbd">"★"</span>
                " means you're following it. Game and event subscriptions automatically "
                "include matches added to that game or event later — you don't need to "
                "re-subscribe. (If you don't see any stars, notifications aren't enabled "
                "on this instance.)"
            </p>

            <h2>"about the data"</h2>
            <p>
                "Schedules come from PandaScore and refresh in the background. "
                "A " <span class="kbd">"LIVE"</span>
                " badge is inferred (a match has started but isn't finished); exact live "
                "scores aren't available on the free data tier, so they fill in shortly "
                "after a match ends."
            </p>

            <p class="about-back">
                <A href="/">"← back to the schedule"</A>
            </p>
        </article>
    }
}

#[derive(Params, PartialEq, Clone)]
struct DetailParams {
    id: String,
}

#[component]
fn MatchDetailPage() -> impl IntoView {
    let params = use_params::<DetailParams>();
    let id = move || {
        params
            .get()
            .ok()
            .and_then(|p| p.id.parse::<i64>().ok())
            .unwrap_or(0)
    };
    let hour24 = use_context::<RwSignal<bool>>().expect("hour24 context");
    let tz = use_context::<RwSignal<String>>().expect("tz context");
    let detail = Resource::new(
        move || (id(), tz.get(), hour24.get()),
        |(id, tz, h)| async move { get_match_detail(id, tz, h).await },
    );
    view! {
        <Suspense fallback=|| view! { <p class="loading">"loading…"</p> }>
            {move || {
                detail
                    .get()
                    .map(|res| match res {
                        Ok(d) if d.found => detail_view(d).into_any(),
                        _ => view! { <p class="empty">"Match not found."</p> }.into_any(),
                    })
            }}
        </Suspense>
    }
}

fn detail_view(d: MatchDetail) -> impl IntoView {
    let MatchDetail {
        match_view,
        streams,
        event,
        ..
    } = d;
    let m = match_view.expect("found implies match_view");
    let score = match (m.team_a.score, m.team_b.score) {
        (Some(a), Some(b)) => format!("{a} – {b}"),
        _ => "vs".to_string(),
    };
    let status_label = match m.status {
        MatchStatus::Live => "LIVE",
        MatchStatus::Finished => "Final",
        MatchStatus::Canceled => "Canceled",
        MatchStatus::Upcoming => "",
    };
    let meta = [
        m.league.clone(),
        m.clock_label.clone(),
        m.best_of.clone(),
        status_label.to_string(),
    ]
    .into_iter()
    .filter(|s| !s.is_empty())
    .collect::<Vec<_>>()
    .join(" · ");
    let win_a = m.team_a.winner;
    let win_b = m.team_b.winner;
    view! {
        <article class="detail">
            <A href="/">"← schedule"</A>
            <h1 class="detail-title">
                <span class="detail-team" class:winner=win_a>{m.team_a.label}</span>
                <span class="detail-score">{score}</span>
                <span class="detail-team" class:winner=win_b>{m.team_b.label}</span>
            </h1>
            <div class="detail-meta">{meta}</div>
            <StreamsList streams=streams />
            <StandingsTable rows=event.standings />
            <Bracket rounds=event.rounds />
        </article>
    }
}

/// Build a readable label for a broadcast (host + language/role tags).
fn stream_label(s: &StreamView) -> String {
    let host = s
        .url
        .split_once("//")
        .map_or(s.url.as_str(), |(_, rest)| rest)
        .trim_end_matches('/');
    let mut tags = Vec::new();
    if !s.language.is_empty() {
        tags.push(s.language.to_uppercase());
    }
    if s.main {
        tags.push("main".to_string());
    } else if s.official {
        tags.push("official".to_string());
    }
    if tags.is_empty() {
        host.to_string()
    } else {
        format!("{host} · {}", tags.join(", "))
    }
}

#[component]
fn StreamsList(streams: Vec<StreamView>) -> impl IntoView {
    if streams.is_empty() {
        return ().into_any();
    }
    let items = streams
        .into_iter()
        .map(|s| {
            let label = stream_label(&s);
            let cls = if s.official { "stream official" } else { "stream" };
            view! {
                <li class=cls>
                    <a href=s.url target="_blank" rel="noreferrer">
                        {label}
                    </a>
                </li>
            }
        })
        .collect_view();
    view! {
        <section class="detail-section">
            <h2 class="section-title">"Streams"</h2>
            <ul class="streams">{items}</ul>
        </section>
    }
    .into_any()
}

#[component]
fn StandingsTable(rows: Vec<StandingRow>) -> impl IntoView {
    if rows.is_empty() {
        return ().into_any();
    }
    let body = rows
        .into_iter()
        .map(|r| {
            view! {
                <tr>
                    <td class="st-rank">{r.rank}</td>
                    <td class="st-team">{r.team}</td>
                    <td class="st-wl">{format!("{}-{}", r.wins, r.losses)}</td>
                    <td class="st-diff">{format!("{}-{}", r.game_wins, r.game_losses)}</td>
                </tr>
            }
        })
        .collect_view();
    view! {
        <section class="detail-section">
            <h2 class="section-title">"Standings"</h2>
            <table class="standings">
                <thead>
                    <tr>
                        <th></th>
                        <th class="st-team">"Team"</th>
                        <th>"W-L"</th>
                        <th>"Maps"</th>
                    </tr>
                </thead>
                <tbody>{body}</tbody>
            </table>
        </section>
    }
    .into_any()
}

#[component]
fn Bracket(rounds: Vec<BracketRound>) -> impl IntoView {
    if rounds.is_empty() {
        return ().into_any();
    }
    let cols = rounds
        .into_iter()
        .map(|r| {
            let ms = r
                .matches
                .into_iter()
                .map(|m| {
                    let cls_a = if m.winner == "a" { "bk-row win" } else { "bk-row" };
                    let cls_b = if m.winner == "b" { "bk-row win" } else { "bk-row" };
                    let sa = m.score_a.map(|s| s.to_string()).unwrap_or_default();
                    let sb = m.score_b.map(|s| s.to_string()).unwrap_or_default();
                    view! {
                        <div class="bk-match">
                            <div class=cls_a>
                                <span class="bk-team">{m.team_a}</span>
                                <span class="bk-score">{sa}</span>
                            </div>
                            <div class=cls_b>
                                <span class="bk-team">{m.team_b}</span>
                                <span class="bk-score">{sb}</span>
                            </div>
                        </div>
                    }
                })
                .collect_view();
            view! {
                <div class="bk-round">
                    <div class="bk-round-title">{r.title}</div>
                    <div class="bk-matches">{ms}</div>
                </div>
            }
        })
        .collect_view();
    view! {
        <section class="detail-section">
            <h2 class="section-title">"Bracket"</h2>
            <div class="bracket">{cols}</div>
        </section>
    }
    .into_any()
}

/// Standings + bracket shown beneath the schedule when one event is filtered.
#[component]
fn EventSection(league: ReadSignal<String>) -> impl IntoView {
    let info = Resource::new(
        move || league.get(),
        |lg| async move {
            if lg.is_empty() {
                Ok(EventInfo::default())
            } else {
                get_event_info(lg).await
            }
        },
    );
    view! {
        <Suspense>
            {move || {
                info.get()
                    .and_then(Result::ok)
                    .filter(|e| !e.is_empty())
                    .map(|e| {
                        view! {
                            <div class="event-extra">
                                <StandingsTable rows=e.standings />
                                <Bracket rounds=e.rounds />
                            </div>
                        }
                    })
            }}
        </Suspense>
    }
}

#[component]
fn ThemeToggle() -> impl IntoView {
    // Cycle dark → oled (pure black) → light. Default to dark; sync to the saved
    // value after mount (client only) — matches the pre-paint shell script.
    let theme = RwSignal::new("dark".to_string());

    Effect::new(move |_| {
        #[cfg(feature = "hydrate")]
        {
            if let Some(t) = saved_theme() {
                theme.set(t);
            }
        }
    });

    let cycle = move |_| {
        let next = match theme.get_untracked().as_str() {
            "dark" => "oled",
            "oled" => "light",
            _ => "dark",
        };
        theme.set(next.to_string());
        #[cfg(feature = "hydrate")]
        apply_theme(next);
    };

    view! {
        <button class="toggle" on:click=cycle>
            {move || theme.get()}
        </button>
    }
}

/// The saved theme ("dark"/"light"/"oled"), if a valid one is stored.
#[cfg(feature = "hydrate")]
fn saved_theme() -> Option<String> {
    web_sys::window()?
        .local_storage()
        .ok()
        .flatten()?
        .get_item("theme")
        .ok()
        .flatten()
        .filter(|t| matches!(t.as_str(), "dark" | "light" | "oled"))
}

/// Apply a theme: set `data-theme` on <html> and persist it.
#[cfg(feature = "hydrate")]
fn apply_theme(theme: &str) {
    if let Some(win) = web_sys::window() {
        if let Some(root) = win.document().and_then(|d| d.document_element()) {
            let _ = root.set_attribute("data-theme", theme);
        }
        if let Ok(Some(storage)) = win.local_storage() {
            let _ = storage.set_item("theme", theme);
        }
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

/// `YYYY-MM-DD` for today plus `offset_days` (browser-local date).
#[cfg(feature = "hydrate")]
fn iso_from_today(offset_days: i64) -> String {
    let ms = js_sys::Date::now() + (offset_days as f64) * 86_400_000.0;
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms));
    format!(
        "{:04}-{:02}-{:02}",
        d.get_full_year() as i32,
        d.get_month() as u32 + 1,
        d.get_date() as u32
    )
}

#[cfg(feature = "hydrate")]
fn save_range(range: Option<(String, String)>) {
    if let Some(win) = web_sys::window() {
        if let Ok(Some(storage)) = win.local_storage() {
            match range {
                Some((s, e)) => {
                    let _ = storage.set_item("range", &format!("{s}|{e}"));
                }
                None => {
                    let _ = storage.remove_item("range");
                }
            }
        }
    }
}

#[cfg(feature = "hydrate")]
fn load_range() -> Option<(String, String)> {
    let storage = web_sys::window()?.local_storage().ok().flatten()?;
    let v = storage.get_item("range").ok().flatten()?;
    let (s, e) = v.split_once('|')?;
    (!s.is_empty() && !e.is_empty()).then(|| (s.to_string(), e.to_string()))
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
    let range = use_context::<DateRange>().expect("range context").0;
    let earlier = use_context::<EarlierDays>().expect("earlier context").0;
    // Default = today + future (get_schedule). A calendar range, else an
    // "earlier days" expansion, switches to the range view.
    let schedule = Resource::new(
        move || (range.get(), earlier.get(), game.get(), tz.get(), hour24.get()),
        |(r, e, g, z, h)| async move {
            match r {
                Some((start, end)) => get_range(start, end, g, z, h).await,
                None if e > 0 => {
                    let (start, end) = earlier_window(e);
                    get_range(start, end, g, z, h).await
                }
                None => get_schedule(g, z, h).await,
            }
        },
    );
    setup_autorefresh(schedule);

    view! {
        <GameTabs game set_game set_league />
        <ScheduleSection resource=schedule league set_league show_nav=false />
    }
}

/// The start/end date window for the "earlier days" view: `days_back` days ago
/// through past the upcoming horizon (so all future matches stay visible).
/// Computed client-side; SSR never reaches this branch (earlier starts at 0).
fn earlier_window(days_back: i64) -> (String, String) {
    #[cfg(feature = "hydrate")]
    {
        (iso_from_today(-days_back), iso_from_today(366))
    }
    #[cfg(not(feature = "hydrate"))]
    {
        let _ = days_back;
        (String::new(), String::new())
    }
}

/// Top-of-schedule control: a "‹ show earlier days" expander by default, or the
/// active calendar range with a "clear" when one is selected.
#[component]
fn EarlierControl() -> impl IntoView {
    let range = use_context::<DateRange>().expect("range context").0;
    let earlier = use_context::<EarlierDays>().expect("earlier context").0;

    let on_earlier = move |_| earlier.update(|n| *n += 3);
    let on_reset = move |_| earlier.set(0);
    let on_clear = move |_| {
        earlier.set(0);
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

// ----- Calendar date-range picker ------------------------------------------

/// Outline calendar glyph (Feather-style); `currentColor` follows the button.
const CALENDAR_ICON: &str = r#"<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="5" width="18" height="16" rx="2"></rect><line x1="3" y1="9" x2="21" y2="9"></line><line x1="8" y1="3" x2="8" y2="6"></line><line x1="16" y1="3" x2="16" y2="6"></line></svg>"#;

fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// Days in month `m` (1-12) of year `y`.
fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap(y) {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

/// Day of week for a date (0 = Sunday), via Sakamoto's algorithm.
fn weekday(y: i32, m: u32, d: u32) -> u32 {
    let t = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let yy = if m < 3 { y - 1 } else { y };
    let w = yy + yy / 4 - yy / 100 + yy / 400 + t[(m - 1) as usize] + d as i32;
    (w.rem_euclid(7)) as u32
}

fn month_name(m: u32) -> &'static str {
    [
        "January", "February", "March", "April", "May", "June", "July", "August", "September",
        "October", "November", "December",
    ]
    .get(m.saturating_sub(1) as usize)
    .copied()
    .unwrap_or("")
}

/// Today as (year, month, day) in the browser's local date.
#[cfg(feature = "hydrate")]
fn today_ymd() -> (i32, u32, u32) {
    let d = js_sys::Date::new_0();
    (
        d.get_full_year() as i32,
        d.get_month() as u32 + 1,
        d.get_date() as u32,
    )
}

/// A 📅 button that opens a monospace month-grid range picker. Click a start day
/// then an end day, Apply to filter the schedule to that range.
#[component]
fn CalendarPicker() -> impl IntoView {
    let range = use_context::<DateRange>().expect("range context").0;
    let earlier = use_context::<EarlierDays>().expect("earlier context").0;
    let open = RwSignal::new(false);
    let ym = RwSignal::new((2026i32, 6u32)); // (year, month 1-12); set to today on mount
    let sel_start = RwSignal::new(None::<String>);
    let sel_end = RwSignal::new(None::<String>);

    Effect::new(move |_| {
        #[cfg(feature = "hydrate")]
        {
            let (y, m, _) = today_ymd();
            ym.set((y, m));
            if let Some((s, e)) = range.get_untracked() {
                sel_start.set(Some(s));
                sel_end.set(Some(e));
            }
        }
    });

    let prev_month = move |_| {
        ym.update(|(y, m)| {
            if *m == 1 {
                *m = 12;
                *y -= 1;
            } else {
                *m -= 1;
            }
        });
    };
    let next_month = move |_| {
        ym.update(|(y, m)| {
            if *m == 12 {
                *m = 1;
                *y += 1;
            } else {
                *m += 1;
            }
        });
    };
    // First click sets the start; second sets the end (ordered); a third restarts.
    let pick = move |iso: String| match (sel_start.get_untracked(), sel_end.get_untracked()) {
        (Some(start), None) => {
            if iso < start {
                sel_start.set(Some(iso));
                sel_end.set(Some(start));
            } else {
                sel_end.set(Some(iso));
            }
        }
        _ => {
            sel_start.set(Some(iso));
            sel_end.set(None);
        }
    };
    let apply = move |_| {
        if let (Some(s), Some(e)) = (sel_start.get_untracked(), sel_end.get_untracked()) {
            earlier.set(0);
            range.set(Some((s.clone(), e.clone())));
            #[cfg(feature = "hydrate")]
            save_range(Some((s, e)));
            open.set(false);
        }
    };
    let clear = move |_| {
        sel_start.set(None);
        sel_end.set(None);
        earlier.set(0);
        range.set(None);
        #[cfg(feature = "hydrate")]
        save_range(None);
        open.set(false);
    };

    let grid = move || {
        let (y, m) = ym.get();
        let start = sel_start.get();
        let end = sel_end.get();
        let lead = weekday(y, m, 1);
        let mut cells: Vec<_> = (0..lead)
            .map(|_| view! { <span class="cal-cell cal-blank"></span> }.into_any())
            .collect();
        for day in 1..=days_in_month(y, m) {
            let iso = format!("{y:04}-{m:02}-{day:02}");
            let in_range = match (&start, &end) {
                (Some(s), Some(e)) => iso.as_str() >= s.as_str() && iso.as_str() <= e.as_str(),
                (Some(s), None) => iso.as_str() == s.as_str(),
                _ => false,
            };
            let mut cls = String::from("cal-cell");
            if in_range {
                cls.push_str(" in-range");
            }
            let on_pick = move |_| pick(iso.clone());
            cells.push(
                view! {
                    <button class=cls on:click=on_pick>
                        {day.to_string()}
                    </button>
                }
                .into_any(),
            );
        }
        cells
    };

    view! {
        <span class="cal-wrap">
            <button
                class="toggle cal-toggle"
                on:click=move |_| open.update(|o| *o = !*o)
                title="Date range"
                aria-label="Date range"
                inner_html=CALENDAR_ICON
            ></button>
            {move || {
                open.get()
                    .then(|| {
                        view! {
                            <div class="calendar">
                                <div class="cal-head">
                                    <button class="cal-nav" on:click=prev_month>
                                        "‹"
                                    </button>
                                    <span class="cal-title">
                                        {move || {
                                            let (y, m) = ym.get();
                                            format!("{} {y}", month_name(m))
                                        }}
                                    </span>
                                    <button class="cal-nav" on:click=next_month>
                                        "›"
                                    </button>
                                </div>
                                <div class="cal-grid">
                                    <span class="cal-dow">"Su"</span>
                                    <span class="cal-dow">"Mo"</span>
                                    <span class="cal-dow">"Tu"</span>
                                    <span class="cal-dow">"We"</span>
                                    <span class="cal-dow">"Th"</span>
                                    <span class="cal-dow">"Fr"</span>
                                    <span class="cal-dow">"Sa"</span>
                                    {grid}
                                </div>
                                <div class="cal-actions">
                                    <button class="linkish" on:click=apply>
                                        "apply"
                                    </button>
                                    <button class="linkish" on:click=clear>
                                        "clear"
                                    </button>
                                </div>
                            </div>
                        }
                    })
            }}
        </span>
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
    // Select a game, or toggle back to "all" if it's already the active filter.
    let pick = move |value: &'static str| {
        let next = if game.get_untracked() == value {
            "all"
        } else {
            value
        };
        set_game.set(next.to_string());
        // Reset the event filter: leagues differ per game.
        set_league.set(String::new());
    };
    // The "All" tab (no game to subscribe to). `.tab-all` lines its trailing
    // items up with the chips row below.
    let plain = move |label: &'static str, value: &'static str| {
        view! {
            <button
                class="tab tab-all"
                class:active=move || game.get() == value
                on:click=move |_| pick(value)
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
                <button class="tab-label" on:click=move |_| pick(value)>
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
                    on:click=move |_| {
                        // Click the active event again to clear back to all events.
                        if selected.get_untracked() == click_name {
                            set_league.set(String::new());
                        } else {
                            set_league.set(click_name.clone());
                        }
                    }
                >
                    {name}
                </button>
            }
        })
        .collect_view();

    view! {
        <div class="chips">
            <button
                class="chip chip-all"
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
        // When a single event is filtered, show its standings/bracket below.
        <EventSection league=league />
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
                    // Header best-of: the single format when the event is uniform,
                    // else the distinct formats joined (e.g. "Bo1 | Bo3"). Per-row
                    // bo still shows for mixed events so you can tell which is which.
                    let bo_label = match &lg.bo {
                        Some(bo) => Some(bo.clone()),
                        None => {
                            let mut bos: Vec<String> = Vec::new();
                            for m in &lg.matches {
                                if !m.best_of.is_empty() && !bos.contains(&m.best_of) {
                                    bos.push(m.best_of.clone());
                                }
                            }
                            bos.sort();
                            (!bos.is_empty()).then(|| bos.join(" | "))
                        }
                    };
                    let header = match &bo_label {
                        Some(bo) => format!("{} · {}", lg.league, bo),
                        None => lg.league.clone(),
                    };
                    let event_url = lg.event_url;
                    let sub_value = lg.league.clone();
                    let rows = lg
                        .matches
                        .into_iter()
                        .map(|m| view! { <MatchRow m=m show_bo=show_bo push=push /> })
                        .collect_view();
                    view! {
                        <div class=format!("league {lc}")>
                            <div class="league-head">
                                <SubscribeStar kind="league" value=sub_value />
                                <h3 class="league-title">
                                    <a href=event_url target="_blank" rel="noreferrer">{header}</a>
                                </h3>
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
    let stale_note = if stale {
        "data may be stale · ".to_string()
    } else {
        String::new()
    };

    view! {
        {nav}
        <div class="status-line">{fixture_note}{stale_note} "loaded " {fetched_label}</div>
        // The "‹ show earlier days" control sits just above the first day (homepage
        // only; the single-day view has its own prev/next nav instead).
        {(!show_nav).then(|| view! { <EarlierControl /> })}
        {day_sections}
        {empty.then(|| view! { <p class="empty">"No tier-1 matches in this window."</p> })}
    }
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

    // Scores are spoilers: reveal only when the global toggle is on or this
    // match was individually revealed (by clicking its "Final" badge).
    let mid = m.id;
    let global = use_context::<ShowScores>().map(|s| s.0);
    let revealed = use_context::<RevealedMatches>().map(|r| r.0);
    let reveal = Memo::new(move |_| {
        global.is_some_and(|g| g.get())
            || revealed.is_some_and(|r| r.with(|set| set.contains(&mid)))
    });

    // A match's score lives behind its status meta: click the whole "Final · Bo3"
    // (or "LIVE") cluster to toggle just this row's score. (Plain meta for
    // score-less rows, e.g. upcoming or a live match still at the 0-0 placeholder.)
    let toggle_reveal = move |ev: leptos::ev::MouseEvent| {
        // Don't let the click fall through to the row's stream link.
        ev.prevent_default();
        ev.stop_propagation();
        if let Some(r) = revealed {
            r.update(|s| {
                if !s.insert(mid) {
                    s.remove(&mid);
                }
            });
        }
    };
    let score_noun = if matches!(m.status, MatchStatus::Live) {
        "live score"
    } else {
        "final score"
    };
    let show_title = format!("Show the {score_noun}");
    let hide_title = format!("Hide the {score_noun}");
    let badge_cls = format!("row-badge {status_class}");
    // Only render the bo when present, so the reveal underline doesn't extend
    // past the badge into an empty cell on uniform events.
    let bo_span = (!bo.is_empty()).then(|| view! { <span class="row-bo">{bo}</span> });
    let meta_view = if has {
        view! {
            <span class="row-meta">
                <span
                    class="reveal-meta"
                    class:on=move || reveal.get()
                    title=move || if reveal.get() { hide_title.clone() } else { show_title.clone() }
                    on:click=toggle_reveal
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
    };

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
        {meta_view}
    };

    // The whole row links to the match detail page; `display: contents` lets its
    // children participate in the row grid alongside the ★ button. The score
    // reveal inside (`reveal-meta`) prevents this navigation when clicked.
    let body = view! {
        <a class="row-body" href=format!("/match/{}", m.id)>
            {inner}
        </a>
    };

    if push {
        // The leading column holds either the reminder ★ (upcoming) or the
        // live/final side bar — never both — so they share one slot and live/
        // final rows don't waste an empty star cell.
        let lead = match m.status {
            MatchStatus::Upcoming => {
                view! { <StarButton id=m.id game=m.game league=m.league.clone() /> }.into_any()
            }
            MatchStatus::Live => view! { <span class="row-bar live"></span> }.into_any(),
            MatchStatus::Finished => view! { <span class="row-bar final"></span> }.into_any(),
            MatchStatus::Canceled => view! { <span class="star star-empty"></span> }.into_any(),
        };
        view! {
            <div class=format!("row has-star {status_class}")>
                {lead}
                {body}
            </div>
        }
        .into_any()
    } else {
        view! { <div class=format!("row {status_class}")>{body}</div> }.into_any()
    }
}

#[component]
fn StarButton(id: i64, game: Game, league: String) -> impl IntoView {
    let starred = use_context::<RwSignal<HashSet<i64>>>().expect("starred context");
    let subscribed = use_context::<Subscribed>().expect("subscribed context").0;
    let vapid = use_context::<RwSignal<Option<String>>>().expect("vapid context");
    // The ★ lights up if this match is starred individually *or* a higher-level
    // subscription (the whole game, or this event) already covers it — so the
    // star reflects everything you'll be notified about.
    let game_key = format!("game|{}", game.slug());
    let league_key = format!("league|{league}");
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
            persist_and_sync(id, !now_on, ids, vapid.get_untracked());
        }
        #[cfg(not(feature = "hydrate"))]
        {
            let _ = (now_on, vapid);
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

/// Persist the starred set and (un)register the reminder on the server. The
/// server derives the notification details from `match_id`, so we send just that.
#[cfg(feature = "hydrate")]
fn persist_and_sync(match_id: i64, starring: bool, ids: Vec<i64>, vapid: Option<String>) {
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
                match_id,
            };
            let _ = crate::server::add_reminder(req).await;
        } else {
            let _ = crate::server::remove_reminder(endpoint, match_id).await;
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
