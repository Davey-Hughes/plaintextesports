//! The team schedule page.
use crate::app::*;

#[derive(Params, PartialEq, Clone)]
pub(crate) struct TeamParams {
    /// The sport slug segment, e.g. `"mlb"` — scopes the team but the lookup is
    /// still keyed by `name` alone.
    sport: String,
    name: String,
}

/// A single team's schedule (past + upcoming), reachable from the match page.
/// Traditional-sport teams window like the homepage/event page (with the
/// earlier/later expanders); esports teams show their full tier-1 history.
#[component]
pub(crate) fn TeamPage() -> impl IntoView {
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

    // Persist the pinned "up next" bar's scroll-visibility across the per-refetch
    // subtree rebuild so a refresh doesn't flash the bar back on. See `UpNextSeen`.
    provide_context(UpNextSeen {
        day: RwSignal::new(false),
        foot: RwSignal::new(false),
    });

    // Keep the global sport mode in sync with the team being viewed, so the
    // header toggle/footer/windowing agree (client-only; never persisted).
    Effect::new(move |_| {
        if let Some(Ok(s)) = schedule.get() {
            sport_mode.set(schedule_is_traditional(&s));
        }
    });

    // Transition, not Suspense: the 60s auto-refresh and the header button
    // refetch the schedule in place, and Transition keeps the current page
    // visible while it reloads (Suspense would flash back to the fallback).
    view! {
        <Transition fallback=|| {
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
                        let logo = team_logo_in_schedule(&s, &name);
                        let team_sport = s
                            .days
                            .iter()
                            .flat_map(|d| &d.leagues)
                            .flat_map(|lg| &lg.matches)
                            .map(|m| m.sport)
                            .next()
                            .unwrap_or_default();
                        let windowed = schedule_needs_window(&s);
                        if windowed {
                            let (lo, hi) = trad_day_bounds(
                                &s.today_key,
                                earlier.get(),
                                later.get(),
                            );
                            s.days
                                .retain(|d| {
                                    d.day_key.as_str() >= lo.as_str()
                                        && d.day_key.as_str() <= hi.as_str()
                                });
                        }
                        let push = use_context::<RwSignal<Option<String>>>()
                            .is_some_and(|v| v.get().is_some());
                        // The team's vector logo (NHL/MLB), from any of its matches.
                        // The team's sport (it plays one), read from any of its
                        // matches — scopes both its page URL and its subscribe key.
                        // Traditional-sport teams cap their forward horizon and show
                        // the earlier/later expanders, like the event page.
                        // Read push here so the ★s appear once the VAPID key
                        // resolves after hydration (this closure re-runs then).
                        view! {
                            <article class="detail">
                                <A href="/">"← schedule"</A>
                                <div class="team-head">
                                    <SubscribeStar
                                        kind="team"
                                        sport=team_sport
                                        value=name.clone()
                                    />
                                    {team_logo(&logo, "team-logo-lg")}
                                    <h1 class="detail-title">{name.clone()}</h1>
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
        </Transition>
    }
}
