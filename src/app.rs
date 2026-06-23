use crate::server::{
    get_day, get_event_info, get_event_schedule, get_event_stages, get_match_detail, get_range,
    get_schedule, get_site,
};
use crate::types::{
    full_event_name, BracketMatch, BracketRound, DayGroup, EventInfo, Game, MatchDetail,
    MatchStatus, MatchView, ScheduleView, StandingRow, StreamView,
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

/// Revealed standings/bracket-round sections, keyed by `"st:<tid>"` /
/// `"bk:<tid>:<round>"` so reveal state is per-round and shared by every page
/// that shows the same tournament's bracket.
#[derive(Clone, Copy)]
struct RevealedSections(RwSignal<HashSet<String>>);

/// Set of "kind|value" scopes the user is subscribed to (game/event reminders).
#[derive(Clone, Copy)]
struct Subscribed(RwSignal<HashSet<String>>);

/// Calendar-selected date range (start, end ISO dates); `None` = default view.
#[derive(Clone, Copy)]
struct DateRange(RwSignal<Option<(String, String)>>);

/// How many extra past days the "‹ show earlier days" control has revealed.
#[derive(Clone, Copy)]
struct EarlierDays(RwSignal<i64>);

/// Selected game filters (slugs) and event filters (league names) — shared and
/// persisted so they survive navigating to a match and back.
#[derive(Clone, Copy)]
struct Games(RwSignal<HashSet<String>>);
#[derive(Clone, Copy)]
struct Leagues(RwSignal<HashSet<String>>);

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
    let sections = RwSignal::new(HashSet::<String>::new());
    let subscribed = RwSignal::new(HashSet::<String>::new());
    // History views: a calendar-selected range, and the "earlier days" expansion.
    let range = RwSignal::new(None::<(String, String)>);
    let earlier = RwSignal::new(0i64);
    // Schedule filters (shared + persisted across navigation).
    let games = RwSignal::new(HashSet::<String>::new());
    let leagues = RwSignal::new(HashSet::<String>::new());
    provide_context(hour24);
    provide_context(tz);
    provide_context(starred);
    provide_context(vapid);
    provide_context(ShowScores(show_scores));
    provide_context(RevealedMatches(revealed));
    provide_context(RevealedSections(sections));
    provide_context(Subscribed(subscribed));
    provide_context(DateRange(range));
    provide_context(EarlierDays(earlier));
    provide_context(Games(games));
    provide_context(Leagues(leagues));

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
            revealed.set(load_revealed());
            sections.set(load_sections());
            range.set(load_range());
            games.set(load_str_set("games"));
            leagues.set(load_str_set("leagues"));
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
                        <Route path=(StaticSegment("event"), ParamSegment("league")) view=EventPage />
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
    // The brand scrolls away with the page; the toggle bar is a sibling of the
    // page so it can stick to the top while scrolling.
    view! {
        <header class="header">
            <div class="brand">
                <A href="/">"plaintextesports"</A>
            </div>
        </header>
        <div class="toggles">
            <CalendarPicker />
            <ScoresToggle />
            <HourToggle />
            <ThemeToggle />
        </div>
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

            <h2>"filtering the schedule"</h2>
            <p>
                "By default you see every tier-1 match. The filters are additive — pick "
                "any number of them and the schedule narrows to what you chose:"
            </p>
            <ul>
                <li>
                    "Click a game tab (" <span class="kbd">"CS2"</span> " / "
                    <span class="kbd">"LoL"</span> ") to include that game."
                </li>
                <li>
                    "Click an event chip (" <span class="kbd">"IEM"</span> ", "
                    <span class="kbd">"LCK"</span> ", …) to include that event. The chips "
                    "shown follow the games you've picked."
                </li>
                <li>
                    "When any filter is active a blue " <span class="kbd">"clear"</span>
                    " appears on the right — click it to reset everything."
                </li>
            </ul>

            <h2>"browsing other days"</h2>
            <p>
                "The schedule leads with today and what's upcoming. To look back, use "
                <span class="kbd">"‹ show earlier days"</span>
                " above the first day to pull in recent days a few at a time. For an exact "
                "window, open the calendar button at the top and pick a start and end date."
            </p>

            <h2>"event pages"</h2>
            <p>
                "Click an event's name (e.g. " <span class="kbd">"IEM"</span>
                ") to open its page: the full run of that event's matches, its group-stage "
                "standings and playoff bracket, and a link out to its Liquipedia/official "
                "page."
            </p>

            <h2>"seeing scores"</h2>
            <p>
                "Scores are hidden by default so you can browse finished matches without "
                "spoilers. To reveal them:"
            </p>
            <ul>
                <li>
                    "Click a match's " <span class="kbd">"Final"</span> " (or "
                    <span class="kbd">"LIVE"</span>
                    ") label to reveal just that match's score (click again to hide it)."
                </li>
                <li>
                    "Use the " <span class="kbd">"show scores"</span>
                    " toggle at the top to reveal every score, standing, and bracket at once."
                </li>
            </ul>

            <h2>"standings & brackets"</h2>
            <p>
                "On an event page (or a match's detail page) the standings and bracket are "
                "hidden too, and reveal in stages so you control how much you see:"
            </p>
            <ul>
                <li>
                    "Click the " <span class="kbd">"Standings"</span>
                    " heading to show the table. Until then it holds blank rows, so the page "
                    "doesn't jump when you reveal it."
                </li>
                <li>
                    "Click the " <span class="kbd">"Bracket"</span>
                    " heading to walk the whole bracket forward one step at a time — team "
                    "names first, then scores, round by round."
                </li>
                <li>
                    "Or reveal a single round by its title (e.g. "
                    <span class="kbd">"Quarterfinals"</span>
                    "), or a single series by clicking it. The first click shows the team "
                    "names, the next shows the score."
                </li>
            </ul>
            <p>
                "A later round's teams only appear once the matches feeding it have had "
                "their scores revealed — so you can't accidentally spoil who advanced. What "
                "you've revealed is remembered and shared everywhere the same event appears."
            </p>

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

#[derive(Params, PartialEq, Clone)]
struct EventParams {
    league: String,
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
    setup_autorefresh(schedule);
    let push = use_context::<RwSignal<Option<String>>>().is_some_and(|v| v.get().is_some());

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
                    Some(Ok(s)) => {
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
                        // One section per stage: a Swiss/group stage shows its
                        // standings, the playoffs its bracket — each labelled.
                        let extra = stage_list
                            .into_iter()
                            .map(|e| {
                                let EventInfo { tournament_id, stage, game, standings, rounds, .. } = e;
                                let bracket_only = standings.is_empty();
                                let label = (!stage.is_empty())
                                    .then(|| view! { <h2 class="stage-head">{stage}</h2> });
                                view! {
                                    <div class="event-extra">
                                        {label}
                                        <StandingsTable rows=standings tournament_id game />
                                        <Bracket rounds=rounds tournament_id bracket_only />
                                    </div>
                                }
                            })
                            .collect_view();
                        view! {
                            <article class="detail">
                                <A href="/">"← schedule"</A>
                                <h1 class="detail-title">{title}</h1>
                                {link}
                                {render_schedule(s, false, push, true)}
                                {extra}
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
    let status_label = match m.status {
        MatchStatus::Live => "LIVE",
        MatchStatus::Finished => "Final",
        MatchStatus::Canceled => "Canceled",
        MatchStatus::Upcoming => "",
    };
    let meta = [
        full_event_name(&m.league, &m.serie_name),
        m.clock_label.clone(),
        m.best_of.clone(),
        status_label.to_string(),
    ]
    .into_iter()
    .filter(|s| !s.is_empty())
    .collect::<Vec<_>>()
    .join(" · ");

    let mid = m.id;
    let (sa, sb) = (m.team_a.score, m.team_b.score);
    let has_score = sa.is_some() && sb.is_some();
    let (win_a, win_b) = (m.team_a.winner, m.team_b.winner);
    let (team_a, team_b) = (m.team_a.label, m.team_b.label);
    let tid = event.tournament_id;
    let game = event.game;
    let standings = event.standings;
    let bracket_only = standings.is_empty();
    let rounds = event.rounds;

    // Scores/standings/bracket are spoilers: reveal when the global toggle is on
    // or this match was individually revealed (persisted, shared with the list).
    let global = use_context::<ShowScores>().map(|s| s.0);
    let revealed = use_context::<RevealedMatches>().map(|r| r.0);
    let reveal = Memo::new(move |_| {
        global.is_some_and(|g| g.get())
            || revealed.is_some_and(|r| r.with(|set| set.contains(&mid)))
    });
    // Per-match reveal button (hidden when the global toggle already shows all).
    let toggle = move |_| {
        if let Some(r) = revealed {
            r.update(|s| {
                if !s.insert(mid) {
                    s.remove(&mid);
                }
            });
            #[cfg(feature = "hydrate")]
            save_revealed(&r.get_untracked());
        }
    };
    let toggle_hidden = move || global.is_some_and(|g| g.get());

    view! {
        <article class="detail">
            <A href="/">"← schedule"</A>
            <h1 class="detail-title">
                <span
                    class="detail-team"
                    class:winner=move || reveal.get() && win_a
                    class:loser=move || reveal.get() && win_b
                >
                    {team_a}
                </span>
                <span class="detail-score">
                    {move || {
                        if reveal.get() && has_score {
                            format!("{} – {}", sa.unwrap_or(0), sb.unwrap_or(0))
                        } else {
                            "vs".to_string()
                        }
                    }}
                </span>
                <span
                    class="detail-team"
                    class:winner=move || reveal.get() && win_b
                    class:loser=move || reveal.get() && win_a
                >
                    {team_b}
                </span>
            </h1>
            <div class="detail-meta">
                <span>{meta}</span>
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
            // Standings + bracket self-gate per section (shared, persisted), so
            // they're always rendered with their own reveal controls.
            <StandingsTable rows=standings tournament_id=tid game=game />
            <Bracket rounds=rounds tournament_id=tid bracket_only=bracket_only />
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
    let items = streams
        .into_iter()
        .map(|s| {
            let (site, channel) = stream_parts(&s.url);
            let tags = stream_tags(&s);
            let cls = if s.official { "stream official" } else { "stream" };
            view! {
                <li class=cls>
                    <a href=s.url target="_blank" rel="noreferrer">
                        <span class="stream-site">{site}</span>
                        {(!channel.is_empty())
                            .then(|| view! { <span class="stream-chan">{format!("/{channel}")}</span> })}
                    </a>
                    {(!tags.is_empty()).then(|| view! { <span class="stream-tags">{tags}</span> })}
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
                stage_of(set, &c.bn, &c.bs)
                    .max(auto_stage(&eff, &c.feeders))
                    .max(c.floor)
                    .min(c.max)
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
                for i in 0..n {
                    if bk_advanceable(grid, &eff, r, i) {
                        advance(set, r, i);
                    }
                }
            } else {
                // Fully revealed → hide the whole round.
                for c in &grid[r] {
                    apply_stage(set, &c.bn, &c.bs, 0);
                }
            }
        }
        BkOp::Cascade => {
            let frontier =
                (0..grid.len()).find(|&r| (0..grid[r].len()).any(|i| bk_advanceable(grid, &eff, r, i)));
            match frontier {
                Some(r) => {
                    for i in 0..grid[r].len() {
                        if bk_advanceable(grid, &eff, r, i) {
                            advance(set, r, i);
                        }
                    }
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
    // The game/map record column: CS2 plays maps, LoL plays games.
    let record_label = match game {
        Game::Lol => "Games",
        Game::Cs2 => "Maps",
    };
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
                    (
                        r.team,
                        format!("{}-{}", r.wins, r.losses),
                        format!("{}-{}", r.game_wins, r.game_losses),
                    )
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
    sa: Option<i32>,
    sb: Option<i32>,
    winner: String,
    max: u8,
    mid: i64,
}

#[component]
fn Bracket(rounds: Vec<BracketRound>, tournament_id: i64, bracket_only: bool) -> impl IntoView {
    if rounds.is_empty() {
        return ().into_any();
    }
    let global = use_context::<ShowScores>().map(|s| s.0);
    let sections = use_context::<RevealedSections>().map(|s| s.0);
    let tid = tournament_id;

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

    let cols = render
        .into_iter()
        .enumerate()
        .map(|(r, (section, title, sres))| {
            let round_on = move || eff.with(|e| e.get(r).is_some_and(|row| row.iter().any(|&s| s >= 1)));
            let ms = sres
                .into_iter()
                .map(|s| {
                    let (i, max) = (s.i, s.max);
                    let (ta, tb) = (s.ta, s.tb);
                    let (sa, sb) = (s.sa, s.sb);
                    let winner = s.winner;
                    let mid = s.mid;
                    let match_title = if max == 0 { "Not decided yet" } else { "Reveal this match" };
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
                                        if let Some(row) = web_sys::window()
                                            .and_then(|w| w.document())
                                            .and_then(|d| {
                                                d.get_element_by_id(&format!("m-{mid}"))
                                            })
                                        {
                                            e.prevent_default();
                                            row.scroll_into_view();
                                            // Flash the row so it's easy to spot;
                                            // remove + reflow + re-add restarts the
                                            // animation on a repeat click.
                                            let cl = row.class_list();
                                            let _ = cl.remove_1("row-flash");
                                            let _ = row.client_width();
                                            let _ = cl.add_1("row-flash");
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
                        // The slice fills the column for connector alignment, but
                        // only the visible box is the click target (the tall slice
                        // around it isn't), so the hit area matches what's drawn.
                        <div class="bk-match" class:bk-locked=max == 0>
                            <div
                                class="bk-box"
                                title=match_title
                                on:click=move |_| do_op(BkOp::Series(r, i))
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
                                    let score_view = move |s: Option<i32>| {
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
            let col = view! {
                <div class="bk-round">
                    <div class="bk-round-title">
                        <button
                            class="bk-round-toggle"
                            class:on=round_on
                            title="Reveal this round"
                            on:click=move |_| do_op(BkOp::Round(r))
                        >
                            {title}
                        </button>
                    </div>
                    <div class="bk-matches">{ms}</div>
                </div>
            };
            (section, col.into_any())
        })
        .collect::<Vec<(String, AnyView)>>();

    // Group consecutive columns by their section so a double-elimination bracket
    // renders as a labelled upper bracket, then lower bracket, then grand final —
    // instead of one linear column run. A single-elim bracket has one "" group.
    let mut groups: Vec<(String, Vec<AnyView>)> = Vec::new();
    for (section, col) in cols {
        match groups.last_mut() {
            Some((s, items)) if *s == section => items.push(col),
            _ => groups.push((section, vec![col])),
        }
    }
    let single = groups.len() == 1 && groups[0].0.is_empty();
    let body = if single {
        let cols = groups.into_iter().next().map(|g| g.1).unwrap_or_default();
        view! { <div class="bracket">{cols}</div> }.into_any()
    } else {
        // Double-elimination: winner's bracket and loser's bracket stack on the
        // left as their own trees; the grand final sits to the right, centred
        // between them (like a printed double-elim bracket).
        let group_view = |section: &str, cols: Vec<AnyView>| {
            let label = match section {
                "upper" => "Winner's Bracket",
                "lower" => "Loser's Bracket",
                "final" => "Grand Final",
                _ => "",
            };
            view! {
                <div class="bk-bracket-group">
                    {(!label.is_empty())
                        .then(|| view! { <div class="bk-group-label">{label}</div> })}
                    <div class="bracket">{cols}</div>
                </div>
            }
        };
        let mut left: Vec<AnyView> = Vec::new();
        let mut grand: Option<AnyView> = None;
        for (section, cols) in groups {
            if section == "final" {
                grand = Some(group_view(&section, cols).into_any());
            } else {
                left.push(group_view(&section, cols).into_any());
            }
        }
        view! {
            <div class="bracket-de">
                <div class="bk-de-left">{left}</div>
                {grand}
            </div>
        }
        .into_any()
    };

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
    let info = Resource::new(
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
                        let EventInfo { tournament_id, game, standings, rounds, .. } = e;
                        let bracket_only = standings.is_empty();
                        view! {
                            <div class="event-extra">
                                <StandingsTable rows=standings tournament_id game />
                                <Bracket rounds=rounds tournament_id bracket_only />
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
        d.get_month() + 1,
        d.get_date()
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
            // Keep the handle so client-side route changes clear the timer instead
            // of leaking a new 60s refetch loop on every page mount.
            if let Ok(handle) =
                set_interval_with_handle(move || resource.refetch(), Duration::from_secs(60))
            {
                on_cleanup(move || handle.clear());
            }
        }
        let _ = resource;
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
    // Default = today + future (get_schedule). A calendar range, else an
    // "earlier days" expansion, switches to the range view.
    let schedule = Resource::new(
        move || (range.get(), earlier.get(), tz.get(), hour24.get()),
        |(r, e, z, h)| async move {
            match r {
                Some((start, end)) => get_range(start, end, "all".into(), z, h).await,
                None if e > 0 => {
                    let (start, end) = earlier_window(e);
                    get_range(start, end, "all".into(), z, h).await
                }
                None => get_schedule("all".into(), z, h).await,
            }
        },
    );
    setup_autorefresh(schedule);

    view! {
        <GameTabs games leagues />
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
    (d.get_full_year() as i32, d.get_month() + 1, d.get_date())
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
        // With no explicit selection, mirror the "‹ show earlier days" expansion:
        // highlight the revealed lookback span [today - earlier, today].
        let earlier_span: Option<(String, String)> = {
            #[cfg(feature = "hydrate")]
            {
                match (start.is_none() && end.is_none()).then(|| earlier.get()) {
                    Some(n) if n > 0 => Some((iso_from_today(-n), iso_from_today(0))),
                    _ => None,
                }
            }
            #[cfg(not(feature = "hydrate"))]
            {
                let _ = earlier;
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
                _ => earlier_span
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
    let schedule = Resource::new(
        move || (date(), tz.get(), hour24.get()),
        |(d, z, h)| async move { get_day(d, "all".into(), z, h).await },
    );
    setup_autorefresh(schedule);

    view! {
        <GameTabs games leagues />
        <ScheduleSection resource=schedule games leagues show_nav=true />
    }
}

#[component]
fn GameTabs(
    games: RwSignal<HashSet<String>>,
    leagues: RwSignal<HashSet<String>>,
) -> impl IntoView {
    // Filters are multi-select and additive: no selection means "all". Click a
    // game to include it; click again to drop it.
    let toggle = move |value: &'static str| {
        games.update(|g| {
            if !g.remove(value) {
                g.insert(value.to_string());
            }
        });
        #[cfg(feature = "hydrate")]
        save_str_set("games", &games.get_untracked());
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
            save_str_set("games", &games.get_untracked());
            save_str_set("leagues", &leagues.get_untracked());
        }
    };
    view! {
        <div class="tabs">
            <div class="tabs-list">
                {with_star("CS2", "cs2")}
                {with_star("LoL", "lol")}
            </div>
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
                        save_str_set("leagues", &selected.get_untracked());
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
    view! {
        <Suspense fallback=|| view! { <p class="loading">"loading…"</p> }>
            {move || {
                resource
                    .get()
                    .map(|res| match res {
                        Ok(s) => {
                            // Event chips reflect the selected games; the schedule
                            // is then narrowed by both games and events.
                            let available = leagues_for_games(&s, &games.get());
                            let filtered = filter_schedule(s, &games.get(), &leagues.get());
                            // Show ★ reminder buttons only when Web Push is configured.
                            let push = use_context::<RwSignal<Option<String>>>()
                                .is_some_and(|v| v.get().is_some());
                            // Only offer event chips when there's more than one.
                            let chips = (available.len() > 1)
                                .then(move || {
                                    view! { <LeagueChips leagues=available selected=leagues /> }
                                });
                            view! {
                                {chips}
                                {render_schedule(filtered, show_nav, push, false)}
                            }
                                .into_any()
                        }
                        Err(_) => {
                            view! { <p class="error">"Failed to load schedule."</p> }.into_any()
                        }
                    })
            }}
        </Suspense>
        // When exactly one event is selected, show its standings/bracket below.
        <EventSection leagues=leagues />
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
    let hidden = RwSignal::new(false);
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
            // The whole bar scrolls to the day; a match still links to its page
            // (stopping propagation so it doesn't also trigger the scroll).
            view! {
                <a
                    class="upnext-match"
                    href=format!("/match/{}", m.id)
                    on:click=|e: leptos::ev::MouseEvent| e.stop_propagation()
                >
                    <span class="upnext-time">{m.clock_label}</span>
                    <span class="upnext-team">{m.team_a.label}</span>
                    <span class="upnext-vs">"vs"</span>
                    <span class="upnext-team">{m.team_b.label}</span>
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
            let Some(el) = web_sys::window()
                .and_then(|w| w.document())
                .and_then(|d| d.get_element_by_id(&anchor))
            else {
                return;
            };
            let cb = Closure::<dyn FnMut(js_sys::Array)>::new(move |entries: js_sys::Array| {
                let first = entries.get(0).unchecked_into::<web_sys::IntersectionObserverEntry>();
                hidden.set(first.is_intersecting());
            });
            if let Ok(obs) = web_sys::IntersectionObserver::new(cb.as_ref().unchecked_ref()) {
                obs.observe(&el);
            }
            // The observer keeps firing as long as its callback is alive; leak it
            // for the page's lifetime (web-sys handles can't ride on_cleanup,
            // which requires Send + Sync).
            cb.forget();
        });
    }

    view! {
        <div
            class="upnext"
            class:upnext-hidden=move || hidden.get()
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

fn render_schedule(s: ScheduleView, show_nav: bool, push: bool, event_mode: bool) -> impl IntoView {
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
    // The "‹ show earlier days" control rides the first day's title line (homepage
    // only; the single-day view has prev/next nav, the event page its own list).
    let show_earlier = !show_nav && !event_mode;
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
                    let show_bo = event_mode || lg.bo.is_none();
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
                    let league_name = lg.league.clone();
                    // Title the group with the full event name (league + edition,
                    // e.g. "IEM Katowice"); the subscribe key stays the short
                    // league name, while the full name keys the event link.
                    let serie = lg.serie_name.clone();
                    let head = (!event_mode).then(move || {
                        let display = full_event_name(&league_name, &serie);
                        let header = match &bo_label {
                            Some(bo) => format!("{display} · {bo}"),
                            None => display.clone(),
                        };
                        // The full edition name keys its event page, so distinct
                        // editions get distinct URLs.
                        let event_href = format!("/event/{}", enc_segment(&display));
                        view! {
                            <div class="league-head">
                                <SubscribeStar kind="league" value=league_name.clone() />
                                <h3 class="league-title">
                                    <A href=event_href>{header}</A>
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
            // The first day carries the "show earlier days" control beside its date.
            let head = if idx == 0 && show_earlier {
                view! {
                    <div class="day-head">
                        <h2 class="day-title">{d.day_label}</h2>
                        <EarlierControl />
                    </div>
                }
                    .into_any()
            } else {
                view! { <h2 class="day-title">{d.day_label}</h2> }.into_any()
            };
            view! {
                <section class="day" id=format!("day-{}", d.day_key)>
                    {head}
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
        // With no days to host it inline, the control stands alone above the notice.
        {(show_earlier && !has_days).then(|| view! { <EarlierControl /> })}
        {day_sections}
        {empty.then(|| view! { <p class="empty">"No tier-1 matches in this window."</p> })}
        {upnext_day.map(|day| view! { <UpNextBar day=day /> })}
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
            #[cfg(feature = "hydrate")]
            save_revealed(&r.get_untracked());
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
                    "vs".to_string()
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
            <div class=format!("row has-star {status_class}") id=format!("m-{}", m.id)>
                {lead}
                {body}
            </div>
        }
        .into_any()
    } else {
        view! {
            <div class=format!("row {status_class}") id=format!("m-{}", m.id)>
                {body}
            </div>
        }
        .into_any()
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
    load_id_set("starred")
}

/// Persist the set of individually-revealed match ids, so a match the user has
/// already peeked at stays revealed across reloads.
#[cfg(feature = "hydrate")]
fn save_revealed(ids: &HashSet<i64>) {
    if let Some(win) = web_sys::window() {
        if let Ok(Some(storage)) = win.local_storage() {
            let csv = ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
            let _ = storage.set_item("revealed", &csv);
        }
    }
}

#[cfg(feature = "hydrate")]
fn load_revealed() -> HashSet<i64> {
    load_id_set("revealed")
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
            let _ = storage.set_item("sections", &joined);
        }
    }
}

#[cfg(feature = "hydrate")]
fn load_sections() -> HashSet<String> {
    let mut out = HashSet::new();
    if let Some(win) = web_sys::window() {
        if let Ok(Some(storage)) = win.local_storage() {
            if let Ok(Some(v)) = storage.get_item("sections") {
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
            serie_name: String::new(),
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
}
