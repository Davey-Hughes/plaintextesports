//! The home (schedule) page and single-day page, plus the schedule-window
//! math (earlier/later/traditional bounds) and schedule-shape predicates.
use crate::app::*;

#[component]
pub(crate) fn HomePage() -> impl IntoView {
    // Sport/event filters are multi-select and applied client-side (the server
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
        move || {
            (
                range.get(),
                earlier.get(),
                later.get(),
                tz.get(),
                hour24.get(),
                traditional.get(),
            )
        },
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
pub(crate) fn earlier_window(days_back: i64) -> (String, String) {
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
pub(crate) fn trad_window(earlier: i64, later: i64) -> (String, String) {
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
pub(crate) fn trad_forward(later: i64) -> i64 {
    (crate::types::TRAD_FORWARD_DAYS + later).min(crate::types::TRAD_FORWARD_MAX)
}

/// Inclusive `[start, end]` ISO day-keys of the traditional window relative to
/// `today` (an ISO day-key): `earlier` days back through `trad_forward(later)`
/// days forward. Pure (built on [`iso_add_days`]) so it's identical on SSR and
/// hydrate — the event page filters its days by this without a browser clock.
pub(crate) fn trad_day_bounds(today: &str, earlier: i64, later: i64) -> (String, String) {
    (
        iso_add_days(today, -earlier),
        iso_add_days(today, trad_forward(later)),
    )
}

/// Whether an event/team page should cap its forward window. Only MLB needs it:
/// its event page (`/event/MLB`) is the whole season, so it windows like the
/// homepage. An F1 event is a single Grand Prix weekend and esports events are
/// finite, so both show in full.
pub(crate) fn schedule_needs_window(s: &ScheduleView) -> bool {
    s.days
        .iter()
        .flat_map(|d| &d.leagues)
        .flat_map(|lg| &lg.matches)
        // The team sports play often, so an event schedule of them caps its
        // horizon like the homepage; F1 (single-entity) shows its whole GP.
        .any(|m| {
            matches!(
                m.sport,
                Sport::Mlb | Sport::Nhl | Sport::Nba | Sport::Nfl | Sport::Soccer
            )
        })
}

/// `(season, round)` for an F1 event's schedule, recovered from a session id
/// (`season * 100000 + round * 100 + session`, see `f1.rs`). `None` for non-F1
/// — including the other motorsport series (WRC/WEC), which share the sport but
/// use a different id scheme and the league name to distinguish themselves.
pub(crate) fn f1_season_round(s: &ScheduleView) -> Option<(i64, i64)> {
    let id = s
        .days
        .iter()
        .flat_map(|d| &d.leagues)
        .flat_map(|lg| &lg.matches)
        .find(|m| m.sport == Sport::Motorsport && m.league == "F1")?
        .id;
    Some((id / 100_000, (id / 100) % 1000))
}

/// The motorsport series ("WRC"/"WEC"/"MotoGP") an event page is for, else "". F1
/// is excluded — it has its own results+standings path keyed on session ids.
pub(crate) fn motor_series(s: &ScheduleView) -> String {
    s.days
        .iter()
        .flat_map(|d| &d.leagues)
        .map(|lg| lg.league.clone())
        .find(|l| matches!(l.as_str(), "WRC" | "WEC" | "MotoGP"))
        .unwrap_or_default()
}

/// The motorsport series ("F1"/"WRC"/"WEC"/"MotoGP") an event page is for, else "".
pub(crate) fn motorsport_league(s: &ScheduleView) -> String {
    s.days
        .iter()
        .flat_map(|d| &d.leagues)
        .map(|lg| lg.league.clone())
        .find(|l| matches!(l.as_str(), "F1" | "WRC" | "WEC" | "MotoGP"))
        .unwrap_or_default()
}

/// The official live-streaming service(s) for a motorsport series — `(name, url)`
/// each — shown as "watch" links on its event page. There's no per-event broadcast
/// data anywhere, so these are the stable per-series services (WEC's is free on
/// YouTube; the others paid, linked for convenience). Empty for non-motorsport.
pub(crate) fn motorsport_watch(league: &str) -> &'static [(&'static str, &'static str)] {
    match league {
        // From 2026 Apple TV is F1's exclusive US broadcaster (so F1 TV is blacked
        // out in the US); F1 TV still covers the rest of the world.
        "F1" => &[
            (
                "Apple TV (US)",
                "https://tv.apple.com/us/channel/formula-1/tvs.sbd.241000",
            ),
            ("F1 TV (outside US)", "https://f1tv.formula1.com/"),
        ],
        "WRC" => &[("Rally.TV", "https://www.rally.tv/en")],
        "WEC" => &[(
            "FIA WEC on YouTube",
            "https://www.youtube.com/@FIAWEC/streams",
        )],
        "MotoGP" => &[("MotoGP VideoPass", "https://videopass.motogp.com/")],
        _ => &[],
    }
}

/// Whether an event still has an upcoming (not-yet-started) session — the only
/// thing a "notify me" ★ can fire on. A wholly-finished event (a past Grand Prix)
/// or one whose only unfinished sessions are already live returns false, so its
/// header ★ is hidden: a reminder there could never fire. Mirrors the per-match
/// star rule (a past/live/finished match can't be reminded about).
pub(crate) fn event_has_upcoming(s: &ScheduleView) -> bool {
    s.days
        .iter()
        .flat_map(|d| &d.leagues)
        .flat_map(|lg| &lg.matches)
        .any(|m| matches!(m.status, MatchStatus::Upcoming))
}

/// Whether a loaded schedule is a traditional sport (any match is, e.g. MLB) —
/// data-driven so it's the same on SSR and hydrate (unlike the `SportMode`
/// toggle, which only settles after hydration).
pub(crate) fn schedule_is_traditional(s: &ScheduleView) -> bool {
    s.days
        .iter()
        .flat_map(|d| &d.leagues)
        .flat_map(|lg| &lg.matches)
        .any(|m| m.sport.traditional())
}

#[derive(Params, PartialEq, Clone)]
pub(crate) struct DayParams {
    date: String,
}

#[component]
pub(crate) fn DayPage() -> impl IntoView {
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
