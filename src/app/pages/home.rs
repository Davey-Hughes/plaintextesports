//! The home (schedule) page and single-day page, plus the schedule-window
//! math (earlier/later/traditional bounds) and schedule-shape predicates.
use crate::app::prelude::*;
use leptos::prelude::*;
use leptos_router::hooks::use_params;
use leptos_router::params::Params;

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
    // esports ("all") vs the traditional sports ("trad"), with the other mode's
    // schedule prefetched so the toggle swaps without a round trip. Traditional
    // sports cap the forward window and reveal more days with `LaterControl`.
    let schedule = schedule_with_prefetch(
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
        |(r, e, l, z, h, trad): HomeKey| async move {
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
        home_key_after_toggle,
    );
    setup_autorefresh(schedule, view_has_live, true);

    view! {
        <FilterTabs games leagues traditional_row=false />
        <FilterTabs games leagues traditional_row=true />
        <ScheduleSection resource=schedule games leagues show_nav=false />
    }
}

/// What the home schedule is keyed on: the calendar range, the earlier and later
/// expansions, the timezone, the 24h clock, and the sport mode.
type HomeKey = (Option<(String, String)>, i64, i64, String, bool, bool);

/// What the single-day schedule is keyed on: the date, the timezone, the 24h
/// clock, and the sport mode.
type DayKey = (String, String, bool, bool);

/// The home key the sport toggle switches to. `SportToggle` flips the mode and
/// resets the earlier/later expansions; the calendar range stays.
fn home_key_after_toggle(k: &HomeKey) -> HomeKey {
    let (range, _, _, tz, hour24, trad) = k;
    (range.clone(), 0, 0, tz.clone(), *hour24, !*trad)
}

/// The day key the sport toggle switches to: the same day in the other mode.
fn day_key_after_toggle(k: &DayKey) -> DayKey {
    let (date, tz, hour24, trad) = k;
    (date.clone(), tz.clone(), *hour24, !*trad)
}

/// A schedule fetched ahead for the sport mode the viewer is not in.
///
/// Each mode's schedule is a separate server fetch, and the sport toggle flips
/// the mode at once. Without this the page waited a round trip for the new
/// schedule, drawing the new mode's tabs over the old mode's matches meanwhile:
/// the chip row dropped out in sports mode and showed the sports leagues in
/// esports mode, then everything changed again when the data landed. That was a
/// visible delay and two layout shifts per toggle. A view taken from here
/// resolves the resource before the next paint, so the page changes once.
struct Prefetched<K> {
    /// The resource key the view was fetched for.
    key: K,
    /// When it was fetched, in ms since the epoch.
    at_ms: i64,
    view: ScheduleView,
}

/// How old a prefetched schedule gets before the next load renews it. The
/// page's own schedule refreshes every 60 s, or every 15 s while a game is live
/// (`setup_autorefresh`). Renewing after a load once the prefetch is this old
/// catches every 60 s refresh but only every fourth live one, so the prefetch is
/// never much older than the schedule on screen. That is why the toggle shows
/// it whatever its age: refetching it at once would redraw the page a second
/// time for data no staler than what the viewer was just reading.
const PREFETCH_RENEW_MS: i64 = 45_000;

/// Takes the prefetched view if it was fetched for `key`. It is taken rather
/// than read so that a refetch of the same key, the 60 s refresh or the header's
/// button, finds the slot empty and goes to the server.
fn take_prefetched<K: PartialEq>(
    slot: &mut Option<Prefetched<K>>,
    key: &K,
) -> Option<Prefetched<K>> {
    slot.take_if(|p| p.key == *key)
}

/// Whether `key` needs prefetching: the slot is empty, holds another key, or
/// is `PREFETCH_RENEW_MS` old.
#[cfg_attr(not(feature = "hydrate"), allow(dead_code))]
fn prefetch_due<K: PartialEq>(slot: Option<&Prefetched<K>>, key: &K, now_ms: i64) -> bool {
    !slot.is_some_and(|p| p.key == *key && now_ms - p.at_ms < PREFETCH_RENEW_MS)
}

