//! Reminder subscribe controls: the sport/event ★ and the per-match ★.
use crate::app::*;

/// ★ toggle to subscribe to a whole sport (`kind="sport"`, value "cs2"/"lol") or
/// a league/competition (`kind="league"`, value = league name). `sport` scopes
/// the persisted key (see [`sub_key`]). Used inside a sport filter tab and in
/// league/event headers. Hidden unless push is on.
#[component]
pub(crate) fn SubscribeStar(kind: &'static str, sport: Sport, value: String) -> impl IntoView {
    let subscribed = use_context::<Subscribed>().expect("subscribed context").0;
    let vapid = use_context::<RwSignal<Option<String>>>().expect("vapid context");
    let global = use_context::<GlobalTimers>().expect("global timers context").0;
    let overrides = use_context::<Overrides>().expect("overrides context").0;
    let key = sub_key(kind, sport, &value);
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
            let leads = effective_leads(&key, &overrides.get_untracked(), &global.get_untracked());
            subscribe_scope(kind.to_string(), value.clone(), !now_on, keys, vapid.get_untracked(), leads);
        }
        #[cfg(not(feature = "hydrate"))]
        {
            let _ = (now_on, &value, vapid, global, overrides);
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

#[component]
pub(crate) fn StarButton(id: i64, sport: Sport, league: String, team_a: String, team_b: String) -> impl IntoView {
    let starred = use_context::<RwSignal<HashSet<String>>>().expect("starred context");
    let subscribed = use_context::<Subscribed>().expect("subscribed context").0;
    let excluded = use_context::<Excluded>().expect("excluded context").0;
    let vapid = use_context::<RwSignal<Option<String>>>().expect("vapid context");
    let global = use_context::<GlobalTimers>().expect("global timers context").0;
    let overrides = use_context::<Overrides>().expect("overrides context").0;
    // The match's uid keys the star/exclude sets (ids aren't unique across games).
    let uid = crate::types::match_uid(sport, id);
    // The scopes that auto-cover this match (its sport, this event, or either team).
    let keys = [
        sub_key("sport", sport, sport.slug()),
        sub_key("league", sport, &league),
        sub_key("team", sport, &team_a),
        sub_key("team", sport, &team_b),
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
            let leads = effective_leads(&uid, &overrides.get_untracked(), &global.get_untracked());
            sync_match_reminder(id, sport, want_on, covered, vapid.get_untracked(), leads);
        }
        #[cfg(not(feature = "hydrate"))]
        {
            let _ = (covered, want_on, vapid, sport, global, overrides);
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

