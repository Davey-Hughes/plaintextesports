use crate::bracket;
use crate::server::{
    get_day, get_event_schedule, get_event_stages, get_event_stages_by_league, get_f1_results,
    get_f1_standings, get_match_detail, get_notifications, get_range, get_schedule, get_site,
    get_team_schedule,
};
use crate::types::{
    full_event_name, BracketMatch, BracketRound, DayGroup, EventInfo, F1Result, F1Standings, Game,
    MatchDetail, MatchStatus, MatchView, ScheduleView, Series, SeriesGame, StandingRow, StreamView,
    SwissMatch, SwissRound,
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

/// The sport-mode cookie key (and `?mode` query param). Unlike the other storage
/// keys it's read on the server too (to render the right mode on the first paint),
/// so it lives outside the hydrate-only `keys` module.
#[cfg(any(feature = "ssr", feature = "hydrate"))]
const SPORT_MODE_KEY: &str = "mode";

/// localStorage / cookie keys, centralized so a preference's read and write can't
/// silently drift onto different keys (and so the storage schema is greppable in
/// one place). Browser storage is client-only, so these are referenced only under
/// `hydrate`. The sport-mode key lives in [`SPORT_MODE_KEY`] (read on both sides).
#[cfg(feature = "hydrate")]
mod keys {
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

/// Whether to show the venue-local time on every game's row at once — clicking
/// any one time toggles it for all of them.
#[derive(Clone, Copy)]
struct ShowVenue(RwSignal<bool>);

/// Set of match ids whose scores are individually revealed (by clicking the
/// match's "Final" badge).
#[derive(Clone, Copy)]
struct RevealedMatches(RwSignal<HashSet<i64>>);

/// Revealed standings/bracket-round sections, keyed by `"st:<tid>"` /
/// `"bk:<tid>:<round>"` so reveal state is per-round and shared by every page
/// that shows the same tournament's bracket.
#[derive(Clone, Copy)]
struct RevealedSections(RwSignal<HashSet<String>>);

/// Set of "kind|value" scopes the user is subscribed to (game/event reminders).
#[derive(Clone, Copy)]
struct Subscribed(RwSignal<HashSet<String>>);

/// Match ids the user has opted out of even though a subscription covers them
/// (a per-match "un-notify" that doesn't drop the whole subscription).
#[derive(Clone, Copy)]
struct Excluded(RwSignal<HashSet<i64>>);

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

/// Selected game filters (slugs) and event filters (league names) — shared and
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

    // Shared user preferences: 24h time (default on) and the detected IANA
    // timezone ("" => server default until the browser reports one).
    let hour24 = RwSignal::new(true);
    let tz = RwSignal::new(String::new());
    // Reminder state: starred match ids, and the VAPID public key (None until
    // fetched / if Web Push isn't configured).
    let starred = RwSignal::new(HashSet::<i64>::new());
    // Matches opted out of a covering subscription (per-match un-notify).
    let excluded = RwSignal::new(HashSet::<i64>::new());
    let vapid = RwSignal::new(None::<String>);
    // Spoiler control: a global reveal + a per-match reveal set.
    let show_scores = RwSignal::new(false);
    // Shared so clicking any one game's time reveals every venue time at once.
    let show_venue = RwSignal::new(false);
    let revealed = RwSignal::new(HashSet::<i64>::new());
    let sections = RwSignal::new(HashSet::<String>::new());
    let subscribed = RwSignal::new(HashSet::<String>::new());
    // History views: a calendar-selected range, and the "earlier days" expansion.
    let range = RwSignal::new(None::<(String, String)>);
    let earlier = RwSignal::new(0i64);
    let later = RwSignal::new(0i64);
    // Schedule filters (shared + persisted across navigation).
    let games = RwSignal::new(HashSet::<String>::new());
    let leagues = RwSignal::new(HashSet::<String>::new());
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
    provide_context(DateRange(range));
    provide_context(EarlierDays(earlier));
    provide_context(LaterDays(later));
    provide_context(Games(games));
    provide_context(Leagues(leagues));
    provide_context(LastUpdated(last_updated));
    provide_context(RefreshTrigger(refresh_trigger));
    provide_context(SportMode(traditional));

    // After hydration, pick up the browser's timezone + saved preferences and
    // the push key. (Client-side only; the initial render uses the defaults above
    // — sport mode excepted, resolved at render — so there's no hydration mismatch.)
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
            excluded.set(load_excluded());
            subscribed.set(load_subs());
            revealed.set(load_revealed());
            sections.set(load_sections());
            range.set(load_range());
            // `games`/`leagues` are initialised by `FilterUrlSync` (URL query, else
            // localStorage) since it needs the router context.
            setup_scrollspy();
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
            <FilterUrlSync />
            <div class="page">
                <SiteHeader />
                <main class="main">
                    <Routes fallback=|| view! { <p class="empty">"Page not found."</p> }>
                        <Route path=StaticSegment("") view=HomePage />
                        <Route path=(StaticSegment("day"), ParamSegment("date")) view=DayPage />
                        <Route path=(StaticSegment("match"), ParamSegment("id")) view=MatchDetailPage />
                        <Route path=(StaticSegment("event"), ParamSegment("league")) view=EventPage />
                        <Route path=(StaticSegment("team"), ParamSegment("name")) view=TeamPage />
                        <Route path=StaticSegment("notifications") view=NotificationsPage />
                        <Route path=StaticSegment("about") view=AboutPage />
                    </Routes>
                </main>
                <SiteFooter />
            </div>
        </Router>
    }
}

/// Update the URL hash to the `.spy` section currently scrolled into view (its
/// `id` becomes the hash) via `replaceState`, so it neither scrolls nor stacks
/// history. Targets are the homepage day groups and the event page's sections.
#[cfg(feature = "hydrate")]
fn scrollspy_update() {
    use wasm_bindgen::JsCast;
    let Some(win) = web_sys::window() else {
        return;
    };
    let Some(doc) = win.document() else {
        return;
    };
    let Ok(nodes) = doc.query_selector_all(".spy[id]") else {
        return;
    };
    // Sections come in document order. The active one spans the line just below
    // the sticky header (matching the jump anchors' scroll-margin); if that line
    // falls in a gap, keep the section above it.
    let trigger = 80.0;
    let mut last_above = String::new();
    let mut contained = String::new();
    for i in 0..nodes.length() {
        let Some(el) = nodes.item(i).and_then(|n| n.dyn_into::<web_sys::Element>().ok()) else {
            continue;
        };
        let r = el.get_bounding_client_rect();
        if r.top() <= trigger {
            last_above = el.id();
            if r.bottom() > trigger {
                contained = last_above.clone();
            }
        }
    }
    let active = if contained.is_empty() { last_above } else { contained };
    let loc = win.location();
    let want = if active.is_empty() { String::new() } else { format!("#{active}") };
    if loc.hash().unwrap_or_default() == want {
        return;
    }
    let url = if want.is_empty() {
        // Strip the hash, keeping the path + query.
        format!("{}{}", loc.pathname().unwrap_or_default(), loc.search().unwrap_or_default())
    } else {
        want
    };
    if let Ok(hist) = win.history() {
        let _ = hist.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&url));
    }
}

/// Install a global, rAF-throttled scroll listener that keeps the URL hash in
/// sync with the visible `.spy` section. Lives for the app's lifetime.
#[cfg(feature = "hydrate")]
fn setup_scrollspy() {
    use std::cell::Cell;
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::JsCast;
    thread_local! {
        static TICKING: Cell<bool> = const { Cell::new(false) };
    }
    let Some(win) = web_sys::window() else {
        return;
    };
    let cb = Closure::<dyn FnMut()>::new(|| {
        if !TICKING.with(Cell::get) {
            TICKING.with(|t| t.set(true));
            request_animation_frame(|| {
                scrollspy_update();
                TICKING.with(|t| t.set(false));
            });
        }
    });
    let _ = win.add_event_listener_with_callback("scroll", cb.as_ref().unchecked_ref());
    cb.forget();
}

/// If the match's schedule row is on the page, scroll to it and flash it; returns
/// whether it was found (so a link can suppress its own navigation). Shared by the
/// bracket team links and the "up next" bar.
#[cfg(feature = "hydrate")]
fn flash_match_row(mid: i64) -> bool {
    let Some(row) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(&format!("m-{mid}")))
    else {
        return false;
    };
    row.scroll_into_view();
    // Remove + reflow + re-add restarts the animation on a repeat click.
    let cl = row.class_list();
    let _ = cl.remove_1("row-flash");
    let _ = row.client_width();
    let _ = cl.add_1("row-flash");
    true
}