/// Whether a prefetch for `fetched` is still worth keeping once it returns: the
/// key the page last loaded must still toggle to it. That is false once a toggle
/// has made `fetched` the page's own key, which would hand the page's next
/// refresh an older copy instead of a server fetch, and false once the key has
/// moved on (a timezone or clock change), where an older fetch landing late
/// would replace the newer prefetch.
#[cfg_attr(not(feature = "hydrate"), allow(dead_code))]
fn prefetch_still_wanted<K: PartialEq>(latest: &K, fetched: &K, after_toggle: fn(&K) -> K) -> bool {
    after_toggle(latest) == *fetched
}

/// A schedule resource that keeps the same schedule in the other sport mode
/// prefetched, so the sport toggle swaps to it without a round trip. See
/// [`Prefetched`].
///
/// `after_toggle` maps the resource's key to the key the toggle switches it to.
/// The slot is matched on that exact key, so if the two ever disagree the toggle
/// misses and fetches as it did before; it cannot show the wrong schedule.
fn schedule_with_prefetch<K, Fut>(
    key: impl Fn() -> K + Copy + Send + Sync + 'static,
    fetch: impl Fn(K) -> Fut + Copy + Send + Sync + 'static,
    after_toggle: fn(&K) -> K,
) -> Resource<Result<ScheduleView, ServerFnError>>
where
    K: Clone + PartialEq + Send + Sync + 'static,
    Fut: Future<Output = Result<ScheduleView, ServerFnError>> + Send + 'static,
{
    // Only the client effect below fills the slot, so on the server every fetch
    // misses and goes through `fetch` as before.
    let slot = StoredValue::new(None::<Prefetched<K>>);
    // The key a prefetch is in flight for. A load that lands meanwhile, such as
    // the refetch a corrected timezone causes right after hydration, would
    // otherwise start a second fetch for the same key.
    let pending = StoredValue::new(None::<K>);
    // The key the resource last fetched for. The prefetch task checks this
    // rather than calling `key`, whose signals may be disposed by the time the
    // fetch returns if the viewer has left the page.
    let latest = StoredValue::new(untrack(key));
    let resource = Resource::new(key, move |k| async move {
        latest.set_value(k.clone());
        if let Some(p) = slot.try_update_value(|s| take_prefetched(s, &k)).flatten() {
            return Ok(p.view);
        }
        fetch(k).await
    });
    // After each load, fetch the other mode's schedule for the slot, unless the
    // slot already holds a recent one.
    #[cfg(feature = "hydrate")]
    Effect::new(move |_| {
        if !resource.with(|r| matches!(r, Some(Ok(_)))) {
            return;
        }
        let now = now_ms();
        let next = after_toggle(&untrack(key));
        if pending.with_value(|p| p.as_ref() == Some(&next))
            || !slot.with_value(|s| prefetch_due(s.as_ref(), &next, now))
        {
            return;
        }
        pending.set_value(Some(next.clone()));
        leptos::task::spawn_local(async move {
            let res = fetch(next.clone()).await;
            pending.update_value(|p| {
                if p.as_ref() == Some(&next) {
                    *p = None;
                }
            });
            if let Ok(view) = res
                && latest
                    .try_with_value(|l| prefetch_still_wanted(l, &next, after_toggle))
                    .unwrap_or(false)
            {
                slot.set_value(Some(Prefetched {
                    key: next,
                    at_ms: now_ms(),
                    view,
                }));
            }
        });
    });
    #[cfg(not(feature = "hydrate"))]
    let _ = (after_toggle, pending);
    resource
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

/// Whether this schedule is an esports (CS2/LoL/TFT) event — gates the event
/// page's "where to watch" streams block to the sports that carry stream links.
pub(crate) fn schedule_is_esports(s: &ScheduleView) -> bool {
    s.days
        .iter()
        .flat_map(|d| &d.leagues)
        .flat_map(|lg| &lg.matches)
        .any(|m| m.sport.esports())
}

#[derive(Params, PartialEq, Clone)]
pub(crate) struct DayParams {
    date: String,
}

#[component]
pub(crate) fn DayPage() -> impl IntoView {
    let params = use_params::<DayParams>();
    let date = move || params.get().ok().map_or_default(|p| p.date);
    let games = use_context::<Games>().expect("games context").0;
    let leagues = use_context::<Leagues>().expect("leagues context").0;
    let hour24 = use_context::<RwSignal<bool>>().expect("hour24 context");
    let tz = use_context::<RwSignal<String>>().expect("tz context");
    let traditional = use_context::<SportMode>().expect("sport mode context").0;
    let schedule = schedule_with_prefetch(
        move || (date(), tz.get(), hour24.get(), traditional.get()),
        |(d, z, h, trad): DayKey| async move {
            get_day(d, if trad { "trad" } else { "all" }.into(), z, h).await
        },
        day_key_after_toggle,
    );
    setup_autorefresh(schedule, view_has_live, true);

    view! {
        <FilterTabs games leagues traditional_row=false />
        <FilterTabs games leagues traditional_row=true />
        <ScheduleSection resource=schedule games leagues show_nav=true />
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefetched(key: u8, at_ms: i64) -> Prefetched<u8> {
        Prefetched {
            key,
            at_ms,
            view: ScheduleView::default(),
        }
    }

    #[test]
    fn a_prefetched_view_is_taken_only_for_its_own_key() {
        // Another key (the page's own refetch) leaves the slot alone.
        let mut s = Some(prefetched(1, 0));
        assert!(take_prefetched(&mut s, &2).is_none());
        assert!(s.is_some());
        // Its key takes it, and a second take (a refetch of that key) finds the
        // slot empty, so the refetch goes to the server.
        assert_eq!(take_prefetched(&mut s, &1).map(|p| p.key), Some(1));
        assert!(s.is_none());
        assert!(take_prefetched(&mut s, &1).is_none());
    }

    #[test]
    fn the_prefetch_is_due_when_missing_for_another_key_or_old() {
        let age = PREFETCH_RENEW_MS;
        assert!(prefetch_due::<u8>(None, &1, 0));
        assert!(prefetch_due(Some(&prefetched(2, 0)), &1, 0));
        let p = prefetched(1, 1_000);
        assert!(!prefetch_due(Some(&p), &1, 1_000 + age - 1));
        assert!(prefetch_due(Some(&p), &1, 1_000 + age));
    }

    #[test]
    fn a_returning_prefetch_is_kept_only_while_the_page_still_toggles_to_it() {
        let flip: fn(&(u8, bool)) -> (u8, bool) = |&(k, m)| (k, !m);
        // Nothing changed while it was in flight: keep it.
        assert!(prefetch_still_wanted(&(1, false), &(1, true), flip));
        // A toggle beat it, so it is now the page's own key.
        assert!(!prefetch_still_wanted(&(1, true), &(1, true), flip));
        // The key moved on (say, a corrected timezone): a newer prefetch is
        // due, and this older one must not replace it.
        assert!(!prefetch_still_wanted(&(2, false), &(1, true), flip));
    }

    #[test]
    fn the_toggled_home_key_flips_the_mode_and_keeps_the_range() {
        let range = Some(("2026-11-01".to_string(), "2026-11-07".to_string()));
        let k = (range.clone(), 3, 2, "UTC".to_string(), true, false);
        // The toggle resets the earlier/later expansions (see `SportToggle`).
        assert_eq!(
            home_key_after_toggle(&k),
            (range, 0, 0, "UTC".to_string(), true, true)
        );
        let d = ("2026-11-17".to_string(), "UTC".to_string(), false, true);
        assert_eq!(
            day_key_after_toggle(&d),
            ("2026-11-17".to_string(), "UTC".to_string(), false, false)
        );
    }
}
