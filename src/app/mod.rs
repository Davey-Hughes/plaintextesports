use crate::bracket;
use crate::server::{
    get_day, get_event_schedule, get_event_stages, get_event_stages_by_league, get_f1_results,
    get_f1_standings, get_match_detail, get_notifications, get_range, get_schedule, get_site,
    get_team_schedule,
};
use crate::types::{
    full_event_name, DayGroup, EventInfo, F1Result, F1Standings, Game, MatchDetail, MatchStatus,
    MatchView, ScheduleView, Series, SeriesGame, StreamView,
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

// Standings / Swiss grid / elimination bracket rendering + reveal machinery.
mod playoffs;
pub(crate) use playoffs::*;

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
    pub const ICONS: &str = "icons";
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
                // Apply the saved theme + icon preference before paint to avoid a
                // flash (icons default on; CSS hides logos when data-icons="0").
                <script>
                    r#"(function(){try{var t=localStorage.getItem('theme')||'dark';document.documentElement.setAttribute('data-theme',t);document.documentElement.setAttribute('data-icons',localStorage.getItem('icons')==='0'?'0':'1');}catch(e){document.documentElement.setAttribute('data-theme','dark');}})();"#
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

/// Set of match uids whose scores are individually revealed (by clicking the
/// match's "Final" badge). Keyed by uid (`"{game}-{id}"`) since a match id isn't
/// unique across games.
#[derive(Clone, Copy)]
struct RevealedMatches(RwSignal<HashSet<String>>);

/// Revealed standings/bracket-round sections, keyed by `"st:<tid>"` /
/// `"bk:<tid>:<round>"` so reveal state is per-round and shared by every page
/// that shows the same tournament's bracket.
#[derive(Clone, Copy)]
struct RevealedSections(RwSignal<HashSet<String>>);

/// Set of "kind|value" scopes the user is subscribed to (game/event reminders).
#[derive(Clone, Copy)]
struct Subscribed(RwSignal<HashSet<String>>);

/// Match uids the user has opted out of even though a subscription covers them
/// (a per-match "un-notify" that doesn't drop the whole subscription).
#[derive(Clone, Copy)]
struct Excluded(RwSignal<HashSet<String>>);

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
    let starred = RwSignal::new(HashSet::<String>::new());
    // Matches opted out of a covering subscription (per-match un-notify).
    let excluded = RwSignal::new(HashSet::<String>::new());
    let vapid = RwSignal::new(None::<String>);
    // Spoiler control: a global reveal + a per-match reveal set.
    let show_scores = RwSignal::new(false);
    // Shared so clicking any one game's time reveals every venue time at once.
    let show_venue = RwSignal::new(false);
    let revealed = RwSignal::new(HashSet::<String>::new());
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
                        <Route path=(StaticSegment("match"), ParamSegment("uid")) view=MatchDetailPage />
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
fn flash_match_row(uid: &str) -> bool {
    let Some(row) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(&format!("m-{uid}")))
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
            // Display preferences (icons / 24h / theme), grouped so they can sit at
            // the right of the bar on desktop and share the brand's row on mobile.
            <div class="toggle-prefs">
                <IconsToggle />
                <HourToggle />
                <ThemeToggle />
            </div>
            // On narrow screens this forces the controls below onto a second row,
            // leaving the prefs up beside the brand.
            <span class="toggles-break" aria-hidden="true"></span>
            // The back-to-top arrow takes the brand's spot once it scrolls away
            // (hidden at the top), then the schedule controls.
            <ScrollTopButton />
            <RefreshButton />
            <SportToggle />
            <CalendarPicker />
            <ScoresToggle />
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
                            "baseball + hockey + basketball + football + soccer + motorsport schedules"
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
    /// The match uid, `"{game}-{id}"` (see [`crate::types::match_uid`]).
    uid: String,
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

/// The vector logo for `name` found among a schedule's matches (it appears as
/// either side), or empty when none of its matches carry one.
fn team_logo_in_schedule(s: &ScheduleView, name: &str) -> String {
    for d in &s.days {
        for lg in &d.leagues {
            for m in &lg.matches {
                if m.team_a.name == name && !m.team_a.logo.is_empty() {
                    return m.team_a.logo.clone();
                }
                if m.team_b.name == name && !m.team_b.logo.is_empty() {
                    return m.team_b.logo.clone();
                }
            }
        }
    }
    String::new()
}

/// A team's vector (SVG) logo as a small `<img>`, or nothing when there's no
/// vector source for it (most leagues — only NHL/MLB expose one). `extra` is an
/// extra class for sizing per context.
fn team_logo(logo: &str, extra: &str) -> AnyView {
    if logo.is_empty() {
        return ().into_any();
    }
    view! {
        <img class=format!("team-logo {extra}") src=logo.to_string() alt="" loading="lazy" />
    }
    .into_any()
}

/// An F1 constructor's logo rendered as a `currentColor` CSS mask of F1's official
/// 2026 white silhouette, so it tracks the theme's foreground and stays visible on
/// every theme (the colour artwork is near-black for some teams). Nothing when the
/// constructor has no mapped logo. Keeps the `team-logo` class so the global icons
/// toggle hides it alongside the raster logos.
fn clogo_icon(logo: &str) -> AnyView {
    if logo.is_empty() {
        return ().into_any();
    }
    view! {
        <span class="team-logo f1-clogo" style=format!("--clogo:url('{logo}')")></span>
    }
    .into_any()
}

/// A team's name for a compact row: its label, with the official abbreviation as a
/// second copy that CSS swaps in where the row is too narrow for the full label
/// (see `.tname`/`.tabbr`). When the source has no abbreviation (esports, whose
/// label is already an acronym) it's just the label.
fn team_cell(label: String, abbrev: String) -> AnyView {
    if abbrev.is_empty() {
        view! { {label} }.into_any()
    } else {
        view! {
            <span class="tname">{label}</span>
            <span class="tabbr">{abbrev}</span>
        }
        .into_any()
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
                <SwissBracket rounds=swiss tournament_id game />
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
            <Bracket rounds=rounds tournament_id game bracket_only times=times.get_value() />
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
                        // Flags/logos are gated with the names — showing a flag while
                        // the driver is blank would leak who finished where.
                        let (driver, con, cabbr, detail, flag, clogo) = if show {
                            (row.driver, row.constructor, row.constructor_abbrev, row.detail, row.flag, row.constructor_logo)
                        } else {
                            (String::new(), String::new(), String::new(), String::new(), String::new(), String::new())
                        };
                        view! {
                            <li class="f1-row">
                                <span class="f1-pos">{row.pos}</span>
                                <span class="f1-driver">
                                    {team_logo(&flag, "f1-flag")}{driver}
                                </span>
                                <span class="f1-con">
                                    {clogo_icon(&clogo)}{team_cell(con, cabbr)}
                                </span>
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
                                    "hide results".to_string()
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
                let (name, con, cabbr, pts, flag, clogo) = if show {
                    (r.name, r.detail, r.constructor_abbrev, r.points, r.flag, r.constructor_logo)
                } else {
                    (String::new(), String::new(), String::new(), String::new(), String::new(), String::new())
                };
                view! {
                    <li class="f1-standing-row">
                        <span class="f1-pos">{r.pos}</span>
                        <span class="f1-standing-name">{team_logo(&flag, "f1-flag")}{name}</span>
                        <span class="f1-standing-con">{clogo_icon(&clogo)}{team_cell(con, cabbr)}</span>
                        <span class="f1-standing-pts">{pts}</span>
                    </li>
                }
            })
            .collect_view();
        let con = constructors
            .get_value()
            .into_iter()
            .map(|r| {
                let (name, cabbr, pts, clogo) = if show {
                    (r.name, r.constructor_abbrev, r.points, r.constructor_logo)
                } else {
                    (String::new(), String::new(), String::new(), String::new())
                };
                view! {
                    <li class="f1-standing-row f1-standing-row-con">
                        <span class="f1-pos">{r.pos}</span>
                        <span class="f1-standing-name">{clogo_icon(&clogo)}{team_cell(name, cabbr)}</span>
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
                        if revealed.get() { "hide standings".to_string() } else { "reveal standings".to_string() }
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
                        // event (all its sessions) — but only while a session is
                        // still to come; a wholly-finished GP can't be notified
                        // about, so it shows the bare title. Other event pages keep
                        // the bare title too — their schedule carries league ★s.
                        let title_head = if f1_sr.is_some() && event_has_upcoming(&s) {
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
                        // The team's vector logo (NHL/MLB), from any of its matches.
                        let logo = team_logo_in_schedule(&s, &name);
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
                                    {team_logo(&logo, "team-logo-lg")}
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
    let uid = move || params.get().ok().map(|p| p.uid).unwrap_or_default();
    let hour24 = use_context::<RwSignal<bool>>().expect("hour24 context");
    let tz = use_context::<RwSignal<String>>().expect("tz context");
    let detail = Resource::new(
        move || (uid(), tz.get(), hour24.get()),
        |(uid, tz, h)| async move { get_match_detail(uid, tz, h).await },
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
    // "Venue · City, Country" (whichever the source provides); empty for esports.
    let venue_line = [m.venue_name.trim(), m.venue_location.trim()]
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

    let muid = m.uid();
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
    let (logo_a, logo_b) = (m.team_a.logo, m.team_b.logo);

    // Per-match "remind me" ★ beside the title — the same control the schedule
    // rows carry, so a match can be subscribed to straight from its own page. Only
    // for an upcoming match (a past/live/finished one can't be reminded about) and
    // only once Web Push is available (read reactively, so it appears after the key
    // resolves). Captured here before the title view consumes the names.
    let star_upcoming = matches!(m.status, MatchStatus::Upcoming);
    let (star_id, star_game, star_league) = (m.id, m.game, m.league.clone());
    let (star_a, star_b) = (name_a.clone(), name_b.clone());
    let vapid_ctx = use_context::<RwSignal<Option<String>>>();
    let match_star = move || {
        let push = vapid_ctx.is_some_and(|v| v.with(|x| x.is_some()));
        (star_upcoming && push).then(|| {
            view! {
                <StarButton
                    id=star_id
                    game=star_game
                    league=star_league.clone()
                    team_a=star_a.clone()
                    team_b=star_b.clone()
                />
            }
        })
    };

    // Scores/standings/bracket are spoilers: reveal when the global toggle is on
    // or this match was individually revealed (persisted, shared with the list).
    let global = use_context::<ShowScores>().map(|s| s.0);
    let revealed = use_context::<RevealedMatches>().map(|r| r.0);
    let reveal = {
        let muid = muid.clone();
        Memo::new(move |_| {
            global.is_some_and(|g| g.get())
                || revealed.is_some_and(|r| r.with(|set| set.contains(&muid)))
        })
    };
    // Per-match reveal button (hidden when the global toggle already shows all).
    let toggle = reveal_toggler(revealed, muid);
    // Hide the reveal toggle when the global toggle already shows all, or when
    // the match hasn't been played yet (nothing to reveal).
    let toggle_hidden = move || global.is_some_and(|g| g.get()) || !played;

    view! {
        <article class="detail">
            <A href="/">"← schedule"</A>
            <div class="detail-title-row">
                <h1 class="detail-title match-title" class:detail-title-solo=is_solo>
                {if is_solo {
                    view! {
                        <span class="detail-team detail-solo">
                            {team_logo(&logo_a, "team-logo-lg")}
                            {team_a}
                        </span>
                    }
                    .into_any()
                } else {
                    view! {
                        <span
                            class="detail-team"
                            class:winner=move || reveal.get() && win_a
                            class:loser=move || reveal.get() && win_b
                        >
                            {team_logo(&logo_a, "team-logo-lg")}
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
                            {team_logo(&logo_b, "team-logo-lg")}
                        </span>
                    }
                    .into_any()
                }}
                </h1>
                {match_star}
            </div>
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
                    let toggle = toggle.clone();
                    (!toggle_hidden())
                        .then(move || {
                            view! {
                                <button class="toggle" on:click=toggle>
                                    {move || if reveal.get() { "hide scores" } else { "show scores" }}
                                </button>
                            }
                        })
                }}
            </div>
            {(!venue_line.is_empty())
                .then(|| view! { <div class="detail-venue">{venue_line}</div> })}
            <StreamsList streams=streams />
            // MLB series between the two teams. Each game row reveals on its own
            // (shared with the schedule); the record line waits until every played
            // game is revealed, so it never leaks the leader ahead of the scores.
            {(!series.games.is_empty())
                .then(|| {
                    // Series games are MLB; key by uid like the per-match set.
                    let played_ids: Vec<String> = series
                        .games
                        .iter()
                        .filter(|g| {
                            matches!(g.status, MatchStatus::Live | MatchStatus::Finished)
                                && g.score_a.is_some()
                                && g.score_b.is_some()
                        })
                        .map(|g| crate::types::match_uid(Game::Mlb, g.match_id))
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
    // shared with the schedule via the per-match set keyed on its uid. Series
    // games are always MLB.
    let muid = crate::types::match_uid(Game::Mlb, match_id);
    let global = use_context::<ShowScores>().map(|s| s.0);
    let revealed = use_context::<RevealedMatches>().map(|r| r.0);
    let reveal = {
        let muid = muid.clone();
        Memo::new(move |_| {
            global.is_some_and(|g| g.get())
                || revealed.is_some_and(|r| r.with(|set| set.contains(&muid)))
        })
    };
    let toggle_reveal = reveal_toggler(revealed, muid.clone());
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
        view! { <a class="row-body" href=format!("/match/{muid}")>{inner}</a> }.into_any()
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

/// Global on/off for team / driver / constructor logos. Default on; the actual
/// hiding is CSS keyed on `data-icons` on <html> (set pre-paint in the shell to
/// avoid a flash), so this only tracks state for the button's brightness and
/// flips the attribute + saved preference. Mirrors [`ThemeToggle`].
#[component]
fn IconsToggle() -> impl IntoView {
    let show = RwSignal::new(true);

    Effect::new(move |_| {
        #[cfg(feature = "hydrate")]
        show.set(saved_icons());
    });

    let toggle = move |_| {
        let next = !show.get_untracked();
        show.set(next);
        #[cfg(feature = "hydrate")]
        apply_icons(next);
    };

    view! {
        <button
            class="toggle state-toggle"
            class:on=move || show.get()
            title=move || if show.get() { "Hide team icons" } else { "Show team icons" }
            on:click=toggle
        >
            "icons"
        </button>
    }
}

/// The saved icon preference: on unless explicitly stored off.
#[cfg(feature = "hydrate")]
fn saved_icons() -> bool {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item(keys::ICONS).ok().flatten())
        != Some("0".to_string())
}

/// Apply the icon preference: set `data-icons` on <html> and persist it.
#[cfg(feature = "hydrate")]
fn apply_icons(on: bool) {
    let val = if on { "1" } else { "0" };
    if let Some(win) = web_sys::window() {
        if let Some(root) = win.document().and_then(|d| d.document_element()) {
            let _ = root.set_attribute("data-icons", val);
        }
        if let Ok(Some(storage)) = win.local_storage() {
            let _ = storage.set_item(keys::ICONS, val);
        }
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
        // Fixed "scores" label; it brightens (the `on` class) once scores are
        // revealed, which is more compact than a "show"/"hide" prefix. The action
        // it'll perform is spelled out in the hover tooltip.
        <button
            class="toggle state-toggle"
            class:on=move || show.get()
            title=move || if show.get() { "Hide scores" } else { "Show scores" }
            on:click=toggle
        >
            "scores"
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
    let starred = use_context::<RwSignal<HashSet<String>>>().expect("starred context");
    let excluded = use_context::<Excluded>().expect("excluded context").0;
    let vapid = use_context::<RwSignal<Option<String>>>().expect("vapid context");
    let hour24 = use_context::<RwSignal<bool>>().expect("hour24 context");
    let tz = use_context::<RwSignal<String>>().expect("tz context");

    // Resolve the starred ids to upcoming match views (re-fetches as you star or
    // remove); started/finished ids are dropped server-side.
    let starred_matches = Resource::new(
        move || {
            let mut uids: Vec<String> = starred.get().into_iter().collect();
            uids.sort_unstable();
            (uids, tz.get(), hour24.get())
        },
        |(uids, z, h)| async move {
            if uids.is_empty() {
                Ok(Vec::new())
            } else {
                get_notifications(uids, z, h).await
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
            save_subs(&empty);
            save_starred(&HashSet::new());
            save_excluded(&HashSet::new());
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
    let starred = use_context::<RwSignal<HashSet<String>>>().expect("starred context");
    let excluded = use_context::<Excluded>().expect("excluded context").0;
    let subscribed = use_context::<Subscribed>().expect("subscribed context").0;
    let vapid = use_context::<RwSignal<Option<String>>>().expect("vapid context");
    let id = m.id;
    let game = m.game;
    let uid = m.uid();
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
    let remove = {
        let uid = uid.clone();
        move |_| {
            let covered = subscribed.with_untracked(|s| keys.iter().any(|k| s.contains(k)));
            starred.update(|s| {
                s.remove(&uid);
            });
            if covered {
                excluded.update(|e| {
                    e.insert(uid.clone());
                });
            }
            #[cfg(feature = "hydrate")]
            {
                save_starred(&starred.get_untracked());
                save_excluded(&excluded.get_untracked());
                sync_match_reminder(id, game, false, covered, vapid.get_untracked());
            }
            #[cfg(not(feature = "hydrate"))]
            {
                let _ = (covered, vapid, game, id);
            }
        }
    };

    view! {
        <li class="notif-item">
            <A href=format!("/match/{uid}")>
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

/// Whether an event still has something to be notified about — any upcoming or
/// in-progress session. A fully-finished event (e.g. a past Grand Prix) returns
/// false, so its "notify me" ★ can be hidden: a reminder there could never fire.
fn event_has_upcoming(s: &ScheduleView) -> bool {
    s.days
        .iter()
        .flat_map(|d| &d.leagues)
        .flat_map(|lg| &lg.matches)
        .any(|m| matches!(m.status, MatchStatus::Upcoming | MatchStatus::Live))
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
    match game {
        // The American team sports read "away at home"; soccer (its World Cup
        // games are at neutral venues, with no home side) and esports read "vs".
        Game::Mlb | Game::Nhl | Game::Nba | Game::Nfl => "at",
        _ => "vs",
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
    let traditional = use_context::<SportMode>().expect("sport mode context").0;
    // In traditional mode, the league whose standings to show beneath the schedule
    // — but only when the current filters narrow the view to exactly one league
    // (its tab, or a single soccer chip), mirroring the esports single-event
    // bracket. Empty when ambiguous (several leagues) or in esports mode.
    let trad_standings_league = Memo::new(move |_| {
        if !traditional.get() {
            return String::new();
        }
        let Some(Ok(s)) = resource.get() else {
            return String::new();
        };
        let available = leagues_for_games(&s, &games.get());
        let selected = leagues.get();
        let effective: Vec<String> = if selected.is_empty() {
            available
        } else {
            available.into_iter().filter(|l| selected.contains(l)).collect()
        };
        match effective.as_slice() {
            [only] => only.clone(),
            _ => String::new(),
        }
    });
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
                                    (games_set.is_empty() || games_set.contains(m.game.slug()))
                                        && m.game.has_sub_leagues()
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
    }
}

/// The division/conference/group standings for a single traditional league,
/// shown beneath the home-page schedule once the filters narrow to one league
/// (the team-sport analogue of the esports single-event bracket). Empty league =
/// nothing rendered.
#[component]
fn TraditionalStandings(league: Memo<String>) -> impl IntoView {
    let stages = Resource::new(
        move || league.get(),
        |lg| async move {
            if lg.is_empty() {
                Ok(Vec::new())
            } else {
                get_event_stages(lg).await
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
            let muid = crate::types::match_uid(m.game, m.id);
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
                    href=format!("/match/{muid}")
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

/// A click handler that toggles whether match `uid`'s score is revealed in the
/// shared [`RevealedMatches`] set (and persists the set). Stops propagation +
/// prevents the default so the click doesn't fall through to an enclosing row
/// link (a no-op on a standalone reveal button). `None` `revealed` ⇒ inert.
fn reveal_toggler(
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
    let muid = crate::types::match_uid(m.game, m.id);
    let global = use_context::<ShowScores>().map(|s| s.0);
    let revealed = use_context::<RevealedMatches>().map(|r| r.0);
    let reveal = {
        let muid = muid.clone();
        Memo::new(move |_| {
            global.is_some_and(|g| g.get())
                || revealed.is_some_and(|r| r.with(|set| set.contains(&muid)))
        })
    };

    // A match's score lives behind its status meta: click the whole "Final · Bo3"
    // (or "LIVE") cluster to toggle just this row's score. (Plain meta for
    // score-less rows, e.g. upcoming or a live match still at the 0-0 placeholder.)
    let toggle_reveal = reveal_toggler(revealed, muid.clone());
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
            <span class="row-team row-solo">{team_cell(m.team_a.label, m.team_a.abbrev)}</span>
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
                {team_cell(m.team_a.label, m.team_a.abbrev)}
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
                {team_cell(m.team_b.label, m.team_b.abbrev)}
            </span>
            {meta_view}
        }
        .into_any()
    };

    // The whole row links to the match detail page; `display: contents` lets its
    // children participate in the row grid alongside the ★ button. The score
    // reveal inside (`reveal-meta`) prevents this navigation when clicked.
    let body = view! {
        <a class="row-body" href=format!("/match/{muid}")>
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
        <div class=format!("row has-star {status_class}") id=format!("m-{muid}")>
            {lead}
            {body}
        </div>
    }
    .into_any()
}

#[component]
fn StarButton(id: i64, game: Game, league: String, team_a: String, team_b: String) -> impl IntoView {
    let starred = use_context::<RwSignal<HashSet<String>>>().expect("starred context");
    let subscribed = use_context::<Subscribed>().expect("subscribed context").0;
    let excluded = use_context::<Excluded>().expect("excluded context").0;
    let vapid = use_context::<RwSignal<Option<String>>>().expect("vapid context");
    // The match's uid keys the star/exclude sets (ids aren't unique across games).
    let uid = crate::types::match_uid(game, id);
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
    let is_on = {
        let uid = uid.clone();
        Memo::new(move |_| {
            !excluded.with(|e| e.contains(&uid))
                && (starred.with(|s| s.contains(&uid))
                    || subscribed.with(|s| keys.iter().any(|k| s.contains(k))))
        })
    };

    let on_click = move |_| {
        let covered = subscribed.with_untracked(|s| keys_click.iter().any(|k| s.contains(k)));
        let want_on = !is_on.get_untracked();
        if want_on {
            // Turn it back on: clear any opt-out, and star it unless a scope
            // already covers it.
            excluded.update(|e| {
                e.remove(&uid);
            });
            if !covered {
                starred.update(|s| {
                    s.insert(uid.clone());
                });
            }
        } else {
            // Turn it off: drop an individual star, and — if a subscription still
            // covers it — opt this one match out without dropping the scope.
            starred.update(|s| {
                s.remove(&uid);
            });
            if covered {
                excluded.update(|e| {
                    e.insert(uid.clone());
                });
            }
        }
        #[cfg(feature = "hydrate")]
        {
            save_starred(&starred.get_untracked());
            save_excluded(&excluded.get_untracked());
            sync_match_reminder(id, game, want_on, covered, vapid.get_untracked());
        }
        #[cfg(not(feature = "hydrate"))]
        {
            let _ = (covered, want_on, vapid, game);
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

// Starred / excluded / revealed are sets of match uids ("{game}-{id}"), persisted
// as comma-separated strings via the shared string-set helpers below.
#[cfg(feature = "hydrate")]
fn save_starred(uids: &HashSet<String>) {
    save_str_set(keys::STARRED, uids);
}

#[cfg(feature = "hydrate")]
fn load_starred() -> HashSet<String> {
    load_str_set(keys::STARRED)
}

#[cfg(feature = "hydrate")]
fn save_excluded(uids: &HashSet<String>) {
    save_str_set(keys::EXCLUDED, uids);
}

#[cfg(feature = "hydrate")]
fn load_excluded() -> HashSet<String> {
    load_str_set(keys::EXCLUDED)
}

/// Persist the set of individually-revealed match uids, so a match the user has
/// already peeked at stays revealed across reloads.
#[cfg(feature = "hydrate")]
fn save_revealed(uids: &HashSet<String>) {
    save_str_set(keys::REVEALED, uids);
}

#[cfg(feature = "hydrate")]
fn load_revealed() -> HashSet<String> {
    load_str_set(keys::REVEALED)
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

/// Persist the starred set and (un)register the reminder on the server. The
/// server derives the notification details from `match_id`, so we send just that.
#[cfg(feature = "hydrate")]
fn sync_match_reminder(
    match_id: i64,
    game: Game,
    want_on: bool,
    covered: bool,
    vapid: Option<String>,
) {
    let Some(vapid) = vapid else { return };
    let game = game.slug().to_string();
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
                game,
            })
            .await;
        } else if covered {
            // Opt this match out of its covering subscription, keeping the scope.
            let _ = crate::server::exclude_reminder(crate::types::ReminderReq {
                sub: push,
                match_id,
                game,
            })
            .await;
        } else {
            // Plain un-star: drop the reminder row entirely.
            let _ = crate::server::remove_reminder(endpoint, match_id, game).await;
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
    use crate::types::{BracketMatch, DayGroup, Game, LeagueGroup, TeamView};

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