/// Keeps the schedule filter (games + events) in sync with the URL query so a
/// filtered view is shareable — `?g=cs2&e=<event>`; no filter ⇒ a clean URL. On
/// load it initialises the filter from the query (falling back to the saved
/// preference). Renders nothing. Must live inside the `Router`.
#[component]
fn FilterUrlSync() -> impl IntoView {
    let games = use_context::<Games>().expect("games context").0;
    let leagues = use_context::<Leagues>().expect("leagues context").0;
    let traditional = use_context::<SportMode>().expect("sport mode context").0;
    let _ = (games, leagues, traditional);
    #[cfg(feature = "hydrate")]
    {
        use leptos_router::hooks::{use_location, use_navigate, use_query_map};
        use leptos_router::NavigateOptions;

        fn parse_set(v: &str) -> HashSet<String> {
            v.split(',').filter(|p| !p.is_empty()).map(str::to_string).collect()
        }

        let query = use_query_map();
        let location = use_location();
        let navigate = use_navigate();

        // Initialise once: a query param wins (a shared link), else the saved
        // preference. (`get_untracked` ⇒ this effect runs a single time.)
        Effect::new(move |_| {
            let q = query.get_untracked();
            let g_param = q.get("g").filter(|v| !v.is_empty());
            match &g_param {
                Some(v) => games.set(parse_set(v)),
                None => games.set(load_str_set(keys::GAMES)),
            }
            match q.get("e").filter(|v| !v.is_empty()) {
                Some(v) => leagues.set(parse_set(&v)),
                None => leagues.set(load_str_set(keys::LEAGUES)),
            }
            // Sport mode is already resolved at render (`initial_sport_mode`); re-
            // apply the URL's view here so an in-app navigation to a ?mode/?g link
            // still lands in the right mode. Same precedence, same value on load.
            if let Some(m) = q.get("mode").filter(|v| !v.is_empty()) {
                traditional.set(m == "sports");
            } else if let Some(v) = &g_param {
                let slugs = parse_set(v);
                if slugs.iter().any(|s| Game::from_filter(s).is_some()) {
                    traditional
                        .set(slugs.iter().any(|s| Game::from_filter(s).is_some_and(Game::traditional)));
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
                    .map(|s| js_sys::encode_uri_component(s).as_string().unwrap_or_default())
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
            let want = if parts.is_empty() { String::new() } else { format!("?{}", parts.join("&")) };
            // Skip a redundant navigation (also breaks any feedback loop).
            if location.search.get_untracked() == want {
                return;
            }
            let url = format!("{path}{want}");
            navigate(&url, NavigateOptions { replace: true, scroll: false, ..Default::default() });
        });
    }
}

#[component]
fn SiteHeader() -> impl IntoView {
    // The brand scrolls away with the page; the toggle bar is a sibling of the
    // page so it can stick to the top while scrolling.
    view! {
        <header class="header">
            <div class="brand">
                <A href="/">"plaintextesports"</A>
            </div>
        </header>
        <div class="toggles">
            // Left: the back-to-top arrow takes the brand's spot once it scrolls
            // away (hidden at the top). Everything else sits in the right cluster,
            // clear of the brand.
            <ScrollTopButton />
            <RefreshButton />
            <SportToggle />
            <CalendarPicker />
            <ScoresToggle />
            <HourToggle />
            <ThemeToggle />
        </div>
    }
}

/// Shows when the server's schedule data was last refreshed, and acts as a button
/// to pull the latest from the server (a client refetch — it never forces the
/// server to re-poll its own data source). Hidden until a schedule has loaded.
#[component]
fn RefreshButton() -> impl IntoView {
    let last = use_context::<LastUpdated>().map(|l| l.0);
    let trigger = use_context::<RefreshTrigger>().map(|t| t.0);
    let bump = move |_| {
        if let Some(t) = trigger {
            t.update(|n| *n += 1);
        }
    };
    move || {
        last.and_then(|l| l.get()).map(|label| {
            let title = format!(
                "Server schedule data from {label} — click to pull the latest from the server"
            );
            view! {
                <button class="refresh-btn" title=title on:click=bump>
                    <span class="refresh-icon">"\u{21bb}"</span>
                    {format!("updated {label}")}
                </button>
            }
        })
    }
}

/// A "back to top" arrow that sits on the left of the sticky toggle bar — taking
/// the place of the brand once it has scrolled away. Hidden at the top of the page.
#[component]
fn ScrollTopButton() -> impl IntoView {
    let scrolled = RwSignal::new(false);
    Effect::new(move |_| {
        #[cfg(feature = "hydrate")]
        {
            use wasm_bindgen::closure::Closure;
            use wasm_bindgen::JsCast;
            let Some(w) = web_sys::window() else {
                return;
            };
            let win = w.clone();
            let update = move || {
                let past = win.scroll_y().unwrap_or(0.0) > 60.0;
                if scrolled.get_untracked() != past {
                    scrolled.set(past);
                }
            };
            update();
            let cb = Closure::<dyn FnMut()>::new(update);
            let _ = w.add_event_listener_with_callback("scroll", cb.as_ref().unchecked_ref());
            // The header mounts once for the app's lifetime, so keep the listener.
            cb.forget();
        }
    });
    let to_top = move |_| {
        #[cfg(feature = "hydrate")]
        if let Some(w) = web_sys::window() {
            w.scroll_to_with_x_and_y(0.0, 0.0);
        }
    };
    view! {
        <button
            class="toggle scrolltop"
            class:scrolltop-off=move || !scrolled.get()
            title="Back to top"
            aria-label="Back to top"
            on:click=to_top
        >
            "↑"
        </button>
    }
}

#[component]
fn SiteFooter() -> impl IntoView {
    let site = Resource::new(|| (), |()| async { get_site().await });
    let traditional = use_context::<SportMode>().expect("sport mode context").0;
    // The notifications page is only useful once push is available (you have
    // something to manage), so the link follows the same gate as the ★s.
    let vapid = use_context::<RwSignal<Option<String>>>().expect("vapid context");
    view! {
        <footer class="footer" id="site-footer">
            // Line 1: navigation + what/where the data is.
            <div class="footer-line">
                <A href="/about">"about"</A>
                {move || {
                    vapid
                        .with(|v| v.is_some())
                        .then(|| {
                            view! {
                                <span class="sep">" · "</span>
                                <A href="/notifications">"notifications"</A>
                            }
                        })
                }}
                <span class="sep">" · "</span>
                <span>
                    {move || {
                        if traditional.get() {
                            "MLB + NHL + NBA + NFL + football + F1 schedules"
                        } else {
                            "tier-1 cs2 + lol schedules"
                        }
                    }}
                </span>
                <span class="sep">" · "</span>
                <span>
                    {move || {
                        if traditional.get() {
                            "data via MLB Stats API, NHL, ESPN & Jolpica"
                        } else {
                            "data via PandaScore"
                        }
                    }}
                </span>
            </div>
            // Line 2: copyright + external links, joined by separators (no
            // leading one, so the centred line reads cleanly).
            <Suspense>
                {move || {
                    site.get()
                        .and_then(Result::ok)
                        .map(|s| {
                            let mut items: Vec<AnyView> = Vec::new();
                            // The copyright links out when a URL is configured.
                            if let Some(c) = s.copyright {
                                let url = s.copyright_url.filter(|u| !u.trim().is_empty());
                                items.push(match url {
                                    Some(u) => {
                                        view! { <a href=u target="_blank" rel="noreferrer">{c}</a> }
                                            .into_any()
                                    }
                                    None => view! { <span>{c}</span> }.into_any(),
                                });
                            }
                            for l in s.links {
                                items.push(
                                    view! {
                                        <a href=l.url target="_blank" rel="noreferrer">{l.label}</a>
                                    }
                                        .into_any(),
                                );
                            }
                            let joined = items
                                .into_iter()
                                .enumerate()
                                .map(|(i, it)| {
                                    view! {
                                        {(i > 0).then(|| view! { <span class="sep">" · "</span> })}
                                        {it}
                                    }
                                })
                                .collect_view();
                            view! { <div class="footer-line">{joined}</div> }
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
                "A fast schedule for "
                <strong>"tier-1 Counter-Strike 2 and League of Legends"</strong>
                " — just the top events. Times are in your timezone; your theme and "
                "12h/24h choices are remembered."
            </p>

            <h2>"Results are hidden by default"</h2>
            <p>"So you can browse finished matches without spoilers. To reveal:"</p>
            <ul>
                <li>
                    <span class="kbd">"show scores"</span>
                    " at the top reveals every score, standing, and bracket at once."
                </li>
                <li>"Click a single match's result to reveal just that one (again to hide)."</li>
                <li>
                    "On a bracket, click " <span class="kbd">"Bracket"</span>
                    " to step through it round by round — names, then scores. Or click a "
                    "round title, or one match, to reveal just that. A later round's teams "
                    "show only once its feeder matches' scores are out, so you can't spoil "
                    "who advanced."
                </li>
            </ul>

            <h2>"Match reminders"</h2>
            <p>
                "Tap a star for a browser notification before a match starts (your browser "
                "asks permission the first time). A filled " <span class="kbd">"★"</span>
                " means you're following it."
            </p>
            <ul>
                <li><span class="kbd">"☆"</span> " on a match — just that match."</li>
                <li>
                    <span class="kbd">"☆"</span>
                    " by a game tab or in an event's header — every match in that game or "
                    "event, including ones added later."
                </li>
            </ul>
            <p class="about-note">"(No stars means reminders aren't enabled on this instance.)"</p>

            <h2>"Finding matches"</h2>
            <p>
                "You start with every tier-1 match. Click game tabs ("
                <span class="kbd">"CS2"</span> " / " <span class="kbd">"LoL"</span>
                ") and event chips to narrow it — the address bar updates so a filtered view "
                "is shareable. Use " <span class="kbd">"‹ show earlier days"</span>
                " or the calendar to look back, and click an event's name for its full "
                "schedule, standings, and bracket."
            </p>

            <p class="about-note">
                "Data is from PandaScore and refreshes in the background; a "
                <span class="kbd">"LIVE"</span>
                " badge is inferred, and scores fill in shortly after a match ends."
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

#[derive(Params, PartialEq, Clone)]
struct EventParams {
    league: String,
}

#[derive(Params, PartialEq, Clone)]
struct TeamParams {
    name: String,
}

/// Percent-encode a league name for use as a URL path segment.
fn enc_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Decode a percent-encoded path segment back to a league name. Idempotent for
/// already-decoded input (league names never contain a literal '%').
fn dec_segment(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A team name rendered as a link to its team page — or plain text when the
/// opponent isn't a real team yet (TBD / empty name). `name` keys the page.
fn team_link(label: String, name: String) -> AnyView {
    if name.is_empty() || name == "TBD" {
        label.into_any()
    } else {
        view! { <A href=format!("/team/{}", enc_segment(&name))>{label}</A> }.into_any()
    }
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
fn EventStageCombo(
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
            let before = anchor.as_ref().map(|el| el.get_bounding_client_rect().top());
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
    let EventInfo { tournament_id, stage, game, standings, rounds, swiss, .. } = event;
    let bracket_only = standings.is_empty();
    let has_swiss = !swiss.is_empty();
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
                <SwissBracket rounds=swiss tournament_id />
            </div>
        }
    });
    let list_view = view! {
        <div class="swiss-view" class:swiss-view-off=move || has_swiss && swiss_grid.get()>
            <StandingsTable rows=standings tournament_id game />
        </div>
    };
    view! {
        <div class="event-extra spy" id=format!("stage-{tournament_id}")>
            <div class="stage-bar">{label}</div>
            // The grid/list toggle sits at the top-right of the views, on the
            // "Bracket"/"Standings" title row (just above its underline).
            <div class="swiss-views">
                {tabs}
                {grid_view}
                {list_view}
            </div>
            <Bracket rounds=rounds tournament_id bracket_only times=times.get_value() />
        </div>
    }
}

#[component]
fn EventStages(
    stages: Vec<EventInfo>,
    /// `match_id -> (day_key, clock_label)` so brackets can date rounds and show
    /// per-match times; empty where the schedule isn't on the page.
    #[prop(optional)]
    times: std::collections::HashMap<i64, (String, String)>,
) -> impl IntoView {
    let times = StoredValue::new(times);
    // One toggle drives every stage.
    let swiss_grid = RwSignal::new(true);
    stages
        .into_iter()
        .map(move |e| view! { <EventStageCombo event=e swiss_grid times=times.get_value() /> })
        .collect_view()
}

/// A Grand Prix's results — each finished session's full finishing order, each
/// spoiler-gated (hidden until revealed, like the rest of the site's scores).
/// Stable in-page anchor for an F1 session/section (e.g. "Practice 1" →
/// "f1sec-practice-1"), shared by the section ids and the jump-to nav.
fn f1_anchor(name: &str) -> String {
    format!("f1sec-{}", name.to_lowercase().replace(' ', "-"))
}

#[component]
fn F1Results(results: Vec<F1Result>, season: i64, round: i64) -> impl IntoView {
    let sections = results
        .into_iter()
        .map(|r| {
            let session = r.session;
            let anchor = f1_anchor(&session);
            let (revealed, toggle) =
                section_reveal(format!("f1res:{season}:{round}:{session}"));
            let count = r.rows.len();
            let rows = StoredValue::new(r.rows);
            // Always render every row so revealing doesn't shift the page; the
            // position column (just a row number) stays, and the driver /
            // constructor / time blank out until revealed.
            let order = move || {
                let show = revealed.get();
                rows.get_value()
                    .into_iter()
                    .map(|row| {
                        let (driver, con, detail) = if show {
                            (row.driver, row.constructor, row.detail)
                        } else {
                            (String::new(), String::new(), String::new())
                        };
                        view! {
                            <li class="f1-row">
                                <span class="f1-pos">{row.pos}</span>
                                <span class="f1-driver">{driver}</span>
                                <span class="f1-con">{con}</span>
                                <span class="f1-detail">{detail}</span>
                            </li>
                        }
                    })
                    .collect_view()
            };
            view! {
                <div class="f1-session" id=anchor>
                    <button class="f1-session-head" on:click=toggle>
                        <span class="f1-session-name">{session}</span>
                        <span class="f1-session-toggle">
                            {move || {
                                if revealed.get() {
                                    "hide".to_string()
                                } else {
                                    format!("reveal results ({count})")
                                }
                            }}
                        </span>
                    </button>
                    <ol class="f1-order">{order}</ol>
                </div>
            }
        })
        .collect_view();
    view! {
        <section class="detail-section">
            <h2 class="section-title f1-results-title">"Results"</h2>
            {sections}
        </section>
    }
}

/// The drivers' and constructors' championship standings as of a GP's round,
/// shown on the F1 event page. Spoiler-gated as one block (it encodes results).
#[component]
fn F1StandingsView(standings: F1Standings, season: i64, round: i64) -> impl IntoView {
    let (revealed, toggle) = section_reveal(format!("f1stand:{season}:{round}"));
    let asof = standings.round;
    let drivers = StoredValue::new(standings.drivers);
    let constructors = StoredValue::new(standings.constructors);
    // Always render both tables (blanked until revealed) so revealing doesn't
    // shift the page — the position column stays, names/points blank out.
    let body = move || {
        let show = revealed.get();
        let drv = drivers
            .get_value()
            .into_iter()
            .map(|r| {
                let (name, con, pts) = if show {
                    (r.name, r.detail, r.points)
                } else {
                    (String::new(), String::new(), String::new())
                };
                view! {
                    <li class="f1-standing-row">
                        <span class="f1-pos">{r.pos}</span>
                        <span class="f1-standing-name">{name}</span>
                        <span class="f1-standing-con">{con}</span>
                        <span class="f1-standing-pts">{pts}</span>
                    </li>
                }
            })
            .collect_view();
        let con = constructors
            .get_value()
            .into_iter()
            .map(|r| {
                let (name, pts) = if show {
                    (r.name, r.points)
                } else {
                    (String::new(), String::new())
                };
                view! {
                    <li class="f1-standing-row f1-standing-row-con">
                        <span class="f1-pos">{r.pos}</span>
                        <span class="f1-standing-name">{name}</span>
                        <span class="f1-standing-pts">{pts}</span>
                    </li>
                }
            })
            .collect_view();
        view! {
            <div class="f1-standings-tables">
                <div class="f1-standings-table">
                    <h3 class="f1-standings-sub">"Drivers"</h3>
                    <ol class="f1-standings">{drv}</ol>
                </div>
                <div class="f1-standings-table">
                    <h3 class="f1-standings-sub">"Constructors"</h3>
                    <ol class="f1-standings f1-standings-con">{con}</ol>
                </div>
            </div>
        }
    };
    view! {
        <section class="detail-section" id="f1sec-standings">
            <h2 class="section-title f1-results-title">
                "Standings"
                <span class="f1-standings-asof">{format!(" · after Round {asof}")}</span>
            </h2>
            <button class="f1-session-head" on:click=toggle>
                <span class="f1-session-toggle">
                    {move || {
                        if revealed.get() { "hide".to_string() } else { "reveal standings".to_string() }
                    }}
                </span>
            </button>
            {body}
        </section>
    }
}

/// Internal event page: the event's standings/bracket, its full schedule, and a
/// link out to the event's Liquipedia/official page.
#[component]
fn EventPage() -> impl IntoView {
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
        move || schedule.get().and_then(Result::ok).and_then(|s| f1_season_round(&s)),
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
        move || schedule.get().and_then(Result::ok).and_then(|s| f1_season_round(&s)),
        |sr| async move {
            match sr {
                Some((season, round)) => {
                    get_f1_standings(season, round).await.unwrap_or_default()
                }
                None => F1Standings::default(),
            }
        },
    );
    setup_autorefresh(schedule);

    // Keep the global sport mode in sync with the event being viewed, so the
    // header toggle, footer, and the windowing controls agree with the data
    // (you reach an MLB event from MLB mode, but a direct link should match too).
    // Client-only (Effects don't run on SSR), and it never persists the choice.
    Effect::new(move |_| {
        if let Some(Ok(s)) = schedule.get() {
            sport_mode.set(schedule_is_traditional(&s));
        }
    });

    // Render the whole page from inside one Suspense (awaiting both resources),
    // mirroring the match-detail page — so nothing reactive sits at the
    // synchronous top level to throw off hydration.
    view! {
        <Suspense fallback=|| {
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
                        // The route segment is already the full edition name.
                        let title = lg_name.clone();
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
                        // A dotted rule between the schedule and the brackets below.
                        let sep = (!stage_list.is_empty())
                            .then(|| view! { <hr class="section-sep" /> });
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
                        // Read push here (not at the top) so the ★s appear once the
                        // VAPID key resolves after hydration — this closure re-runs.
                        let push = use_context::<RwSignal<Option<String>>>()
                            .is_some_and(|v| v.get().is_some());
                        // (season, round) for an F1 GP — keys its results' reveal.
                        let f1_sr = f1_season_round(&s);
                        // An F1 Grand Prix gets a ★ to subscribe to the whole
                        // event (all its sessions). Other event pages keep the
                        // bare title — their schedule already carries league ★s.
                        let title_head = if f1_sr.is_some() {
                            view! {
                                <div class="event-title-head">
                                    <SubscribeStar kind="event" value=title.clone() />
                                    <h1 class="detail-title">{title.clone()}</h1>
                                </div>
                            }
                            .into_any()
                        } else {
                            view! { <h1 class="detail-title">{title.clone()}</h1> }.into_any()
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
                        view! {
                            <article class="detail">
                                <A href="/">"← schedule"</A>
                                {title_head}
                                {link}
                                {nav}
                                {f1_nav}
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
        </Suspense>
    }
}

/// A single team's schedule (past + upcoming), reachable from the match page.
/// Traditional-sport teams window like the homepage/event page (with the
/// earlier/later expanders); esports teams show their full tier-1 history.
#[component]
fn TeamPage() -> impl IntoView {
    let params = use_params::<TeamParams>();
    let team = move || {
        params
            .get()
            .ok()
            .map(|p| dec_segment(&p.name))
            .unwrap_or_default()
    };
    let hour24 = use_context::<RwSignal<bool>>().expect("hour24 context");
    let tz = use_context::<RwSignal<String>>().expect("tz context");
    let earlier = use_context::<EarlierDays>().expect("earlier context").0;
    let later = use_context::<LaterDays>().expect("later context").0;
    let sport_mode = use_context::<SportMode>().expect("sport mode context").0;
    let schedule = Resource::new(
        move || (team(), tz.get(), hour24.get()),
        |(t, z, h)| async move { get_team_schedule(t, z, h).await },
    );
    setup_autorefresh(schedule);

    // Keep the global sport mode in sync with the team being viewed, so the
    // header toggle/footer/windowing agree (client-only; never persisted).
    Effect::new(move |_| {
        if let Some(Ok(s)) = schedule.get() {
            sport_mode.set(schedule_is_traditional(&s));
        }
    });

    view! {
        <Suspense fallback=|| {
            view! {
                <article class="detail">
                    <A href="/">"← schedule"</A>
                    <p class="loading">"loading…"</p>
                </article>
            }
        }>
            {move || {
                let name = team();
                match schedule.get() {
                    Some(Ok(mut s)) => {
                        // Traditional-sport teams cap their forward horizon and show
                        // the earlier/later expanders, like the event page.
                        let windowed = schedule_needs_window(&s);
                        if windowed {
                            let (lo, hi) =
                                trad_day_bounds(&s.today_key, earlier.get(), later.get());
                            s.days.retain(|d| {
                                d.day_key.as_str() >= lo.as_str()
                                    && d.day_key.as_str() <= hi.as_str()
                            });
                        }
                        // Read push here so the ★s appear once the VAPID key
                        // resolves after hydration (this closure re-runs then).
                        let push = use_context::<RwSignal<Option<String>>>()
                            .is_some_and(|v| v.get().is_some());
                        view! {
                            <article class="detail">
                                <A href="/">"← schedule"</A>
                                <div class="team-head">
                                    <h1 class="detail-title">{name.clone()}</h1>
                                    <SubscribeStar kind="team" value=name.clone() />
                                </div>
                                <div id="sched" class="spy">
                                    {render_schedule(s, false, push, true, windowed)}
                                </div>
                            </article>
                        }
                            .into_any()
                    }
                    _ => {
                        view! {
                            <article class="detail">
                                <A href="/">"← schedule"</A>
                                <p class="error">"Failed to load team."</p>
                            </article>
                        }
                            .into_any()
                    }
                }
            }}
        </Suspense>
    }
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
        stages,
        series,
        ..
    } = d;
    let m = match_view.expect("found implies match_view");
    let status_label = match m.status {
        MatchStatus::Live => "LIVE",
        MatchStatus::Finished => "Final",
        MatchStatus::Canceled => "Canceled",
        MatchStatus::Upcoming => "",
    };
    // The meta line is split so the date+time can swap to the venue-local time on
    // click (blue), like the schedule rows. Lead = event name; the middle is the
    // clickable when; trail = best-of + status.
    let event_name = full_event_name(&m.league, &m.serie_name);
    let when_local = [m.date_label.clone(), m.clock_label.clone()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    let venue_when = m.venue_label.clone();
    let trail = [m.best_of.clone(), status_label.to_string()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");

    // Shared venue-time toggle (same one the schedule rows use), so the date+time
    // swaps to the venue's local date+time when clicked.
    let show_venue = use_context::<ShowVenue>().map(|s| s.0);
    let has_venue = !venue_when.is_empty();
    let venue_shown = move || show_venue.is_some_and(|sv| sv.get());
    let toggle_venue = move |ev: leptos::ev::MouseEvent| {
        ev.prevent_default();
        ev.stop_propagation();
        if let Some(sv) = show_venue {
            sv.update(|v| *v = !*v);
        }
    };
    let venue_title = move || {
        if venue_shown() {
            "Click to hide the local time at the venue"
        } else {
            "Click to show the local time at the venue"
        }
    };

    let mid = m.id;
    let (sa, sb) = (m.team_a.score, m.team_b.score);
    let has_score = sa.is_some() && sb.is_some();
    // "Played" = under way or done (an upcoming match can still carry a 0-0
    // placeholder score, so a score alone isn't enough).
    let played = matches!(m.status, MatchStatus::Live | MatchStatus::Finished) && has_score;
    let (win_a, win_b) = (m.team_a.winner, m.team_b.winner);
    let sep = versus_sep(m.game);
    // F1 (and any single-entity sport) has no opponent: the title is the one
    // session label, with no "vs"/"at" separator and no empty second team.
    let is_solo = m.game.single_entity();
    let (team_a, team_b) = (m.team_a.label, m.team_b.label);
    let (name_a, name_b) = (m.team_a.name, m.team_b.name);

    // Scores/standings/bracket are spoilers: reveal when the global toggle is on
    // or this match was individually revealed (persisted, shared with the list).
    let global = use_context::<ShowScores>().map(|s| s.0);
    let revealed = use_context::<RevealedMatches>().map(|r| r.0);
    let reveal = Memo::new(move |_| {
        global.is_some_and(|g| g.get())
            || revealed.is_some_and(|r| r.with(|set| set.contains(&mid)))
    });
    // Per-match reveal button (hidden when the global toggle already shows all).
    let toggle = reveal_toggler(revealed, mid);
    // Hide the reveal toggle when the global toggle already shows all, or when
    // the match hasn't been played yet (nothing to reveal).
    let toggle_hidden = move || global.is_some_and(|g| g.get()) || !played;

    view! {
        <article class="detail">
            <A href="/">"← schedule"</A>
            <h1 class="detail-title" class:detail-title-solo=is_solo>
                {if is_solo {
                    view! { <span class="detail-team detail-solo">{team_a}</span> }.into_any()
                } else {
                    view! {
                        <span
                            class="detail-team"
                            class:winner=move || reveal.get() && win_a
                            class:loser=move || reveal.get() && win_b
                        >
                            {team_link(team_a, name_a)}
                        </span>
                        <span class="detail-score">
                            {move || {
                                if reveal.get() && has_score {
                                    format!("{} – {}", sa.unwrap_or(0), sb.unwrap_or(0))
                                } else {
                                    sep.to_string()
                                }
                            }}
                        </span>
                        <span
                            class="detail-team"
                            class:winner=move || reveal.get() && win_b
                            class:loser=move || reveal.get() && win_a
                        >
                            {team_link(team_b, name_b)}
                        </span>
                    }
                    .into_any()
                }}
            </h1>
            <div class="detail-meta">
                <span class="detail-meta-line">
                    <span>{event_name}" · "</span>
                    {if has_venue {
                        let local = when_local.clone();
                        let venue = venue_when.clone();
                        view! {
                            <span
                                class="detail-when row-time-tz"
                                class:on=venue_shown
                                title=venue_title
                                on:click=toggle_venue
                            >
                                {move || if venue_shown() { venue.clone() } else { local.clone() }}
                            </span>
                        }
                        .into_any()
                    } else {
                        view! { <span>{when_local.clone()}</span> }.into_any()
                    }}
                    {(!trail.is_empty()).then(|| format!(" · {trail}"))}
                </span>
                {move || {
                    (!toggle_hidden())
                        .then(|| {
                            view! {
                                <button class="toggle" on:click=toggle>
                                    {move || if reveal.get() { "hide scores" } else { "show scores" }}
                                </button>
                            }
                        })
                }}
            </div>
            <StreamsList streams=streams />
            // MLB series between the two teams. Each game row reveals on its own
            // (shared with the schedule); the record line waits until every played
            // game is revealed, so it never leaks the leader ahead of the scores.
            {(!series.games.is_empty())
                .then(|| {
                    let played_ids: Vec<i64> = series
                        .games
                        .iter()
                        .filter(|g| {
                            matches!(g.status, MatchStatus::Live | MatchStatus::Finished)
                                && g.score_a.is_some()
                                && g.score_b.is_some()
                        })
                        .map(|g| g.match_id)
                        .collect();
                    let global = use_context::<ShowScores>().map(|s| s.0);
                    let revealed = use_context::<RevealedMatches>().map(|r| r.0);
                    let record_reveal = Memo::new(move |_| {
                        global.is_some_and(|g| g.get())
                            || (!played_ids.is_empty()
                                && revealed.is_some_and(|r| {
                                    r.with(|set| played_ids.iter().all(|id| set.contains(id)))
                                }))
                    });
                    view! { <SeriesSection series=series record_reveal=record_reveal /> }
                })}
            // The event's stage combo (Swiss grid/list + playoff bracket), same as
            // the event page; each section self-gates its own reveal (shared,
            // persisted). Empty sections render nothing.
            {(!event.is_empty())
                .then(|| view! { <EventStageCombo event=event swiss_grid=RwSignal::new(true) /> })}
            // MLB has no per-event standings; show the two teams' division tables.
            {(!stages.is_empty()).then(|| view! { <EventStages stages=stages /> })}
        </article>
    }
}

/// Split a stream URL into a friendly (site, channel), e.g.
/// "https://www.twitch.tv/esl_csgo" → ("Twitch", "esl_csgo").
fn stream_parts(url: &str) -> (String, String) {
    let after = url
        .split_once("//")
        .map_or(url, |(_, rest)| rest)
        .trim_end_matches('/');
    let (domain, path) = after.split_once('/').unwrap_or((after, ""));
    let domain = domain.trim_start_matches("www.");
    let site = if domain.contains("twitch") {
        "Twitch"
    } else if domain.contains("youtube") || domain.contains("youtu.be") {
        "YouTube"
    } else if domain.contains("kick") {
        "Kick"
    } else if domain.contains("afreeca") || domain.contains("sooplive") || domain.contains("soop") {
        "SOOP"
    } else if domain.contains("bilibili") {
        "Bilibili"
    } else {
        domain
    };
    // The channel/handle is the path (drop any query string).
    let channel = path.split('?').next().unwrap_or("").trim_matches('/');
    (site.to_string(), channel.to_string())
}

/// Language + main/official tags for a stream, e.g. "EN · main".
fn stream_tags(s: &StreamView) -> String {
    let mut tags = Vec::new();
    if !s.language.is_empty() {
        tags.push(s.language.to_uppercase());
    }
    if s.main {
        tags.push("main".to_string());
    } else if s.official {
        tags.push("official".to_string());
    }
    tags.join(" · ")
}

#[component]
fn StreamsList(streams: Vec<StreamView>) -> impl IntoView {
    if streams.is_empty() {
        return ().into_any();
    }
    // MLB broadcasts carry a network `name` (and sometimes a watch link); esports
    // streams carry a clickable `url`. Title the section to match what it lists.
    let broadcast = streams.iter().any(|s| !s.name.is_empty());
    let title = if broadcast { "Broadcasts" } else { "Streams" };
    // A small gap leads each new group (after the first), separating official
    // streams from co-streams, and TV / streaming / radio for broadcasts.
    let gaps: Vec<bool> = streams
        .iter()
        .enumerate()
        .map(|(i, s)| i > 0 && streams[i - 1].group != s.group)
        .collect();
    let items = streams
        .into_iter()
        .zip(gaps)
        .map(|(s, gap)| {
            // A broadcast's label is its network name; an esports stream's is
            // derived from its url (site + channel).
            let (label, channel) = if s.name.is_empty() {
                stream_parts(&s.url)
            } else {
                (s.name.clone(), String::new())
            };
            let tags = if s.name.is_empty() { stream_tags(&s) } else { s.tag.clone() };
            let mut cls = String::from("stream");
            // Highlight an esports official broadcast (not the per-game MLB flag).
            if s.official && s.name.is_empty() {
                cls.push_str(" official");
            }
            if gap {
                cls.push_str(" stream-group-start");
            }
            let label_view = view! {
                <span class="stream-site">{label}</span>
                {(!channel.is_empty())
                    .then(|| view! { <span class="stream-chan">{format!("/{channel}")}</span> })}
            };
            let body = if s.url.is_empty() {
                label_view.into_any()
            } else {
                view! { <a href=s.url target="_blank" rel="noreferrer">{label_view}</a> }.into_any()
            };
            view! {
                <li class=cls>
                    {body}
                    {(!tags.is_empty()).then(|| view! { <span class="stream-tags">{tags}</span> })}
                </li>
            }
        })
        .collect_view();
    view! {
        <section class="detail-section">
            <h2 class="section-title">{title}</h2>
            <ul class="streams">{items}</ul>
        </section>
    }
    .into_any()
}

/// The MLB "Series" section: each game of the series between the two teams as a
/// schedule-style row — date, status badge ("Final"/"LIVE"), the matchup, and a
/// score you reveal by clicking the badge (no need to open the game first). Each
/// row gates on its OWN game's reveal (the same per-match set the schedule uses),
/// so revealing here reveals it on the homepage too, and vice versa. The series
/// record only shows once every played game in the series is revealed, so it
/// never leaks the leader ahead of the scores it's derived from.
#[component]
fn SeriesSection(series: Series, record_reveal: Memo<bool>) -> impl IntoView {
    let Series { games, game_label, record_label } = series;
    // Whether any game can swap to a ballpark-local time (so the "venue time"
    // hint by the title only appears when there's actually a venue toggle here,
    // not just because some other page flipped the shared ShowVenue).
    let has_venue = games.iter().any(|g| !g.venue_label.is_empty());
    let rows = games
        .into_iter()
        .map(|g| view! { <SeriesRow game=g /> })
        .collect_view();
    // The record line is a spoiler (it names the leader). Show it (when present)
    // only once every played game in the series is revealed — `record_reveal`,
    // computed by the parent from the shared per-game reveal set. The line always
    // occupies its height (a blank placeholder when hidden) so revealing it never
    // shifts the page layout.
    let record = (!record_label.is_empty()).then(|| {
        view! {
            <p class="series-record" class:series-hidden=move || !record_reveal.get()>
                {move || if record_reveal.get() { record_label.clone() } else { "\u{00a0}".to_string() }}
            </p>
        }
    });
    let game_label = (!game_label.is_empty())
        .then(|| view! { <span class="series-meta">{game_label}</span> });
    // When the ballpark time is being shown (and this series has a venue toggle),
    // a small blue "venue time" hint by the title makes the swapped state obvious.
    let venue_hint = has_venue.then(|| {
        let show_venue = use_context::<ShowVenue>().map(|s| s.0);
        let shown = move || show_venue.is_some_and(|sv| sv.get());
        view! {
            {move || shown().then(|| view! { <span class="series-venue-hint">"venue time"</span> })}
        }
    });
    view! {
        <section class="detail-section">
            <h2 class="section-title">"Series" {game_label} {venue_hint}</h2>
            <div class="series-games">{rows}</div>
            {record}
        </section>
    }
}

/// One schedule-style row in the Series section. Mirrors `MatchRow`'s per-game
/// score reveal: the status badge is clickable to toggle just this game's score,
/// keyed on its own `match_id` in the shared `RevealedMatches` set (so it stays
/// in sync with the homepage). The whole row links to that game's detail page.
#[component]
fn SeriesRow(game: SeriesGame) -> impl IntoView {
    let SeriesGame {
        day_label,
        clock_label,
        venue_label,
        team_a,
        team_b,
        score_a,
        score_b,
        winner,
        status,
        current,
        match_id,
    } = game;
    let status_class = status.row_class();
    let badge = status.badge();
    let has = score_a.is_some() && score_b.is_some();
    let played = matches!(status, MatchStatus::Live | MatchStatus::Finished) && has;
    let (win_a, win_b) = (winner == "a", winner == "b");

    // This game's own reveal (global toggle OR this game individually revealed),
    // shared with the schedule via the per-match set keyed on its gamePk.
    let global = use_context::<ShowScores>().map(|s| s.0);
    let revealed = use_context::<RevealedMatches>().map(|r| r.0);
    let reveal = Memo::new(move |_| {
        global.is_some_and(|g| g.get())
            || revealed.is_some_and(|r| r.with(|set| set.contains(&match_id)))
    });
    let toggle_reveal = reveal_toggler(revealed, match_id);
    let score_noun = if matches!(status, MatchStatus::Live) { "live score" } else { "final score" };
    let show_title = format!("Show the {score_noun}");
    let hide_title = format!("Hide the {score_noun}");
    let badge_cls = format!("row-badge {status_class}");
    // A played game's badge is the reveal control; an unplayed game's is static.
    let meta_view = if played {
        view! {
            <span class="row-meta">
                <span
                    class="reveal-meta"
                    class:on=move || reveal.get()
                    title=move || if reveal.get() { hide_title.clone() } else { show_title.clone() }
                    on:click=toggle_reveal
                >
                    <span class=badge_cls>{badge}</span>
                </span>
            </span>
        }
        .into_any()
    } else {
        view! { <span class="row-meta"><span class=badge_cls>{badge}</span></span> }.into_any()
    };

    // Each row leads with its date and start time on one line, both in the
    // viewer's tz so they always agree. When the venue tz is known it's clickable
    // to *swap* the whole label to the ballpark's local date+time (rather than
    // appending it) — so the row width and the team highlighting stay put. The
    // toggle is the shared `ShowVenue` one the schedule uses.
    let when_text = if clock_label.is_empty() {
        day_label.clone()
    } else {
        format!("{day_label} · {clock_label}")
    };
    let when_view = venue_time_cell(
        "row-time series-when",
        when_text,
        venue_label,
        "Click to show the local time at the ballpark",
        "Showing the time at the ballpark — click for your local time",
    );
    let inner = view! {
        {when_view}
        <span
            class="row-team row-a"
            class:winner=move || reveal.get() && win_a
            class:loser=move || reveal.get() && win_b
        >
            {team_a}
        </span>
        <span class="row-mid" class:scored=move || reveal.get() && has>
            {move || {
                if reveal.get() && has {
                    format!("{} \u{2013} {}", score_a.unwrap_or(0), score_b.unwrap_or(0))
                } else {
                    "at".to_string()
                }
            }}
        </span>
        <span
            class="row-team row-b"
            class:winner=move || reveal.get() && win_b
            class:loser=move || reveal.get() && win_a
        >
            {team_b}
        </span>
        {meta_view}
    };

    // The row links to this game's detail page; the current game (this page)
    // isn't a link — it's marked instead. `display: contents` lets the body's
    // children sit in the row grid.
    let body = if current {
        view! { <span class="row-body">{inner}</span> }.into_any()
    } else {
        view! { <a class="row-body" href=format!("/match/{match_id}")>{inner}</a> }.into_any()
    };
    let mut row_cls = format!("row {status_class}");
    if current {
        row_cls.push_str(" series-current");
    }
    let lead = match status {
        MatchStatus::Live => view! { <span class="row-bar live"></span> }.into_any(),
        MatchStatus::Finished => view! { <span class="row-bar final"></span> }.into_any(),
        _ => view! { <span class="row-lead-empty"></span> }.into_any(),
    };
    view! {
        <div class=format!("{row_cls} has-star")>
            {lead}
            {body}
        </div>
    }
}

/// Reveal state for a persisted section key (standings / one bracket round),
/// OR'd with the global "show scores". Returns `(revealed, toggle, toggle-hidden)`
/// where the toggle is shared across every page showing this same section.
fn section_reveal(key: String) -> (Memo<bool>, impl Fn(leptos::ev::MouseEvent) + Copy) {
    let global = use_context::<ShowScores>().map(|s| s.0);
    let sections = use_context::<RevealedSections>().map(|s| s.0);
    let key = StoredValue::new(key);
    let revealed = Memo::new(move |_| {
        global.is_some_and(|g| g.get())
            || sections.is_some_and(|s| s.with(|set| set.contains(&key.get_value())))
    });
    let toggle = move |_: leptos::ev::MouseEvent| {
        if let Some(s) = sections {
            let k = key.get_value();
            s.update(|set| {
                if !set.remove(&k) {
                    set.insert(k);
                }
            });
            #[cfg(feature = "hydrate")]
            save_sections(&s.get_untracked());
        }
    };
    (revealed, toggle)
}

/// Reveal stage of a bracket series from its precomputed name/score keys:
/// 0 hidden, 1 team names, 2 scores. (`bs` set ⇒ scores; `bn` ⇒ names.)
fn stage_of(set: &HashSet<String>, bn: &str, bs: &str) -> u8 {
    if set.contains(bs) {
        2
    } else if set.contains(bn) {
        1
    } else {
        0
    }
}

/// Move one series to a reveal stage (0 hidden / 1 names / 2 scores).
fn apply_stage(set: &mut HashSet<String>, bn: &str, bs: &str, stage: u8) {
    match stage {
        0 => {
            set.remove(bn);
            set.remove(bs);
        }
        1 => {
            set.insert(bn.to_string());
            set.remove(bs);
        }
        _ => {
            set.insert(bn.to_string());
            set.insert(bs.to_string());
        }
    }
}

/// Whether a bracket match has actually been played: it has both scores and
/// either a decided winner or a non-zero score. A 0–0 with no winner is an
/// unplayed/upcoming match, not a real result, so it never gets a score stage.
fn bm_played(m: &BracketMatch) -> bool {
    m.score_a.is_some()
        && m.score_b.is_some()
        && (!m.winner.is_empty() || m.score_a != Some(0) || m.score_b != Some(0))
}

/// The furthest a series can be revealed: 0 if its teams are undecided (TBD —
/// stays hidden so we don't expose an unannounced match), 2 if it's been played
/// (has a score to show), else 1 (decided teams, not yet played → names only).
fn bm_max_stage(m: &BracketMatch) -> u8 {
    let tbd = |t: &str| t.is_empty() || t == "TBD";
    if tbd(&m.team_a) || tbd(&m.team_b) {
        0
    } else if bm_played(m) {
        2
    } else {
        1
    }
}

/// Swiss-match equivalents of [`bm_played`] / [`bm_max_stage`], so a Swiss stage
/// reveals through the same staged feeder-gated machinery as the playoff.
fn sw_played(m: &SwissMatch) -> bool {
    !m.winner.is_empty()
        || (m.score_a.is_some()
            && m.score_b.is_some()
            && (m.score_a != Some(0) || m.score_b != Some(0)))
}

/// A finishing record like "3-0"/"0-3": advanced if it has more wins than
/// losses, eliminated otherwise.
fn record_is_pass(rec: &str) -> bool {
    rec.split_once('-')
        .and_then(|(w, l)| Some(w.trim().parse::<i32>().ok()? > l.trim().parse::<i32>().ok()?))
        .unwrap_or(false)
}

fn sw_max_stage(m: &SwissMatch) -> u8 {
    let tbd = |t: &str| t.is_empty() || t == "TBD";
    if tbd(&m.team_a) || tbd(&m.team_b) {
        0
    } else if sw_played(m) {
        2
    } else {
        1
    }
}

/// Per-series reveal metadata used to compute effective stages and gate clicks.
#[derive(Clone)]
struct BkCell {
    bn: String,
    bs: String,
    /// Furthest this series can be revealed (see [`bm_max_stage`]).
    max: u8,
    /// Minimum stage always shown (1 = team names) — used so a bracket-only
    /// event's first-round matchups stay visible (only scores are a spoiler).
    floor: u8,
    /// `(round, index)` of the matches whose scores must be shown before this
    /// series' lineup is known (empty for a first-round match).
    feeders: Vec<(usize, usize)>,
}

/// A series' lineup is auto-revealed (stage 1) once all its feeders' scores are
/// shown — works for single- and double-elimination. First-round matches
/// (no feeders) never auto-reveal; they're revealed manually.
fn auto_stage(eff: &[Vec<u8>], feeders: &[(usize, usize)]) -> u8 {
    if feeders.is_empty() {
        return 0;
    }
    let all_scored = feeders
        .iter()
        .all(|&(r, i)| eff.get(r).and_then(|row| row.get(i)).copied().unwrap_or(0) >= 2);
    u8::from(all_scored)
}

/// Effective reveal stage per series: the manual reveal (from the shared set)
/// unioned with the auto-revealed lineup, capped at each series' max. Computed
/// round by round so feeders (always in earlier rounds) are ready. `global`
/// reveals everything (still capped, so TBD matches stay hidden).
fn compute_effective(grid: &[Vec<BkCell>], set: &HashSet<String>, global: bool) -> Vec<Vec<u8>> {
    let mut eff: Vec<Vec<u8>> = Vec::with_capacity(grid.len());
    for row in grid {
        let stages: Vec<u8> = row
            .iter()
            .map(|c| {
                if global {
                    return c.max;
                }
                let raw = stage_of(set, &c.bn, &c.bs)
                    .max(auto_stage(&eff, &c.feeders))
                    .max(c.floor)
                    .min(c.max);
                // Cascade hiding: a series can't show its score until its own
                // teams are known (every feeder's score is shown). So hiding an
                // early round pulls its lineup, which pulls every later round it
                // feeds — not just the immediately next one.
                let teams_known = c.feeders.iter().all(|&(r, i)| {
                    eff.get(r).and_then(|row| row.get(i)).copied().unwrap_or(0) >= 2
                });
                if teams_known {
                    raw
                } else {
                    raw.min(1)
                }
            })
            .collect();
        eff.push(stages);
    }
    eff
}

/// Whether a series can still be advanced: decided (max > 0), not already maxed,
/// and — for later rounds — its feeders' scores revealed (lineup known).
fn bk_advanceable(grid: &[Vec<BkCell>], eff: &[Vec<u8>], r: usize, i: usize) -> bool {
    let c = &grid[r][i];
    c.max > 0
        && eff[r][i] < c.max
        && (c.feeders.is_empty() || auto_stage(eff, &c.feeders) == 1)
}

/// A bracket reveal action, applied against the shared section set.
#[derive(Clone, Copy)]
enum BkOp {
    /// Cycle one series (the granular click).
    Series(usize, usize),
    /// Advance a whole round one stage, or reset it once fully revealed.
    Round(usize),
    /// Advance the earliest revealable round; reset all once everything is shown.
    Cascade,
}

/// Reveal a series' contributing matches (recursively) up to their scores, so the
/// series' own lineup becomes known. This is the "team reveal" step for a series
/// whose participants aren't decided yet.
fn reveal_feeders(set: &mut HashSet<String>, grid: &[Vec<BkCell>], r: usize, i: usize) {
    for &(fr, fi) in &grid[r][i].feeders {
        reveal_feeders(set, grid, fr, fi);
        let fc = &grid[fr][fi];
        apply_stage(set, &fc.bn, &fc.bs, fc.max);
    }
}

/// Whether a series' teams are known: it's a first-round match, or all its
/// feeders' scores are revealed.
fn bk_teams_known(grid: &[Vec<BkCell>], eff: &[Vec<u8>], r: usize, i: usize) -> bool {
    let c = &grid[r][i];
    c.feeders.is_empty() || auto_stage(eff, &c.feeders) == 1
}

/// Reveal a whole round to one uniform stage — the least-revealed decided match's
/// stage plus one. Bringing every series to the same stage pulls any matches that
/// were individually revealed earlier into the even progression, so the outer
/// "round"/"Bracket" controls don't preserve those granular reveals out of order
/// ("revealing is granular, hiding is bigger-picture").
fn reveal_round_uniform(set: &mut HashSet<String>, grid: &[Vec<BkCell>], eff: &[Vec<u8>], r: usize) {
    let min_stage = (0..grid[r].len())
        .filter(|&i| grid[r][i].max > 0)
        .map(|i| eff[r][i])
        .min()
        .unwrap_or(0);
    let target = (min_stage + 1).min(2);
    for i in 0..grid[r].len() {
        let c = &grid[r][i];
        if c.max == 0 {
            continue;
        }
        // A score can't show before the lineup is known.
        let cap = if bk_teams_known(grid, eff, r, i) { c.max } else { c.max.min(1) };
        let t = target.min(cap);
        // If the auto-revealed lineup already provides this stage, clear the manual
        // entry and let auto provide it (and so hiding a feeder still cascades).
        let auto = auto_stage(eff, &c.feeders);
        apply_stage(set, &c.bn, &c.bs, if t <= auto { 0 } else { t });
    }
}

/// Drop every manual reveal in the rounds after `r`, so an outer control (a round
/// toggle or the cascade) is authoritative over everything past it — cycling it
/// never leaves a later round revealed from earlier clicks. Auto-revealed lineups
/// aren't in the set, so they're untouched.
fn hide_rounds_after(set: &mut HashSet<String>, grid: &[Vec<BkCell>], r: usize) {
    for later in &grid[r + 1..] {
        for c in later {
            apply_stage(set, &c.bn, &c.bs, 0);
        }
    }
}

/// Apply a bracket reveal action to the shared set, respecting feeder gating so a
/// later round's lineup can't be revealed before its feeders' scores are shown.
fn apply_bracket_op(set: &mut HashSet<String>, grid: &[Vec<BkCell>], op: BkOp) {
    let eff = compute_effective(grid, set, false);
    let advance = |set: &mut HashSet<String>, r: usize, i: usize| {
        let c = &grid[r][i];
        let auto = auto_stage(&eff, &c.feeders);
        let next = (eff[r][i] + 1).min(c.max);
        // Falling back to the auto stage means "let auto provide it" → clear manual.
        let target = if next <= auto { 0 } else { next };
        apply_stage(set, &c.bn, &c.bs, target);
    };
    match op {
        BkOp::Series(r, i) => {
            let c = &grid[r][i];
            if c.max == 0 {
                return; // TBD: never revealable
            }
            if !bk_teams_known(grid, &eff, r, i) {
                // First step: reveal the contributing matches' scores; the lineup
                // then appears on its own (and you can't peek ahead unfairly).
                reveal_feeders(set, grid, r, i);
            } else if eff[r][i] >= c.max {
                // Fully shown → step back (auto keeps the names for a fed series).
                let auto = auto_stage(&eff, &c.feeders);
                apply_stage(set, &c.bn, &c.bs, auto);
            } else {
                advance(set, r, i);
            }
        }
        BkOp::Round(r) => {
            let n = grid[r].len();
            // If any series' teams aren't decided yet, the round's first step is to
            // reveal the contributing matches that feed it.
            let need_feeders: Vec<usize> = (0..n)
                .filter(|&i| grid[r][i].max > 0 && !bk_teams_known(grid, &eff, r, i))
                .collect();
            if !need_feeders.is_empty() {
                for i in need_feeders {
                    reveal_feeders(set, grid, r, i);
                }
            } else if (0..n).any(|i| bk_advanceable(grid, &eff, r, i)) {
                reveal_round_uniform(set, grid, &eff, r);
            } else {
                // Fully revealed → hide the whole round.
                for c in &grid[r] {
                    apply_stage(set, &c.bn, &c.bs, 0);
                }
            }
            // The round toggle owns everything after it: cycling it shouldn't keep
            // later rounds revealed from earlier granular clicks.
            hide_rounds_after(set, grid, r);
        }
        BkOp::Cascade => {
            let frontier =
                (0..grid.len()).find(|&r| (0..grid[r].len()).any(|i| bk_advanceable(grid, &eff, r, i)));
            match frontier {
                Some(r) => {
                    reveal_round_uniform(set, grid, &eff, r);
                    // Reveal strictly front-to-back: forget any granular reveals
                    // further along ("hiding is bigger-picture").
                    hide_rounds_after(set, grid, r);
                }
                None => {
                    for row in grid {
                        for c in row {
                            apply_stage(set, &c.bn, &c.bs, 0);
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn StandingsTable(rows: Vec<StandingRow>, tournament_id: i64, game: Game) -> impl IntoView {
    if rows.is_empty() {
        return ().into_any();
    }
    // The last column: CS2 counts maps, LoL games, the team sports a single
    // ranking value (games-back / points / win%).
    let record_label = game.standings_last_label();
    let show_last = game.standings_single_value();
    // Click the "Standings" title to reveal/hide the table.
    let (revealed, toggle) = section_reveal(format!("st:{tournament_id}"));
    // Always render every row so the table reserves its height — when hidden, the
    // team/record cells show blank placeholders so revealing doesn't shift the page.
    let body = move || {
        let show = revealed.get();
        rows.clone()
            .into_iter()
            .map(|r| {
                let (team, wl, maps) = if show {
                    let last = if show_last {
                        r.gb
                    } else {
                        format!("{}-{}", r.game_wins, r.game_losses)
                    };
                    // NHL carries OT losses as the third record number (W-L-OTL).
                    let wl = if r.ties > 0 {
                        format!("{}-{}-{}", r.wins, r.losses, r.ties)
                    } else {
                        format!("{}-{}", r.wins, r.losses)
                    };
                    (r.team, wl, last)
                } else {
                    ("—".to_string(), "—".to_string(), "—".to_string())
                };
                view! {
                    <tr>
                        <td class="st-rank">{r.rank}</td>
                        <td class="st-team">{team}</td>
                        <td class="st-wl">{wl}</td>
                        <td class="st-diff">{maps}</td>
                    </tr>
                }
            })
            .collect_view()
    };
    view! {
        <section class="detail-section">
            <div class="section-head">
                <button
                    class="section-title section-toggle"
                    class:on=move || revealed.get()
                    title=move || if revealed.get() { "Hide the standings" } else { "Show the standings" }
                    on:click=toggle
                >
                    "Standings"
                </button>
                {move || (!revealed.get()).then(|| view! { <span class="section-hint">"hidden"</span> })}
            </div>
            <table class="standings" class:placeholder=move || !revealed.get()>
                <thead>
                    <tr>
                        <th></th>
                        <th class="st-team">"Team"</th>
                        <th>"W-L"</th>
                        <th>{record_label}</th>
                    </tr>
                </thead>
                <tbody>{body}</tbody>
            </table>
        </section>
    }
    .into_any()
}

/// Render data for one bracket series (parallel to its [`BkCell`] in the grid).
struct BkRender {
    i: usize,
    ta: String,
    tb: String,
    sa: Option<i64>,
    sb: Option<i64>,
    winner: String,
    max: u8,
    mid: i64,
}

/// The day number from a `YYYY-MM-DD` key (leading zero stripped by parsing).
fn day_num(key: &str) -> u32 {
    key.rsplit('-').next().and_then(|s| s.parse().ok()).unwrap_or(0)
}

/// Format a `YYYY-MM-DD` day key as a short date, e.g. "Jun 18".
fn fmt_day_short(key: &str) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let month: usize = key.split('-').nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let Some(mon) = MONTHS.get(month.wrapping_sub(1)) else {
        return String::new();
    };
    format!("{mon} {}", day_num(key))
}

/// Format a round's date span from its sorted-unique day keys: a single day
/// ("Jun 18"), a same-month range ("Jun 18–19"), or a cross-month range
/// ("Jun 30 – Jul 1"). Empty when no dates are known.
fn fmt_date_range(keys: &[String]) -> String {
    let (Some(first), Some(last)) = (keys.first(), keys.last()) else {
        return String::new();
    };
    if first == last {
        return fmt_day_short(first);
    }
    if first.get(..7).is_some() && first.get(..7) == last.get(..7) {
        format!("{}–{}", fmt_day_short(first), day_num(last))
    } else {
        format!("{} – {}", fmt_day_short(first), fmt_day_short(last))
    }
}

/// One Swiss match's render data, paired with its `(round, position)` grid slot
/// and the resolved grid coordinates of its two feeders (each side's prior
/// match), so it reveals through the same staged, feeder-gated machinery as the
/// playoff bracket.
struct SwRender {
    r: usize,
    i: usize,
    team_a: String,
    team_b: String,
    sa: Option<i64>,
    sb: Option<i64>,
    winner: String,
    mid: i64,
    a_record: String,
    b_record: String,
    f0: Option<(usize, usize)>,
    f1: Option<(usize, usize)>,
    max: u8,
}

/// One Swiss record bucket for render: its entering record label and the matches
/// in it. A round is a `Vec` of these; the whole grid a `Vec` of rounds.
type SwBucketRender = (String, Vec<SwRender>);
/// One Swiss round for render: its heading and its record buckets.
type SwRoundRender = (String, Vec<SwBucketRender>);
/// A reachable outcome from a bucket: its record, whether it advances, and the
/// `(round, match)` positions that produce it (for spoiler-gating).
type SwOutcome = (String, bool, Vec<(usize, usize)>);

/// A Swiss / "first to N" stage drawn as a buchholz grid: a column per round,
/// each round's matches grouped into record buckets (2-0, 1-1, 0-2, …). Styled
/// and revealed exactly like the playoff bracket — winner bold, loser muted,
/// finishers badged with their final record (3-0 … 0-3). Progressive reveal is
/// feeder-gated: a side's name only shows once its previous-round match's score
/// is revealed, so you reveal round by round and can't peek ahead. A hidden but
/// played match shows a dot (there's a result to reveal); an unplayed one a
/// dash. Clicking a name scrolls to that match's row in the schedule.
#[component]
fn SwissBracket(rounds: Vec<SwissRound>, tournament_id: i64) -> impl IntoView {
    if rounds.is_empty() {
        return ().into_any();
    }
    let global = use_context::<ShowScores>().map(|s| s.0);
    let sections = use_context::<RevealedSections>().map(|s| s.0);
    let tid = tournament_id;
    // The team whose path is being traced (hovered), keyed by name.
    let hovered = RwSignal::new(String::new());

    // Map every match to its (round, position) slot so feeders resolve to grid
    // coordinates (position runs across a round's buckets, in display order).
    let mut pos: std::collections::HashMap<i64, (usize, usize)> = std::collections::HashMap::new();
    for (r, round) in rounds.iter().enumerate() {
        let mut i = 0usize;
        for bucket in &round.buckets {
            for m in &bucket.matches {
                if m.match_id > 0 {
                    pos.insert(m.match_id, (r, i));
                }
                i += 1;
            }
        }
    }

    // Build the reveal grid (one row per round, matches in column order) and the
    // parallel render tree (kept grouped into record buckets for display).
    let mut grid: Vec<Vec<BkCell>> = Vec::with_capacity(rounds.len());
    let mut render: Vec<SwRoundRender> = Vec::with_capacity(rounds.len());
    for (r, round) in rounds.into_iter().enumerate() {
        let head = format!("Round {}", round.number);
        let mut cells: Vec<BkCell> = Vec::new();
        let mut buckets_r: Vec<(String, Vec<SwRender>)> = Vec::new();
        let mut i = 0usize;
        for bucket in round.buckets {
            let mut ms: Vec<SwRender> = Vec::new();
            for m in bucket.matches {
                let max = sw_max_stage(&m);
                let f0 = m.a_feeder.and_then(|f| pos.get(&f).copied());
                let f1 = m.b_feeder.and_then(|f| pos.get(&f).copied());
                // Both feeders or neither (round 1) — keep slot order [a, b].
                let feeders = match (f0, f1) {
                    (Some(a), Some(b)) => vec![a, b],
                    _ => Vec::new(),
                };
                cells.push(BkCell {
                    bn: format!("swn:{tid}:{r}:{i}"),
                    bs: format!("sws:{tid}:{r}:{i}"),
                    max,
                    floor: 0,
                    feeders,
                });
                ms.push(SwRender {
                    r,
                    i,
                    team_a: m.team_a,
                    team_b: m.team_b,
                    sa: m.score_a,
                    sb: m.score_b,
                    winner: m.winner,
                    mid: m.match_id,
                    a_record: m.a_record,
                    b_record: m.b_record,
                    f0,
                    f1,
                    max,
                });
                i += 1;
            }
            buckets_r.push((bucket.record, ms));
        }
        grid.push(cells);
        render.push((head, buckets_r));
    }

    // Effective stages for the whole stage, recomputed when the shared reveal set
    // or the global toggle changes (same machinery as the playoff bracket).
    let grid_sv = StoredValue::new(grid.clone());
    let eff = Memo::new(move |_| {
        let g = global.is_some_and(|s| s.get());
        let set = sections.map(|s| s.get()).unwrap_or_default();
        compute_effective(&grid, &set, g)
    });
    let do_op = move |op: BkOp| {
        if let Some(sec) = sections {
            sec.update(|set| apply_bracket_op(set, &grid_sv.get_value(), op));
            #[cfg(feature = "hydrate")]
            save_sections(&sec.get_untracked());
        }
    };

    // Render one match box: feeder-gated per-slot reveal, dot/dash for a hidden
    // result, hover-trace, and a name that scrolls to the schedule row.
    let mk_match = move |m: SwRender| -> leptos::prelude::AnyView {
        let SwRender {
            r, i, team_a, team_b, sa, sb, winner, mid, a_record, b_record, f0, f1, max,
        } = m;
        let stage = move || eff.with(|e| e.get(r).and_then(|row| row.get(i)).copied().unwrap_or(0));
        let fed_shown = move |f: Option<(usize, usize)>| {
            f.is_some_and(|(fr, fi)| {
                eff.with(|e| e.get(fr).and_then(|row| row.get(fi)).copied().unwrap_or(0) >= 2)
            })
        };
        let team_link = move |name: String| -> leptos::prelude::AnyView {
            if mid <= 0 {
                return view! { <span class="sw-team">{name}</span> }.into_any();
            }
            view! {
                <a
                    class="sw-team sw-link"
                    href=format!("/match/{mid}")
                    title="Find this match in the schedule"
                    on:click=move |e: leptos::ev::MouseEvent| {
                        e.stop_propagation();
                        #[cfg(feature = "hydrate")]
                        if flash_match_row(mid) {
                            e.prevent_default();
                        }
                    }
                >
                    {name}
                </a>
            }
            .into_any()
        };
        let body = move || {
            let st = stage();
            // A fed slot is known once its feeder's score is shown; a round-1 slot
            // once this match itself is revealed. Scores need both sides known.
            let known_a = if f0.is_some() { fed_shown(f0) } else { st >= 1 };
            let known_b = if f1.is_some() { fed_shown(f1) } else { st >= 1 };
            let scores = st >= 2 && known_a && known_b;
            let score_view = move |s: Option<i64>| -> leptos::prelude::AnyView {
                if scores {
                    view! {
                        <span class="sw-score">{s.map(|v| v.to_string()).unwrap_or_default()}</span>
                    }
                    .into_any()
                } else if max >= 2 {
                    view! {
                        <span class="sw-score sw-pending" title="Result hidden — click to reveal">
                            "●"
                        </span>
                    }
                    .into_any()
                } else {
                    view! {
                        <span class="sw-score sw-unplayed" title="Not played yet">"-"</span>
                    }
                    .into_any()
                }
            };
            let row = move |known: bool,
                            win: bool,
                            lose: bool,
                            name: String,
                            score: Option<i64>,
                            rec: String|
                  -> leptos::prelude::AnyView {
                if !known {
                    return view! {
                        <div class="sw-row sw-hidden">
                            <span class="sw-team">"—"</span>
                        </div>
                    }
                    .into_any();
                }
                // A team that finished the stage (has a record) colours its name
                // green if it advanced (more wins) or red if it was knocked out;
                // the exact record sits above the bucket. Otherwise the usual
                // bold-winner / muted-loser styling.
                let cls = if scores && !rec.is_empty() {
                    if record_is_pass(&rec) {
                        "sw-row pass"
                    } else {
                        "sw-row exit"
                    }
                } else if win {
                    "sw-row win"
                } else if lose {
                    "sw-row lose"
                } else {
                    "sw-row"
                };
                // Hovering a team lights its row in every round (and dims the
                // rest), so you can trace its run. `mouseenter` is per-row but
                // `mouseleave` is on the box (below), so moving between the two
                // rows never clears the trace — no flash across the row gap.
                let (n_class, n_enter) = (name.clone(), name.clone());
                view! {
                    <div
                        class=cls
                        class:sw-path=move || hovered.with(|h| !h.is_empty() && *h == n_class)
                        on:mouseenter=move |_| hovered.set(n_enter.clone())
                    >
                        {team_link(name)}
                        {score_view(score)}
                    </div>
                }
                .into_any()
            };
            view! {
                {row(known_a, scores && winner == "a", scores && winner == "b", team_a.clone(), sa, a_record.clone())}
                {row(known_b, scores && winner == "b", scores && winner == "a", team_b.clone(), sb, b_record.clone())}
            }
            .into_any()
        };
        let title = if max == 0 { "Not decided yet" } else { "Click to reveal this match" };
        view! {
            <div
                class="sw-match"
                class:sw-locked=max == 0
                role="button"
                tabindex=if max == 0 { "-1" } else { "0" }
                title=title
                aria-label=title
                on:click=move |_| do_op(BkOp::Series(r, i))
                on:keydown=move |e: leptos::ev::KeyboardEvent| {
                    #[cfg(feature = "hydrate")]
                    if matches!(e.key().as_str(), "Enter" | " ") {
                        e.prevent_default();
                        do_op(BkOp::Series(r, i));
                    }
                    #[cfg(not(feature = "hydrate"))]
                    let _ = &e;
                }
                on:mouseleave=move |_| hovered.set(String::new())
            >
                {body}
            </div>
        }
        .into_any()
    };

    let cols = render
        .into_iter()
        .enumerate()
        .map(move |(r, (head, buckets_r))| {
            let buckets = buckets_r
                .into_iter()
                .map(move |(record, ms)| {
                    // The records teams reach from this bucket (3-0 advance, 0-3
                    // out, …), deduped, each with the match positions that produce
                    // it — shown above the group, gated on those matches being
                    // revealed so it isn't a spoiler.
                    let mut outcomes: Vec<SwOutcome> = Vec::new();
                    for m in &ms {
                        for rec in [&m.a_record, &m.b_record] {
                            if rec.is_empty() {
                                continue;
                            }
                            match outcomes.iter_mut().find(|(r, _, _)| r == rec) {
                                Some(e) => e.2.push((m.r, m.i)),
                                None => {
                                    outcomes.push((rec.clone(), record_is_pass(rec), vec![(m.r, m.i)]))
                                }
                            }
                        }
                    }
                    let outcome_view = (!outcomes.is_empty()).then_some(move || {
                        outcomes
                            .iter()
                            .filter(|(_, _, pos)| {
                                pos.iter().any(|&(rr, ii)| {
                                    eff.with(|e| {
                                        e.get(rr).and_then(|row| row.get(ii)).copied().unwrap_or(0) >= 2
                                    })
                                })
                            })
                            .map(|(rec, pass, _)| {
                                let cls = if *pass { "sw-out sw-pass" } else { "sw-out sw-exit" };
                                view! { <span class=cls>{rec.clone()}</span> }
                            })
                            .collect_view()
                    });
                    let matches = ms.into_iter().map(mk_match).collect_view();
                    view! {
                        <div class="sw-bucket">
                            <div class="sw-record">
                                <span class="sw-record-in">{record}</span>
                                <span class="sw-outs">{outcome_view}</span>
                            </div>
                            {matches}
                        </div>
                    }
                })
                .collect_view();
            // The round header reveals/hides this whole round (names → scores),
            // like the playoff bracket's round titles.
            let round_on =
                move || eff.with(|e| e.get(r).is_some_and(|row| row.iter().any(|&s| s >= 1)));
            view! {
                <div class="sw-col">
                    <button
                        class="sw-col-head sw-round-toggle"
                        class:on=round_on
                        title="Reveal this round"
                        on:click=move |_| do_op(BkOp::Round(r))
                    >
                        {head}
                    </button>
                    {buckets}
                </div>
            }
        })
        .collect_view();

    // The "Bracket" title cascades the whole stage: each click reveals the
    // earliest revealable round's scores, unlocking the next round's lineup.
    let bracket_on = move || eff.with(|e| e.iter().flatten().any(|&s| s >= 1));
    view! {
        <section class="detail-section swiss-section">
            <div class="section-head">
                <button
                    class="section-title section-toggle"
                    class:on=bracket_on
                    title="Reveal the bracket, round by round"
                    on:click=move |_| do_op(BkOp::Cascade)
                >
                    "Bracket"
                </button>
                {move || (!bracket_on()).then(|| view! { <span class="section-hint">"hidden"</span> })}
            </div>
            <div class="swiss-scroll">
                <div class="swiss" class:sw-tracing=move || hovered.with(|h| !h.is_empty())>
                    {cols}
                </div>
            </div>
        </section>
    }
    .into_any()
}

#[component]
fn Bracket(
    rounds: Vec<BracketRound>,
    tournament_id: i64,
    bracket_only: bool,
    /// `match_id -> (day_key, clock_label)` for the event's matches, so the
    /// bracket can date each round and show a per-match time on hover. Empty
    /// where the schedule isn't on the page (then no times are shown).
    #[prop(optional)]
    times: std::collections::HashMap<i64, (String, String)>,
) -> impl IntoView {
    if rounds.is_empty() {
        return ().into_any();
    }
    let times = StoredValue::new(times);
    let global = use_context::<ShowScores>().map(|s| s.0);
    let sections = use_context::<RevealedSections>().map(|s| s.0);
    let tid = tournament_id;

    // Position every match from the feeder graph; the render places boxes
    // absolutely at these slots and draws an SVG connector per edge. Slot units →
    // em: a round title (name over date) sits above each section's first box,
    // with a section banner above that for double-elim. `head_em` is the box
    // centre offset that reserves room for all of it above slot 0.
    let layout = bracket::layout(&rounds);
    const TITLE_EM: f64 = 2.8; // round name + date (two lines)
    const TITLE_GAP: f64 = 0.5; // below the date, above the box
    const LABEL_EM: f64 = 1.5; // section banner
    const LABEL_GAP: f64 = 0.5; // below the banner, above the title
    const TOP_PAD: f64 = 0.3; // above the topmost banner/title
    let half_h = bracket::BOX_H_EM / 2.0;
    let multi =
        layout.group_rows.len() > 1 || layout.group_rows.first().is_some_and(|(s, _, _)| !s.is_empty());
    let head_em = TOP_PAD
        + half_h
        + TITLE_GAP
        + TITLE_EM
        + if multi { LABEL_GAP + LABEL_EM } else { 0.0 };
    let center_em = move |y: f64| y * bracket::ROW_EM + head_em;

    // Build the reveal grid (keys/max/feeders, for stage computation + clicks)
    // and the parallel render data, once. `winners` holds each match's advancing
    // team so a fed match can show a single feeder's winner before its other
    // feeder is revealed (and before PandaScore has seeded that slot).
    let mut grid: Vec<Vec<BkCell>> = Vec::with_capacity(rounds.len());
    let mut render: Vec<(String, String, Vec<BkRender>)> = Vec::with_capacity(rounds.len());
    let mut winners: Vec<Vec<String>> = Vec::with_capacity(rounds.len());
    for (r, round) in rounds.into_iter().enumerate() {
        let mut cells = Vec::with_capacity(round.matches.len());
        let mut sres = Vec::with_capacity(round.matches.len());
        let mut wins = Vec::with_capacity(round.matches.len());
        for (i, m) in round.matches.into_iter().enumerate() {
            let max = bm_max_stage(&m);
            // For a bracket-only event, keep first-round matchups (no feeders)
            // visible by default — only their scores are a spoiler.
            let floor = u8::from(bracket_only && m.feeders.is_empty() && max >= 1);
            wins.push(match m.winner.as_str() {
                "a" => m.team_a.clone(),
                "b" => m.team_b.clone(),
                _ => String::new(),
            });
            cells.push(BkCell {
                bn: format!("bn:{tid}:{r}:{i}"),
                bs: format!("bs:{tid}:{r}:{i}"),
                max,
                floor,
                feeders: m.feeders,
            });
            sres.push(BkRender {
                i,
                ta: m.team_a,
                tb: m.team_b,
                sa: m.score_a,
                sb: m.score_b,
                winner: m.winner,
                max,
                mid: m.match_id,
            });
        }
        grid.push(cells);
        render.push((round.section, round.title, sres));
        winners.push(wins);
    }
    let winners_sv = StoredValue::new(winners);

    // Effective stages for the whole bracket, recomputed when the shared reveal
    // set or the global toggle changes.
    let grid_sv = StoredValue::new(grid.clone());
    let eff = Memo::new(move |_| {
        let g = global.is_some_and(|s| s.get());
        let set = sections.map(|s| s.get()).unwrap_or_default();
        compute_effective(&grid, &set, g)
    });
    // A single Copy handle for every reveal click (series / round / cascade).
    let do_op = move |op: BkOp| {
        if let Some(sec) = sections {
            sec.update(|set| apply_bracket_op(set, &grid_sv.get_value(), op));
            #[cfg(feature = "hydrate")]
            save_sections(&sec.get_untracked());
        }
    };

    let positions = &layout.positions;
    let group_rows = &layout.group_rows;
    // Each round (column) contributes a positioned title header plus its boxes,
    // all absolutely placed in one canvas (no flex columns).
    let nodes = render
        .into_iter()
        .enumerate()
        .map(|(r, (section, title, sres))| {
            let x_em = positions[r].first().map_or(0, |p| p.col) as f64 * bracket::COL_EM;
            let sec_ymin = group_rows
                .iter()
                .find(|(s, _, _)| s == &section)
                .map_or(0.0, |(_, lo, _)| *lo);
            let title_top = center_em(sec_ymin) - half_h - TITLE_GAP - TITLE_EM;
            let round_on = move || eff.with(|e| e.get(r).is_some_and(|row| row.iter().any(|&s| s >= 1)));
            // The round's date span, from its matches' schedule days.
            let round_date = {
                let mut keys: Vec<String> = sres
                    .iter()
                    .filter_map(|s| times.with_value(|t| t.get(&s.mid).map(|(d, _)| d.clone())))
                    .collect();
                keys.sort();
                keys.dedup();
                fmt_date_range(&keys)
            };
            let header = view! {
                <div class="bk-round-title" style=format!("left:{x_em}em;top:{title_top}em")>
                    <button
                        class="bk-round-toggle"
                        class:on=round_on
                        title="Reveal this round"
                        on:click=move |_| do_op(BkOp::Round(r))
                    >
                        {title}
                    </button>
                    {(!round_date.is_empty())
                        .then(|| view! { <span class="bk-round-date">{round_date}</span> })}
                </div>
            };
            let ms = sres
                .into_iter()
                .map(|s| {
                    let (i, max) = (s.i, s.max);
                    let (ta, tb) = (s.ta, s.tb);
                    let (sa, sb) = (s.sa, s.sb);
                    let winner = s.winner;
                    let mid = s.mid;
                    let top_em = center_em(positions[r][i].y) - half_h;
                    let match_title = if max == 0 { "Not decided yet" } else { "Reveal this match" };
                    // Box tooltip shows the match time when known (the schedule is
                    // on the page), else the reveal hint.
                    let box_title = times
                        .with_value(|t| {
                            t.get(&mid).map(|(d, c)| format!("{} · {}", fmt_day_short(d), c))
                        })
                        .unwrap_or_else(|| match_title.to_string());
                    let stage = move || eff.with(|e| e.get(r).and_then(|row| row.get(i)).copied().unwrap_or(0));
                    // A revealed team name links to its match page; the rest of
                    // the box still reveals on click (the link stops that
                    // propagation). TBD/demo matches (no id) stay plain text.
                    // Both names share one title and highlight together (via
                    // `:has` in CSS) so they read as a single link to the match.
                    // A click scrolls to this match's row in the schedule above
                    // (so you see it in context, then open it from there); where
                    // that row isn't on the page, the href opens the match page.
                    let link_title = format!("Find {ta} vs {tb} in the schedule");
                    let team_cell = move |name: String| {
                        if mid > 0 {
                            view! {
                                <a
                                    class="bk-team bk-team-link"
                                    href=format!("/match/{mid}")
                                    title=link_title.clone()
                                    on:click=move |e: leptos::ev::MouseEvent| {
                                        e.stop_propagation();
                                        #[cfg(feature = "hydrate")]
                                        if flash_match_row(mid) {
                                            e.prevent_default();
                                        }
                                    }
                                >
                                    {name}
                                </a>
                            }
                            .into_any()
                        } else {
                            view! { <span class="bk-team">{name}</span> }.into_any()
                        }
                    };
                    // Per-slot reveal: each side of a fed match shows its feeder's
                    // winner as soon as that feeder's score is revealed, independent
                    // of the other feeder. So one finished feeder reveals one team
                    // here, and hiding a feeder hides only its slot. `feeders[k]`
                    // maps to side k (verified against the PandaScore bracket).
                    let feeders = grid_sv.with_value(|g| g[r][i].feeders.clone());
                    let (f0, f1) = (feeders.first().copied(), feeders.get(1).copied());
                    let is_real = |s: &str| !s.is_empty() && s != "TBD";
                    // A slot's label is the match's own opponent, falling back to
                    // the feeder's winner when PandaScore hasn't seeded it yet.
                    let label_for = move |own: &str, f: Option<(usize, usize)>| {
                        if is_real(own) {
                            own.to_string()
                        } else {
                            f.map(|(fr, fi)| winners_sv.with_value(|w| w[fr][fi].clone()))
                                .unwrap_or_default()
                        }
                    };
                    let label_a = label_for(&ta, f0);
                    let label_b = label_for(&tb, f1);
                    let fed_shown = move |f: Option<(usize, usize)>| {
                        f.is_some_and(|(fr, fi)| eff.with(|e| e[fr][fi] >= 2))
                    };
                    view! {
                        // The box is absolutely placed at its computed slot; only
                        // the visible box is the click target (the SVG connectors
                        // behind it are `pointer-events:none`).
                        <div
                            class="bk-match"
                            class:bk-locked=max == 0
                            style=format!("left:{x_em}em;top:{top_em}em")
                        >
                            <div
                                class="bk-box"
                                role="button"
                                tabindex=if max == 0 { "-1" } else { "0" }
                                title=box_title.clone()
                                aria-label=box_title
                                on:click=move |_| do_op(BkOp::Series(r, i))
                                on:keydown=move |e: leptos::ev::KeyboardEvent| {
                                    #[cfg(feature = "hydrate")]
                                    if matches!(e.key().as_str(), "Enter" | " ") {
                                        e.prevent_default();
                                        do_op(BkOp::Series(r, i));
                                    }
                                    #[cfg(not(feature = "hydrate"))]
                                    let _ = &e;
                                }
                            >
                                {move || {
                                    let st = stage();
                                    // A fed slot is known only while its feeder's
                                    // score is shown (the team comes from that
                                    // result); a first-round slot is known once the
                                    // match itself is revealed. So hiding a feeder's
                                    // score makes the fed team unknown again, and a
                                    // played match can't show its score until both
                                    // of its teams are known. This cascades up the
                                    // bracket: hide a semifinal score and the final's
                                    // fed side goes blank.
                                    let known_a = if f0.is_some() { fed_shown(f0) } else { st >= 1 };
                                    let known_b = if f1.is_some() { fed_shown(f1) } else { st >= 1 };
                                    let scores = st >= 2 && known_a && known_b;
                                    let show_a = is_real(&label_a) && known_a;
                                    let show_b = is_real(&label_b) && known_b;
                                    let cls_a = if scores && winner == "a" {
                                        "bk-row win"
                                    } else if scores && winner == "b" {
                                        "bk-row lose"
                                    } else {
                                        "bk-row"
                                    };
                                    let cls_b = if scores && winner == "b" {
                                        "bk-row win"
                                    } else if scores && winner == "a" {
                                        "bk-row lose"
                                    } else {
                                        "bk-row"
                                    };
                                    // When a score is hidden, show whether there's
                                    // a result waiting — a played match gets a
                                    // filled dot you can reveal; one not yet played
                                    // gets a faint dash (nothing to reveal).
                                    let score_view = move |s: Option<i64>| {
                                        if scores {
                                            view! {
                                                <span class="bk-score">
                                                    {s.map(|v| v.to_string()).unwrap_or_default()}
                                                </span>
                                            }
                                            .into_any()
                                        } else if max >= 2 {
                                            view! {
                                                <span
                                                    class="bk-score bk-pending"
                                                    title="Result hidden — click to reveal"
                                                >
                                                    "●"
                                                </span>
                                            }
                                            .into_any()
                                        } else {
                                            view! {
                                                <span
                                                    class="bk-score bk-unplayed"
                                                    title="Not played yet"
                                                >
                                                    "-"
                                                </span>
                                            }
                                            .into_any()
                                        }
                                    };
                                    let row_a = if show_a {
                                        view! {
                                            <div class=cls_a>
                                                {team_cell(label_a.clone())}
                                                {score_view(sa)}
                                            </div>
                                        }
                                        .into_any()
                                    } else {
                                        view! {
                                            <div class="bk-row bk-hidden">
                                                <span class="bk-team">"—"</span>
                                            </div>
                                        }
                                        .into_any()
                                    };
                                    let row_b = if show_b {
                                        view! {
                                            <div class=cls_b>
                                                {team_cell(label_b.clone())}
                                                {score_view(sb)}
                                            </div>
                                        }
                                        .into_any()
                                    } else {
                                        view! {
                                            <div class="bk-row bk-hidden">
                                                <span class="bk-team">"—"</span>
                                            </div>
                                        }
                                        .into_any()
                                    };
                                    view! {
                                        {row_a}
                                        {row_b}
                                    }
                                }}
                            </div>
                        </div>
                    }
                })
                .collect_view();
            view! {
                {header}
                {ms}
            }
            .into_any()
        })
        .collect::<Vec<AnyView>>();

    // Connector overlay: one elbow path per feeder edge, drawn behind the boxes in
    // the same em coordinate space, so it stays attached at any shape or zoom.
    let w_em = layout.cols as f64 * bracket::COL_EM;
    let h_em = positions
        .iter()
        .flatten()
        .map(|p| center_em(p.y) + half_h)
        .fold(0.0_f64, f64::max)
        + 0.4;
    let paths = layout
        .edges
        .iter()
        .map(|e| {
            let (fr, fi) = e.from;
            let (tr, ti) = e.to;
            let x1 = positions[fr][fi].col as f64 * bracket::COL_EM + bracket::BOX_W_EM;
            let y1 = center_em(positions[fr][fi].y);
            let x2 = positions[tr][ti].col as f64 * bracket::COL_EM;
            let y2 = center_em(positions[tr][ti].y);
            let xm = (x1 + x2) / 2.0;
            view! { <path d=format!("M {x1:.2} {y1:.2} H {xm:.2} V {y2:.2} H {x2:.2}") /> }
        })
        .collect_view();

    // Section banners for double-elim (Winner's / Loser's Bracket); stop each at
    // the winner's/loser's-bracket edge so its underline never reaches the
    // grand-final rail to its right.
    let banner_w = bracket::banner_width_em(layout.bracket_cols);
    let labels = multi.then(|| {
        group_rows
            .iter()
            .filter_map(|(sec, lo, _)| {
                // The grand final's own round title labels it; a banner over it
                // would cut across the brackets it sits between.
                let label = match sec.as_str() {
                    "upper" => "Winner's Bracket",
                    "lower" => "Loser's Bracket",
                    _ => return None,
                };
                let top = center_em(*lo) - half_h - TITLE_GAP - TITLE_EM - LABEL_GAP - LABEL_EM;
                Some(view! {
                    <div class="bk-group-label" style=format!("top:{top}em;width:{banner_w:.2}em")>
                        {label}
                    </div>
                })
            })
            .collect_view()
    });

    let body = view! {
        <div class="bracket-scroll">
            <div class="bracket" style=format!("width:{w_em:.2}em;height:{h_em:.2}em")>
                <svg class="bk-connectors" viewBox=format!("0 0 {w_em:.2} {h_em:.2}")>
                    {paths}
                </svg>
                {labels}
                {nodes}
            </div>
        </div>
    }
    .into_any();

    // The "Bracket" title cascades the whole bracket: each click advances the
    // earliest revealable round (names → scores), unlocking the next round's
    // lineup as it goes; once everything is shown, the next click hides all.
    let bracket_on = move || eff.with(|e| e.iter().flatten().any(|&s| s >= 1));
    view! {
        <section class="detail-section">
            <div class="section-head">
                <button
                    class="section-title section-toggle"
                    class:on=bracket_on
                    title="Reveal the bracket, round by round"
                    on:click=move |_| do_op(BkOp::Cascade)
                >
                    "Bracket"
                </button>
                {move || (!bracket_on()).then(|| view! { <span class="section-hint">"hidden"</span> })}
            </div>
            {body}
        </section>
    }
    .into_any()
}

/// Standings + bracket shown beneath the schedule when exactly one event is
/// selected (ambiguous for multiple, so only a single selection shows it).
#[component]
fn EventSection(leagues: RwSignal<HashSet<String>>) -> impl IntoView {
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
        .get_item(keys::THEME)
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
            let _ = storage.set_item(keys::THEME, theme);
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

/// Switch between esports and traditional sports (MLB). Sits by the calendar.
#[component]
fn SportToggle() -> impl IntoView {
    let traditional = use_context::<SportMode>().expect("sport mode context").0;
    let earlier = use_context::<EarlierDays>().expect("earlier context").0;
    let later = use_context::<LaterDays>().expect("later context").0;
    let games = use_context::<Games>().expect("games context").0;
    let leagues = use_context::<Leagues>().expect("leagues context").0;
    let toggle = move |_| {
        let next = !traditional.get_untracked();
        traditional.set(next);
        // Start each mode from its default window (the forward cap differs).
        earlier.set(0);
        later.set(0);
        // The game/sport tabs share the `games` set, so clear it (and the esports
        // event chips) on a mode switch — a stale "cs2"/"mlb" slug would otherwise
        // filter out everything in the other mode.
        games.update(HashSet::clear);
        leagues.update(HashSet::clear);
        #[cfg(feature = "hydrate")]
        {
            save_sport_pref(next);
            save_str_set(keys::GAMES, &games.get_untracked());
            save_str_set(keys::LEAGUES, &leagues.get_untracked());
        }
    };
    view! {
        // The slot is fixed to the wider label ("esports") by an invisible sizer,
        // so the refresh label to its left never shifts when the word changes
        // width. The visible button hugs its own text and sits at the right of the
        // slot (any slack falls on the left).
        <div class="sport-slot">
            <span class="toggle sport-sizer" aria-hidden="true">"esports"</span>
            <button
                class="toggle sport-toggle"
                class:on=move || traditional.get()
                title="Switch between esports and traditional sports"
                on:click=toggle
            >
                {move || if traditional.get() { "sports" } else { "esports" }}
            </button>
        </div>
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

/// Manage what you're notified about: your game/event/team subscriptions, plus
/// individually-starred upcoming matches. Matches that have already started or
/// finished are left out (their reminders can no longer fire).
#[component]
fn NotificationsPage() -> impl IntoView {
    let subscribed = use_context::<Subscribed>().expect("subscribed context").0;
    let starred = use_context::<RwSignal<HashSet<i64>>>().expect("starred context");
    let excluded = use_context::<Excluded>().expect("excluded context").0;
    let vapid = use_context::<RwSignal<Option<String>>>().expect("vapid context");
    let hour24 = use_context::<RwSignal<bool>>().expect("hour24 context");
    let tz = use_context::<RwSignal<String>>().expect("tz context");

    // Resolve the starred ids to upcoming match views (re-fetches as you star or
    // remove); started/finished ids are dropped server-side.
    let starred_matches = Resource::new(
        move || {
            let mut ids: Vec<i64> = starred.get().into_iter().collect();
            ids.sort_unstable();
            (ids, tz.get(), hour24.get())
        },
        |(ids, z, h)| async move {
            if ids.is_empty() {
                Ok(Vec::new())
            } else {
                get_notifications(ids, z, h).await
            }
        },
    );

    // Anything to clear? (subscriptions or stars; exclusions alone aren't shown).
    let has_any =
        move || !subscribed.with(HashSet::is_empty) || !starred.with(HashSet::is_empty);
    let clear_all = move |_| {
        subscribed.set(HashSet::new());
        starred.set(HashSet::new());
        excluded.set(HashSet::new());
        #[cfg(feature = "hydrate")]
        {
            let empty: Vec<String> = Vec::new();
            let no_ids: Vec<i64> = Vec::new();
            save_subs(&empty);
            save_starred(&no_ids);
            save_excluded(&no_ids);
            clear_all_notifications(vapid.get_untracked());
        }
        #[cfg(not(feature = "hydrate"))]
        {
            let _ = vapid;
        }
    };

    view! {
        <article class="detail">
            <A href="/">"← schedule"</A>
            <div class="notif-title-row">
                <h1 class="detail-title">"Notifications"</h1>
                {move || {
                    has_any()
                        .then(|| {
                            view! {
                                <button class="linkish notif-clear" on:click=clear_all>
                                    "clear all"
                                </button>
                            }
                        })
                }}
            </div>
            <p class="notif-intro">
                "Everything you'll be reminded about. Subscriptions cover every "
                "upcoming match for a game, event, or team."
            </p>
            {move || {
                let mut keys: Vec<String> = subscribed.get().into_iter().collect();
                keys.sort();
                (!keys.is_empty())
                    .then(|| {
                        view! {
                            <section class="notif-section">
                                <h2 class="notif-head">"Subscriptions"</h2>
                                <ul class="notif-list">
                                    {keys
                                        .into_iter()
                                        .map(|k| view! { <SubRow key=k /> })
                                        .collect_view()}
                                </ul>
                            </section>
                        }
                    })
            }}
            <Suspense>
                {move || {
                    starred_matches
                        .get()
                        .map(|res| {
                            let ms = res.unwrap_or_default();
                            (!ms.is_empty())
                                .then(|| {
                                    view! {
                                        <section class="notif-section">
                                            <h2 class="notif-head">"Starred matches"</h2>
                                            <ul class="notif-list">
                                                {ms
                                                    .into_iter()
                                                    .map(|m| view! { <StarredRow m=m /> })
                                                    .collect_view()}
                                            </ul>
                                        </section>
                                    }
                                })
                        })
                }}
            </Suspense>
            {move || {
                let none = subscribed.with(HashSet::is_empty) && starred.with(HashSet::is_empty);
                none.then(|| {
                    view! {
                        <p class="empty">
                            "Nothing yet. Subscribe to a game, event, or team from its page "
                            "(the ★), or star an upcoming match to be reminded."
                        </p>
                    }
                })
            }}
        </article>
    }
}

/// One subscription row on the notifications page — its label (linking to the
/// team/event page where there is one) and a remove button.
#[component]
fn SubRow(key: String) -> impl IntoView {
    let subscribed = use_context::<Subscribed>().expect("subscribed context").0;
    let vapid = use_context::<RwSignal<Option<String>>>().expect("vapid context");
    let (kind, value) = key
        .split_once('|')
        .map_or_else(|| (String::new(), key.clone()), |(k, v)| (k.to_string(), v.to_string()));
    // Label + optional link to the subscribed thing's own page.
    let (tag, label, link): (&str, String, Option<String>) = match kind.as_str() {
        "game" => (
            "game",
            Game::from_filter(&value).map_or_else(|| value.clone(), |g| g.label().to_string()),
            None,
        ),
        "team" => ("team", value.clone(), Some(format!("/team/{}", enc_segment(&value)))),
        _ => ("event", value.clone(), Some(format!("/event/{}", enc_segment(&value)))),
    };

    let key_for_click = key.clone();
    let remove = move |_| {
        subscribed.update(|s| {
            s.remove(&key_for_click);
        });
        #[cfg(feature = "hydrate")]
        {
            let keys: Vec<String> = subscribed.with_untracked(|s| s.iter().cloned().collect());
            subscribe_scope(kind.clone(), value.clone(), false, keys, vapid.get_untracked());
        }
        #[cfg(not(feature = "hydrate"))]
        {
            let _ = (&kind, &value, vapid);
        }
    };

    let label_view = match link {
        Some(href) => view! { <A href=href>{label}</A> }.into_any(),
        None => label.into_any(),
    };

    view! {
        <li class="notif-item">
            <span class="notif-kind">{tag}</span>
            <span class="notif-label">{label_view}</span>
            <button class="notif-remove" on:click=remove aria-label="Remove subscription">
                "×"
            </button>
        </li>
    }
}

/// One individually-starred upcoming match on the notifications page.
#[component]
fn StarredRow(m: MatchView) -> impl IntoView {
    let starred = use_context::<RwSignal<HashSet<i64>>>().expect("starred context");
    let excluded = use_context::<Excluded>().expect("excluded context").0;
    let subscribed = use_context::<Subscribed>().expect("subscribed context").0;
    let vapid = use_context::<RwSignal<Option<String>>>().expect("vapid context");
    let id = m.id;
    // F1 (single-entity) has no opponent: show the GP and session (e.g.
    // "Austrian Grand Prix · Race") instead of a "Race at " matchup.
    let line = if m.game.single_entity() {
        if m.serie_name.is_empty() {
            m.team_a.label.clone()
        } else {
            format!("{} · {}", m.serie_name, m.team_a.label)
        }
    } else {
        format!("{} {} {}", m.team_a.label, versus_sep(m.game), m.team_b.label)
    };
    // The scopes that also cover this match — so removing it actually stops the
    // notification (opt out) rather than just dropping a redundant star.
    let keys = [
        format!("game|{}", m.game.slug()),
        format!("league|{}", m.league),
        format!("team|{}", m.team_a.name),
        format!("team|{}", m.team_b.name),
        format!("event|{}", full_event_name(&m.league, &m.serie_name)),
    ];
    let remove = move |_| {
        let covered = subscribed.with_untracked(|s| keys.iter().any(|k| s.contains(k)));
        starred.update(|s| {
            s.remove(&id);
        });
        if covered {
            excluded.update(|e| {
                e.insert(id);
            });
        }
        #[cfg(feature = "hydrate")]
        {
            save_starred(&starred.get_untracked().iter().copied().collect::<Vec<_>>());
            save_excluded(&excluded.get_untracked().iter().copied().collect::<Vec<_>>());
            sync_match_reminder(id, false, covered, vapid.get_untracked());
        }
        #[cfg(not(feature = "hydrate"))]
        {
            let _ = (covered, vapid);
        }
    };

    view! {
        <li class="notif-item">
            <A href=format!("/match/{id}")>
                <span class="notif-time">{m.clock_label}</span>
                <span class="notif-label">{line}</span>
            </A>
            <button class="notif-remove" on:click=remove aria-label="Remove reminder">
                "×"
            </button>
        </li>
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
    let v = storage.get_item(keys::HOUR24).ok().flatten()?;
    Some(v != "0")
}

#[cfg(feature = "hydrate")]
fn save_hour24_pref(hour24: bool) {
    if let Some(win) = web_sys::window() {
        if let Ok(Some(storage)) = win.local_storage() {
            let _ = storage.set_item(keys::HOUR24, if hour24 { "1" } else { "0" });
        }
    }
}

#[cfg(feature = "hydrate")]
fn load_scores_pref() -> bool {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item(keys::SCORES).ok().flatten())
        .is_some_and(|v| v == "1")
}

/// Read a cookie value by name (client-side).
#[cfg(feature = "hydrate")]
fn read_cookie(name: &str) -> Option<String> {
    use wasm_bindgen::JsValue;
    let doc = web_sys::window()?.document()?;
    let cookies = js_sys::Reflect::get(&doc, &JsValue::from_str("cookie")).ok()?.as_string()?;
    cookies.split(';').find_map(|kv| {
        let (k, v) = kv.trim().split_once('=')?;
        (k == name).then(|| v.to_string())
    })
}

/// Write a long-lived, root-scoped cookie (client-side).
#[cfg(feature = "hydrate")]
fn write_cookie(name: &str, value: &str) {
    use wasm_bindgen::JsValue;
    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
        // ~1 year; lax so it rides top-level navigations (shared links).
        let v = format!("{name}={value}; path=/; max-age=31536000; samesite=lax");
        let _ = js_sys::Reflect::set(&doc, &JsValue::from_str("cookie"), &JsValue::from_str(&v));
    }
}

/// Resolve the initial sport mode (`true` = traditional sports) from the page's
/// inputs. Shared by SSR (request URL + `Cookie` header) and hydrate (location +
/// `document.cookie`) so both render the same mode — no first-paint flash and no
/// hydration mismatch. Precedence: an explicit `?mode` wins (a shared link); else
/// infer from a known game slug in `?g` (e.g. `?g=nhl` ⇒ sports); else the saved
/// `mode` cookie ("sports"/"esports"), defaulting to esports.
#[cfg(any(feature = "ssr", feature = "hydrate"))]
fn resolve_sport_mode(
    mode_param: Option<&str>,
    g_param: Option<&str>,
    cookie_mode: Option<&str>,
) -> bool {
    if let Some(m) = mode_param.filter(|v| !v.is_empty()) {
        return m == "sports";
    }
    if let Some(v) = g_param.filter(|v| !v.is_empty()) {
        let known: Vec<Game> = v.split(',').filter_map(Game::from_filter).collect();
        if !known.is_empty() {
            return known.iter().any(|g| g.traditional());
        }
    }
    cookie_mode == Some("sports")
}

/// The first `key`'s value in an `&`-separated query string (values stay URL-
/// encoded; the params we read — `mode`/`g` — are ASCII slugs).
#[cfg(any(feature = "ssr", feature = "hydrate"))]
fn query_param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then(|| v.to_string())
    })
}

/// The initial sport mode for this render, read server-side from the request so
/// the first paint already matches the client (see [`resolve_sport_mode`]).
#[cfg(feature = "ssr")]
fn initial_sport_mode() -> bool {
    let Some(parts) = use_context::<http::request::Parts>() else {
        return false;
    };
    let query = parts.uri.query().unwrap_or_default();
    let cookie_mode = parts
        .headers
        .get(http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|cookies| {
            cookies.split(';').find_map(|kv| {
                let (k, v) = kv.trim().split_once('=')?;
                (k == SPORT_MODE_KEY).then(|| v.to_string())
            })
        });
    resolve_sport_mode(
        query_param(query, SPORT_MODE_KEY).as_deref(),
        query_param(query, "g").as_deref(),
        cookie_mode.as_deref(),
    )
}

#[cfg(feature = "hydrate")]
fn initial_sport_mode() -> bool {
    let search = web_sys::window().and_then(|w| w.location().search().ok()).unwrap_or_default();
    let query = search.trim_start_matches('?');
    resolve_sport_mode(
        query_param(query, SPORT_MODE_KEY).as_deref(),
        query_param(query, "g").as_deref(),
        read_cookie(SPORT_MODE_KEY).as_deref(),
    )
}

#[cfg(not(any(feature = "ssr", feature = "hydrate")))]
fn initial_sport_mode() -> bool {
    false
}

#[cfg(feature = "hydrate")]
fn save_sport_pref(traditional: bool) {
    write_cookie(SPORT_MODE_KEY, if traditional { "sports" } else { "esports" });
}

#[cfg(feature = "hydrate")]
fn save_scores_pref(show: bool) {
    if let Some(win) = web_sys::window() {
        if let Ok(Some(storage)) = win.local_storage() {
            let _ = storage.set_item(keys::SCORES, if show { "1" } else { "0" });
        }
    }
}

#[cfg(feature = "hydrate")]
fn save_subs(keys: &[String]) {
    if let Some(win) = web_sys::window() {
        if let Ok(Some(storage)) = win.local_storage() {
            let _ = storage.set_item(keys::SUBS, &keys.join("\n"));
        }
    }
}

#[cfg(feature = "hydrate")]
fn load_subs() -> HashSet<String> {
    let mut out = HashSet::new();
    if let Some(win) = web_sys::window() {
        if let Ok(Some(storage)) = win.local_storage() {
            if let Ok(Some(v)) = storage.get_item(keys::SUBS) {
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
        d.get_month() + 1,
        d.get_date()
    )
}

/// Add `delta` days to an ISO `YYYY-MM-DD` date, returning ISO. Pure (no JS) so
/// it's deterministic on both SSR and hydrate — used to window an event's days
/// relative to the server-provided `today_key`, which needs no browser clock.
fn iso_add_days(iso: &str, delta: i64) -> String {
    let mut it = iso.split('-');
    let y: i64 = it.next().and_then(|s| s.parse().ok()).unwrap_or(1970);
    let m: i64 = it.next().and_then(|s| s.parse().ok()).unwrap_or(1);
    let d: i64 = it.next().and_then(|s| s.parse().ok()).unwrap_or(1);
    let (ny, nm, nd) = civil_from_days(days_from_civil(y, m, d) + delta);
    format!("{ny:04}-{nm:02}-{nd:02}")
}

/// Days since 1970-01-01 for a proleptic-Gregorian date (Howard Hinnant's algo).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146097 + doe - 719468
}

/// Inverse of [`days_from_civil`]: `(year, month, day)` for a days-since-epoch.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(feature = "hydrate")]
fn save_range(range: Option<(String, String)>) {
    if let Some(win) = web_sys::window() {
        if let Ok(Some(storage)) = win.local_storage() {
            match range {
                Some((s, e)) => {
                    let _ = storage.set_item(keys::RANGE, &format!("{s}|{e}"));
                }
                None => {
                    let _ = storage.remove_item(keys::RANGE);
                }
            }
        }
    }
}

#[cfg(feature = "hydrate")]
fn load_range() -> Option<(String, String)> {
    let storage = web_sys::window()?.local_storage().ok().flatten()?;
    let v = storage.get_item(keys::RANGE).ok().flatten()?;
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
    // The silent refresh re-renders (replaces) the schedule subtree, which drops
    // the browser's scroll anchor and can jump the page. Remember the scroll
    // position when the timer fires and restore it once the new schedule renders.
    let pending = StoredValue::new(None::<f64>);
    // Save the scroll position, then refetch the schedule from the server (so the
    // re-render doesn't jump the page). Shared by the 60s timer and the header's
    // manual button — neither forces the *server* to re-poll its own source.
    let refresh = move || {
        #[cfg(feature = "hydrate")]
        {
            pending.set_value(web_sys::window().and_then(|w| w.scroll_y().ok()));
            resource.refetch();
        }
        #[cfg(not(feature = "hydrate"))]
        let _ = (pending, resource);
    };
    Effect::new(move |_| {
        #[cfg(feature = "hydrate")]
        {
            use std::time::Duration;
            // Keep the handle so client-side route changes clear the timer instead
            // of leaking a new 60s refetch loop on every page mount.
            if let Ok(handle) =
                set_interval_with_handle(refresh, Duration::from_secs(60))
            {
                on_cleanup(move || handle.clear());
            }
        }
        let _ = (resource, pending, refresh);
    });
    // Manual refresh from the header button: a shared, incrementing trigger.
    if let Some(RefreshTrigger(trigger)) = use_context::<RefreshTrigger>() {
        Effect::new(move |prev: Option<u64>| {
            let n = trigger.get();
            if prev.is_some() {
                refresh(); // skip the initial registration run
            }
            n
        });
    }
    // After the refreshed schedule renders, put the scroll back — only for the
    // silent auto-refresh; user-driven refetches leave `pending` unset.
    Effect::new(move |_| {
        resource.track();
        #[cfg(feature = "hydrate")]
        if let Some(y) = pending.get_value() {
            pending.set_value(None);
            request_animation_frame(move || {
                if let Some(w) = web_sys::window() {
                    w.scroll_to_with_x_and_y(0.0, y);
                }
            });
        }
        let _ = pending;
    });
}

#[component]
fn HomePage() -> impl IntoView {
    // Game/event filters are multi-select and applied client-side (the server
    // always returns all games), so the data is fetched once regardless. Shared
    // via context + persisted so they survive navigating away and back.
    let games = use_context::<Games>().expect("games context").0;
    let leagues = use_context::<Leagues>().expect("leagues context").0;
    let hour24 = use_context::<RwSignal<bool>>().expect("hour24 context");
    let tz = use_context::<RwSignal<String>>().expect("tz context");
    let range = use_context::<DateRange>().expect("range context").0;
    let earlier = use_context::<EarlierDays>().expect("earlier context").0;
    let later = use_context::<LaterDays>().expect("later context").0;
    let traditional = use_context::<SportMode>().expect("sport mode context").0;
    // Default = today + future (get_schedule). A calendar range, else an
    // earlier/later expansion, switches to the range view. The sport mode picks
    // esports ("all") vs MLB and re-fetches when toggled. Traditional sports cap
    // the forward window and reveal more days with "show later days".
    let schedule = Resource::new(
        move || (range.get(), earlier.get(), later.get(), tz.get(), hour24.get(), traditional.get()),
        |(r, e, l, z, h, trad)| async move {
            // "trad" = all traditional sports; the sport tabs narrow it client-side.
            let f = if trad { "trad" } else { "all" }.to_string();
            match r {
                Some((start, end)) => get_range(start, end, f, z, h).await,
                None if trad && (e > 0 || l > 0) => {
                    let (start, end) = trad_window(e, l);
                    get_range(start, end, f, z, h).await
                }
                None if e > 0 => {
                    let (start, end) = earlier_window(e);
                    get_range(start, end, f, z, h).await
                }
                None => get_schedule(f, z, h).await,
            }
        },
    );
    setup_autorefresh(schedule);

    view! {
        <FilterTabs games leagues traditional_row=false />
        <FilterTabs games leagues traditional_row=true />
        <ScheduleSection resource=schedule games leagues show_nav=false />
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

/// The window for a traditional-sports schedule: `earlier` days back through a
/// forward horizon of `TRAD_FORWARD_DAYS + later` days (capped at
/// `TRAD_FORWARD_MAX`). Computed client-side; only reached after a click, so SSR
/// never hits it (earlier/later start at 0 → the default `get_schedule`).
fn trad_window(earlier: i64, later: i64) -> (String, String) {
    let fwd = (crate::types::TRAD_FORWARD_DAYS + later).min(crate::types::TRAD_FORWARD_MAX);
    #[cfg(feature = "hydrate")]
    {
        (iso_from_today(-earlier), iso_from_today(fwd))
    }
    #[cfg(not(feature = "hydrate"))]
    {
        let _ = (earlier, fwd);
        (String::new(), String::new())
    }
}

/// Forward horizon (in days) of the traditional-sports window for a given
/// "later" expansion: `TRAD_FORWARD_DAYS` by default, growing to `TRAD_FORWARD_MAX`.
fn trad_forward(later: i64) -> i64 {
    (crate::types::TRAD_FORWARD_DAYS + later).min(crate::types::TRAD_FORWARD_MAX)
}

/// Inclusive `[start, end]` ISO day-keys of the traditional window relative to
/// `today` (an ISO day-key): `earlier` days back through `trad_forward(later)`
/// days forward. Pure (built on [`iso_add_days`]) so it's identical on SSR and
/// hydrate — the event page filters its days by this without a browser clock.
fn trad_day_bounds(today: &str, earlier: i64, later: i64) -> (String, String) {
    (iso_add_days(today, -earlier), iso_add_days(today, trad_forward(later)))
}

/// Whether an event/team page should cap its forward window. Only MLB needs it:
/// its event page (`/event/MLB`) is the whole season, so it windows like the
/// homepage. An F1 event is a single Grand Prix weekend and esports events are
/// finite, so both show in full.
fn schedule_needs_window(s: &ScheduleView) -> bool {
    s.days
        .iter()
        .flat_map(|d| &d.leagues)
        .flat_map(|lg| &lg.matches)
        // The team sports play often, so an event schedule of them caps its
        // horizon like the homepage; F1 (single-entity) shows its whole GP.
        .any(|m| {
            matches!(
                m.game,
                Game::Mlb | Game::Nhl | Game::Nba | Game::Nfl | Game::Soccer
            )
        })
}

/// `(season, round)` for an F1 event's schedule, recovered from a session id
/// (`season * 100000 + round * 100 + session`, see `f1.rs`). `None` for non-F1.
fn f1_season_round(s: &ScheduleView) -> Option<(i64, i64)> {
    let id = s
        .days
        .iter()
        .flat_map(|d| &d.leagues)
        .flat_map(|lg| &lg.matches)
        .find(|m| m.game == Game::F1)?
        .id;
    Some((id / 100_000, (id / 100) % 1000))
}

/// Whether a loaded schedule is a traditional sport (any match is, e.g. MLB) —
/// data-driven so it's the same on SSR and hydrate (unlike the `SportMode`
/// toggle, which only settles after hydration).
fn schedule_is_traditional(s: &ScheduleView) -> bool {
    s.days
        .iter()
        .flat_map(|d| &d.leagues)
        .flat_map(|lg| &lg.matches)
        .any(|m| m.game.traditional())
}

/// The separator between the two teams of a match. Traditional sports list the
/// away team "at" the home team (team_a is away, team_b home — see `mlb.rs`);
/// esports use "vs".
const fn versus_sep(game: Game) -> &'static str {
    if game.traditional() {
        "at"
    } else {
        "vs"
    }
}

/// Bottom-of-schedule control for traditional sports: a "show later days ›"
/// expander that reveals more future days, hidden once the forward window hits
/// `TRAD_FORWARD_MAX`. Renders nothing in esports mode.
#[component]
fn LaterControl() -> impl IntoView {
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
fn EarlierControl() -> impl IntoView {
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
    (d.get_full_year() as i32, d.get_month() + 1, d.get_date())
}

/// A 📅 button that opens a monospace month-grid range picker. Click a start day
/// then an end day, Apply to filter the schedule to that range.
#[component]
fn CalendarPicker() -> impl IntoView {
    let range = use_context::<DateRange>().expect("range context").0;
    let earlier = use_context::<EarlierDays>().expect("earlier context").0;
    // The forward expansion + sport mode let the calendar mirror the traditional
    // window: "show later days" visibly extends the highlighted span forward.
    let later = use_context::<LaterDays>().expect("later context").0;
    let sport_mode = use_context::<SportMode>().expect("sport mode context").0;
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
        // With no explicit selection, mirror the active schedule window: the
        // "‹ show earlier days" lookback, plus (for traditional sports) the
        // capped forward span that "show later days ›" extends — so the calendar
        // tracks what the schedule is showing.
        let window_span: Option<(String, String)> = {
            #[cfg(feature = "hydrate")]
            {
                if start.is_some() || end.is_some() {
                    None
                } else if sport_mode.get() {
                    Some((iso_from_today(-earlier.get()), iso_from_today(trad_forward(later.get()))))
                } else if earlier.get() > 0 {
                    Some((iso_from_today(-earlier.get()), iso_from_today(0)))
                } else {
                    None
                }
            }
            #[cfg(not(feature = "hydrate"))]
            {
                let _ = (earlier, later, sport_mode);
                None
            }
        };
        let lead = weekday(y, m, 1);
        let mut cells: Vec<_> = (0..lead)
            .map(|_| view! { <span class="cal-cell cal-blank"></span> }.into_any())
            .collect();
        for day in 1..=days_in_month(y, m) {
            let iso = format!("{y:04}-{m:02}-{day:02}");
            let in_range = match (&start, &end) {
                (Some(s), Some(e)) => iso.as_str() >= s.as_str() && iso.as_str() <= e.as_str(),
                (Some(s), None) => iso.as_str() == s.as_str(),
                _ => window_span
                    .as_ref()
                    .is_some_and(|(s, e)| iso.as_str() >= s.as_str() && iso.as_str() <= e.as_str()),
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
    let games = use_context::<Games>().expect("games context").0;
    let leagues = use_context::<Leagues>().expect("leagues context").0;
    let hour24 = use_context::<RwSignal<bool>>().expect("hour24 context");
    let tz = use_context::<RwSignal<String>>().expect("tz context");
    let traditional = use_context::<SportMode>().expect("sport mode context").0;
    let schedule = Resource::new(
        move || (date(), tz.get(), hour24.get(), traditional.get()),
        |(d, z, h, trad)| async move {
            get_day(d, if trad { "trad" } else { "all" }.into(), z, h).await
        },
    );
    setup_autorefresh(schedule);

    view! {
        <FilterTabs games leagues traditional_row=false />
        <FilterTabs games leagues traditional_row=true />
        <ScheduleSection resource=schedule games leagues show_nav=true />
    }
}

/// A row of filter tabs: the esports games (CS2/LoL) when `traditional_row` is
/// false, or the traditional sports (MLB/NHL/.../F1) when true. Each tab carries
/// a subscribe ★ for the whole game and clicks to (de)select it; the row hides
/// itself in the other top-level mode. The `games`/`leagues` filter sets are
/// shared across both rows, so a selection narrows the schedule.
#[component]
fn FilterTabs(
    games: RwSignal<HashSet<String>>,
    leagues: RwSignal<HashSet<String>>,
    traditional_row: bool,
) -> impl IntoView {
    let traditional = use_context::<SportMode>().expect("sport mode context").0;
    // Filters are multi-select and additive: no selection means "all". Click a
    // game to include it; click again to drop it.
    let toggle = move |value: &'static str| {
        games.update(|g| {
            if !g.remove(value) {
                g.insert(value.to_string());
            }
        });
        #[cfg(feature = "hydrate")]
        save_str_set(keys::GAMES, &games.get_untracked());
    };
    // A game filter tab with an embedded ★ that subscribes to the whole game.
    // The label clicks to filter; the ★ clicks to (un)subscribe.
    let with_star = move |label: &'static str, value: &'static str| {
        view! {
            <span class="tab tab-with-star" class:active=move || games.with(|g| g.contains(value))>
                <button class="tab-label" on:click=move |_| toggle(value)>
                    {label}
                </button>
                <SubscribeStar kind="game" value=value.to_string() />
            </span>
        }
    };
    // A "clear" appears once any game/event filter is active and resets them all.
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
    let tabs = Game::ALL
        .iter()
        .filter(|g| g.traditional() == traditional_row)
        .map(|g| with_star(g.label(), g.slug()))
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
fn LeagueChips(leagues: Vec<String>, selected: RwSignal<HashSet<String>>) -> impl IntoView {
    // Multi-select: no active chip means all events. Click an event to include it;
    // click again to drop it.
    let chips = leagues
        .into_iter()
        .map(|name| {
            let lc = league_color_class(&name);
            let sel_name = name.clone();
            let click_name = name.clone();
            let is_active = move || selected.with(|s| s.contains(&sel_name));
            view! {
                <button
                    class=format!("chip {lc}")
                    class:active=is_active
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
            }
        })
        .collect_view();

    view! { <div class="chips">{chips}</div> }
}

#[component]
fn ScheduleSection(
    resource: Resource<Result<ScheduleView, ServerFnError>>,
    games: RwSignal<HashSet<String>>,
    leagues: RwSignal<HashSet<String>>,
    show_nav: bool,
) -> impl IntoView {
    // The game/event filters and per-event standings are esports-only; MLB mode
    // shows the whole MLB schedule unfiltered.
    let traditional = use_context::<SportMode>().expect("sport mode context").0;
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
                            // shown only when the view spans more than one league —
                            // e.g. soccer's Premier League + World Cup — so a single-
                            // league sport doesn't get a chip that just repeats its
                            // tab.
                            let available = leagues_for_games(&s, &games_set);
                            let show_chips =
                                if trad { available.len() > 1 } else { !available.is_empty() };
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
    }
}

/// Distinct league names among groups whose game passes the filter (empty set =
/// all games), in first-appearance order. Drives the event chip list, so the
/// chips reflect the selected games.
fn leagues_for_games(s: &ScheduleView, games: &HashSet<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for day in &s.days {
        for lg in &day.leagues {
            let game_ok = games.is_empty()
                || lg.matches.first().is_some_and(|m| games.contains(m.game.slug()));
            if game_ok && !out.iter().any(|l| l == &lg.league) {
                out.push(lg.league.clone());
            }
        }
    }
    out
}

/// Keep only groups matching the selected games AND events (empty set = no
/// constraint on that axis); drop days left empty.
fn filter_schedule(
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
                || lg.matches.first().is_some_and(|m| games.contains(m.game.slug()));
            let league_ok = leagues.is_empty() || leagues.contains(&lg.league);
            game_ok && league_ok
        });
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

/// A compact "up next" bar pinned to the bottom of the event page, previewing
/// the current/next day's matches. Clicking it scrolls to that day in the list;
/// it hides once that day scrolls into view, so it never duplicates what's shown.
#[component]
fn UpNextBar(day: DayGroup) -> impl IntoView {
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
        if upcoming { "▸ Up next" } else { "▸ Latest" },
        day.day_label
    );
    let more = all.len().saturating_sub(CAP);
    let rows = all
        .into_iter()
        .take(CAP)
        .map(|m| {
            let mid = m.id;
            let sep = versus_sep(m.game);
            // F1 (single-entity) has no opponent: show just the session label, with
            // no "at"/"vs" separator and no empty second team.
            let teams = if m.game.single_entity() {
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
                    href=format!("/match/{mid}")
                    on:click=move |e: leptos::ev::MouseEvent| {
                        e.stop_propagation();
                        #[cfg(feature = "hydrate")]
                        if flash_match_row(mid) {
                            e.prevent_default();
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
                    let first =
                        entries.get(0).unchecked_into::<web_sys::IntersectionObserverEntry>();
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

fn render_schedule(
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
                    d.leagues.iter().flat_map(|lg| &lg.matches).any(|m| {
                        matches!(m.status, MatchStatus::Live | MatchStatus::Upcoming)
                    })
                })
                .or_else(|| days.len().checked_sub(1))
                .and_then(|i| days.get(i).cloned())
        })
        .flatten();

    let day_sections = days
        .into_iter()
        .enumerate()
        .map(|(idx, d)| {
            let leagues = d
                .leagues
                .into_iter()
                .map(|lg| {
                    // On the event page the page title already names the event, so we
                    // drop the per-day league header, the colour-coded bar, and show
                    // the best-of per row instead.
                    let league_class = if event_mode {
                        "league league-plain".to_string()
                    } else {
                        format!("league {}", league_color_class(&lg.league))
                    };
                    // The best-of always shows per row (right of each match); the
                    // event title no longer repeats it.
                    let show_bo = true;
                    let league_name = lg.league.clone();
                    // Title the group with the full event name (league + edition,
                    // e.g. "IEM Katowice"); the subscribe key stays the short
                    // league name, while the full name keys the event link.
                    let serie = lg.serie_name.clone();
                    let head = (!event_mode).then(move || {
                        let display = full_event_name(&league_name, &serie);
                        // The full edition name keys its event page, so distinct
                        // editions get distinct URLs.
                        let event_href = format!("/event/{}", enc_segment(&display));
                        view! {
                            <div class="league-head">
                                <SubscribeStar kind="league" value=league_name.clone() />
                                <h3 class="league-title">
                                    <A href=event_href>{display}</A>
                                </h3>
                            </div>
                        }
                    });
                    let rows = lg
                        .matches
                        .into_iter()
                        .map(|m| view! { <MatchRow m=m show_bo=show_bo push=push /> })
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

/// A click handler that toggles whether match `id`'s score is revealed in the
/// shared [`RevealedMatches`] set (and persists the set). Stops propagation +
/// prevents the default so the click doesn't fall through to an enclosing row
/// link (a no-op on a standalone reveal button). `None` `revealed` ⇒ inert.
fn reveal_toggler(
    revealed: Option<RwSignal<HashSet<i64>>>,
    id: i64,
    // `Copy` (it captures only an `RwSignal` + an `i64`, both `Copy`) so it can be
    // moved into a reactive view more than once, as the detail page does.
) -> impl Fn(leptos::ev::MouseEvent) + Copy {
    move |ev: leptos::ev::MouseEvent| {
        ev.prevent_default();
        ev.stop_propagation();
        if let Some(r) = revealed {
            r.update(|s| {
                if !s.insert(id) {
                    s.remove(&id);
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
fn venue_time_cell(
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

#[component]
fn MatchRow(m: MatchView, show_bo: bool, push: bool) -> impl IntoView {
    let status_class = m.status.row_class();
    let badge = m.status.badge();

    let sa = m.team_a.score;
    let sb = m.team_b.score;
    let has = sa.is_some() && sb.is_some();
    let win_a = m.team_a.winner;
    let win_b = m.team_b.winner;
    let sep = versus_sep(m.game);
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
    let toggle_reveal = reveal_toggler(revealed, mid);
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
    let inner = if m.game.single_entity() {
        view! {
            {time_view}
            <span class="row-team row-solo">{m.team_a.label}</span>
            {meta_view}
        }
        .into_any()
    } else {
        view! {
            {time_view}
            <span
                class="row-team row-a"
                class:winner=move || reveal.get() && win_a
                class:loser=move || reveal.get() && win_b
            >
                {m.team_a.label}
            </span>
            <span class="row-mid" class:scored=move || reveal.get() && has>
                {move || {
                    if reveal.get() && has {
                        format!("{} – {}", sa.unwrap_or(0), sb.unwrap_or(0))
                    } else {
                        sep.to_string()
                    }
                }}
            </span>
            <span
                class="row-team row-b"
                class:winner=move || reveal.get() && win_b
                class:loser=move || reveal.get() && win_a
            >
                {m.team_b.label}
            </span>
            {meta_view}
        }
        .into_any()
    };

    // The whole row links to the match detail page; `display: contents` lets its
    // children participate in the row grid alongside the ★ button. The score
    // reveal inside (`reveal-meta`) prevents this navigation when clicked.
    let body = view! {
        <a class="row-body" href=format!("/match/{}", m.id)>
            {inner}
        </a>
    };

    // The leading column is always reserved, so the layout never shifts when push
    // support (the ★) resolves after hydration — `push` only controls whether the
    // reminder ★ is shown, not the grid. The column holds the ★ (upcoming, push
    // enabled), the live/final side bar, or an empty placeholder.
    let lead = match m.status {
        MatchStatus::Live => view! { <span class="row-bar live"></span> }.into_any(),
        MatchStatus::Finished => view! { <span class="row-bar final"></span> }.into_any(),
        MatchStatus::Upcoming if push => {
            view! {
                <StarButton
                    id=m.id
                    game=m.game
                    league=m.league.clone()
                    team_a=m.team_a.name.clone()
                    team_b=m.team_b.name.clone()
                />
            }
                .into_any()
        }
        _ => view! { <span class="row-lead-empty"></span> }.into_any(),
    };
    view! {
        <div class=format!("row has-star {status_class}") id=format!("m-{}", m.id)>
            {lead}
            {body}
        </div>
    }
    .into_any()
}

#[component]
fn StarButton(id: i64, game: Game, league: String, team_a: String, team_b: String) -> impl IntoView {
    let starred = use_context::<RwSignal<HashSet<i64>>>().expect("starred context");
    let subscribed = use_context::<Subscribed>().expect("subscribed context").0;
    let excluded = use_context::<Excluded>().expect("excluded context").0;
    let vapid = use_context::<RwSignal<Option<String>>>().expect("vapid context");
    // The scopes that auto-cover this match (its game, this event, or either team).
    let keys = [
        format!("game|{}", game.slug()),
        format!("league|{league}"),
        format!("team|{team_a}"),
        format!("team|{team_b}"),
    ];
    let keys_click = keys.clone();
    // The ★ lights up if this match is starred individually *or* a subscription
    // covers it — unless it's been individually opted out (excluded) of that
    // coverage. So the star always reflects what you'll actually be notified of.
    let is_on = Memo::new(move |_| {
        !excluded.with(|e| e.contains(&id))
            && (starred.with(|s| s.contains(&id))
                || subscribed.with(|s| keys.iter().any(|k| s.contains(k))))
    });

    let on_click = move |_| {
        let covered = subscribed.with_untracked(|s| keys_click.iter().any(|k| s.contains(k)));
        let want_on = !is_on.get_untracked();
        if want_on {
            // Turn it back on: clear any opt-out, and star it unless a scope
            // already covers it.
            excluded.update(|e| {
                e.remove(&id);
            });
            if !covered {
                starred.update(|s| {
                    s.insert(id);
                });
            }
        } else {
            // Turn it off: drop an individual star, and — if a subscription still
            // covers it — opt this one match out without dropping the scope.
            starred.update(|s| {
                s.remove(&id);
            });
            if covered {
                excluded.update(|e| {
                    e.insert(id);
                });
            }
        }
        #[cfg(feature = "hydrate")]
        {
            save_starred(&starred.get_untracked().iter().copied().collect::<Vec<_>>());
            save_excluded(&excluded.get_untracked().iter().copied().collect::<Vec<_>>());
            sync_match_reminder(id, want_on, covered, vapid.get_untracked());
        }
        #[cfg(not(feature = "hydrate"))]
        {
            let _ = (covered, want_on, vapid);
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
            let _ = storage.set_item(keys::STARRED, &csv);
        }
    }
}

#[cfg(feature = "hydrate")]
fn load_starred() -> HashSet<i64> {
    load_id_set(keys::STARRED)
}

#[cfg(feature = "hydrate")]
fn save_excluded(ids: &[i64]) {
    if let Some(win) = web_sys::window() {
        if let Ok(Some(storage)) = win.local_storage() {
            let csv = ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
            let _ = storage.set_item(keys::EXCLUDED, &csv);
        }
    }
}

#[cfg(feature = "hydrate")]
fn load_excluded() -> HashSet<i64> {
    load_id_set(keys::EXCLUDED)
}

/// Persist the set of individually-revealed match ids, so a match the user has
/// already peeked at stays revealed across reloads.
#[cfg(feature = "hydrate")]
fn save_revealed(ids: &HashSet<i64>) {
    if let Some(win) = web_sys::window() {
        if let Ok(Some(storage)) = win.local_storage() {
            let csv = ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
            let _ = storage.set_item(keys::REVEALED, &csv);
        }
    }
}

#[cfg(feature = "hydrate")]
fn load_revealed() -> HashSet<i64> {
    load_id_set(keys::REVEALED)
}

/// Persist a set of strings under `key` (newline-separated).
#[cfg(feature = "hydrate")]
fn save_str_set(key: &str, set: &HashSet<String>) {
    if let Some(win) = web_sys::window() {
        if let Ok(Some(storage)) = win.local_storage() {
            let joined = set.iter().cloned().collect::<Vec<_>>().join("\n");
            let _ = storage.set_item(key, &joined);
        }
    }
}

#[cfg(feature = "hydrate")]
fn load_str_set(key: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    if let Some(win) = web_sys::window() {
        if let Ok(Some(storage)) = win.local_storage() {
            if let Ok(Some(v)) = storage.get_item(key) {
                for part in v.split('\n').filter(|p| !p.is_empty()) {
                    out.insert(part.to_string());
                }
            }
        }
    }
    out
}

/// Persist revealed standings/bracket sections (newline-separated string keys).
#[cfg(feature = "hydrate")]
fn save_sections(keys: &HashSet<String>) {
    if let Some(win) = web_sys::window() {
        if let Ok(Some(storage)) = win.local_storage() {
            let joined = keys.iter().cloned().collect::<Vec<_>>().join("\n");
            let _ = storage.set_item(keys::SECTIONS, &joined);
        }
    }
}

#[cfg(feature = "hydrate")]
fn load_sections() -> HashSet<String> {
    let mut out = HashSet::new();
    if let Some(win) = web_sys::window() {
        if let Ok(Some(storage)) = win.local_storage() {
            if let Ok(Some(v)) = storage.get_item(keys::SECTIONS) {
                for part in v.split('\n').filter(|p| !p.is_empty()) {
                    out.insert(part.to_string());
                }
            }
        }
    }
    out
}

/// Load a comma-separated set of match ids from local storage.
#[cfg(feature = "hydrate")]
fn load_id_set(key: &str) -> HashSet<i64> {
    let mut out = HashSet::new();
    if let Some(win) = web_sys::window() {
        if let Ok(Some(storage)) = win.local_storage() {
            if let Ok(Some(csv)) = storage.get_item(key) {
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
fn sync_match_reminder(match_id: i64, want_on: bool, covered: bool, vapid: Option<String>) {
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
        let push = crate::types::PushSub {
            endpoint: endpoint.clone(),
            p256dh,
            auth,
        };
        if want_on {
            // Arm it (and clear any exclusion tombstone server-side).
            let _ = crate::server::add_reminder(crate::types::ReminderReq {
                sub: push,
                match_id,
            })
            .await;
        } else if covered {
            // Opt this match out of its covering subscription, keeping the scope.
            let _ = crate::server::exclude_reminder(crate::types::ReminderReq {
                sub: push,
                match_id,
            })
            .await;
        } else {
            // Plain un-star: drop the reminder row entirely.
            let _ = crate::server::remove_reminder(endpoint, match_id).await;
        }
    });
}

/// Clear every subscription + pending reminder for this browser's push endpoint.
#[cfg(feature = "hydrate")]
fn clear_all_notifications(vapid: Option<String>) {
    let Some(vapid) = vapid else { return };
    leptos::task::spawn_local(async move {
        let sub = match pte_subscribe(&vapid).await {
            Ok(s) => s,
            Err(_) => return,
        };
        if let Some(endpoint) = reflect_str(&sub, "endpoint") {
            let _ = crate::server::clear_notifications(endpoint).await;
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
            serie_name: String::new(),
            tier: "S".into(),
            status: MatchStatus::Upcoming,
            clock_label: String::new(),
            date_label: String::new(),
            venue_label: String::new(),
            best_of: "Bo3".into(),
            team_a: TeamView {
                label: "A".into(),
                name: "A".into(),
                score: None,
                winner: false,
            },
            team_b: TeamView {
                label: "B".into(),
                name: "B".into(),
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
                        serie_name: String::new(),
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
        assert_eq!(compute_effective(&grid, &set, false), vec![vec![0, 0], vec![0]]);
        // Reveal both semis' scores → the final's lineup (stage 1) auto-appears.
        apply_stage(&mut set, "bn:1:0:0", "bs:1:0:0", 2);
        apply_stage(&mut set, "bn:1:0:1", "bs:1:0:1", 2);
        assert_eq!(compute_effective(&grid, &set, false), vec![vec![2, 2], vec![1]]);
    }

    #[test]
    fn bracket_series_click_reveals_contributing_matches_first() {
        let grid = semis_final_grid();
        let mut set = HashSet::new();
        // Clicking the final before its teams are known reveals the feeding semis'
        // scores; the final's lineup then auto-appears (names, no score yet).
        apply_bracket_op(&mut set, &grid, BkOp::Series(1, 0));
        assert_eq!(compute_effective(&grid, &set, false), vec![vec![2, 2], vec![1]]);
        // The next click reveals the final's own score.
        apply_bracket_op(&mut set, &grid, BkOp::Series(1, 0));
        assert_eq!(compute_effective(&grid, &set, false), vec![vec![2, 2], vec![2]]);
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
            vec![cell(0, 0, &[]), cell(0, 1, &[]), cell(0, 2, &[]), cell(0, 3, &[])],
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
        assert!(!set.contains("bs:1:0:0"), "the granular score reveal is cleared");
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
        assert!(!set.contains("bs:1:1:0"), "the ahead semifinal reveal is forgotten");
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
        assert_eq!(leagues_for_games(&s, &HashSet::new()), vec!["LCK", "LEC", "LPL"]);
        // `mv` matches are LoL, so the cs2 game filter hides them all.
        assert!(leagues_for_games(&s, &names(&["cs2"])).is_empty());
        assert_eq!(leagues_for_games(&s, &names(&["lol"])), vec!["LCK", "LEC", "LPL"]);
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
        // LoL game keeps both; cs2 drops everything (mv matches are LoL).
        assert_eq!(filter_schedule(s.clone(), &names(&["lol"]), &HashSet::new()).days.len(), 1);
        assert!(filter_schedule(s.clone(), &names(&["cs2"]), &HashSet::new()).days.is_empty());
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
        // Else infer from a known game slug in ?g.
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
        assert_eq!(query_param("mode=sports&g=nhl", "mode").as_deref(), Some("sports"));
        assert_eq!(query_param("mode=sports&g=nhl", "g").as_deref(), Some("nhl"));
        assert_eq!(query_param("g=cs2,lol", "g").as_deref(), Some("cs2,lol"));
        assert_eq!(query_param("mode=sports", "g"), None);
        assert_eq!(query_param("", "mode"), None);
    }
}
