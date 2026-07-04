//! Header preference toggles (theme, icons, 24h, scores, sport mode) and the
//! clock-link, plus their apply/saved helpers.
use crate::app::*;

#[component]
pub(crate) fn ThemeToggle() -> impl IntoView {
    // Cycle dark → oled (pure black) → light. The visible label is the current
    // theme name, rendered from `data-theme` on <html> via CSS (see `.theme-toggle`,
    // set pre-paint in the shell) — so it shows the saved theme at first paint with
    // no flash and no hydration mismatch (this button carries no reactive text). The
    // click reads the live attribute to pick the next theme.
    let cycle = move |_| {
        #[cfg(feature = "hydrate")]
        {
            let current = web_sys::window()
                .and_then(|w| w.document())
                .and_then(|d| d.document_element())
                .and_then(|el| el.get_attribute("data-theme"))
                .unwrap_or_else(|| "dark".to_string());
            let next = match current.as_str() {
                "dark" => "oled",
                "oled" => "light",
                _ => "dark",
            };
            apply_theme(next);
        }
    };

    view! {
        <button class="toggle theme-toggle" title="Switch color theme" aria-label="Switch color theme" on:click=cycle></button>
    }
}

/// Global on/off for team / driver / constructor logos. Default on; the actual
/// hiding is CSS keyed on `data-icons` on <html> (set pre-paint in the shell to
/// avoid a flash), so this only tracks state for the button's brightness and
/// flips the attribute + saved preference. Mirrors [`ThemeToggle`].
#[component]
pub(crate) fn IconsToggle() -> impl IntoView {
    // Default off; the hydrate effect below reads the saved preference.
    let show = RwSignal::new(false);

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
        // The on-state highlight comes from `data-icons` on <html> (set pre-paint),
        // not a reactive class — so it's correct at first paint with no flash. The
        // `show` signal only feeds the hover tooltip here.
        <button
            class="toggle state-toggle icons-toggle"
            title=move || if show.get() { "Hide team icons" } else { "Show team icons" }
            aria-pressed=move || if show.get() { "true" } else { "false" }
            on:click=toggle
        >
            "icons"
        </button>
    }
}

/// The saved icon preference: on unless explicitly stored off.
#[cfg(feature = "hydrate")]
pub(crate) fn saved_icons() -> bool {
    // Default off (like the scores toggle): on only when explicitly stored "1".
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item(keys::ICONS).ok().flatten())
        == Some("1".to_string())
}

/// Apply the icon preference: set `data-icons` on <html> and persist it.
#[cfg(feature = "hydrate")]
pub(crate) fn apply_icons(on: bool) {
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

/// Apply a theme: set `data-theme` on <html> and persist it.
#[cfg(feature = "hydrate")]
pub(crate) fn apply_theme(theme: &str) {
    if let Some(win) = web_sys::window() {
        if let Some(root) = win.document().and_then(|d| d.document_element()) {
            let _ = root.set_attribute("data-theme", theme);
        }
        if let Ok(Some(storage)) = win.local_storage() {
            let _ = storage.set_item(keys::THEME, theme);
        }
    }
}

/// Header link to the notifications page — a clock icon (the 24h/12h toggle that
/// used to live here now lives on that page, so the clock reads as "time + alerts
/// settings"). Always shown: the page is useful even without push (it hosts the
/// time-format toggle).
#[component]
pub(crate) fn ClockLink() -> impl IntoView {
    view! {
        <A href="/notifications" attr:class="toggle clock-link" attr:title="Notifications & time settings" attr:aria-label="Notifications">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
                <circle cx="12" cy="12" r="9"></circle>
                <polyline points="12 7 12 12 15.5 14"></polyline>
            </svg>
        </A>
    }
}

/// The 24h/12h time-format toggle. Lives on the notifications page (time-themed).
#[component]
pub(crate) fn HourToggle() -> impl IntoView {
    let hour24 = use_context::<RwSignal<bool>>().expect("hour24 context");
    let toggle = move |_| {
        let next = !hour24.get_untracked();
        hour24.set(next);
        #[cfg(feature = "hydrate")]
        save_hour24_pref(next);
    };
    view! {
        <button class="toggle" aria-pressed=move || if hour24.get() { "true" } else { "false" } on:click=toggle>
            {move || if hour24.get() { "24h" } else { "12h" }}
        </button>
    }
}

#[component]
pub(crate) fn ScoresToggle() -> impl IntoView {
    let show = use_context::<ShowScores>().expect("show_scores context").0;
    let toggle = move |_| {
        let next = !show.get_untracked();
        show.set(next);
        #[cfg(feature = "hydrate")]
        apply_scores(next);
    };
    view! {
        // Fixed "scores" label; the filled on-state (a more compact signal than a
        // "show"/"hide" prefix) is driven by `data-scores` on <html>, set pre-paint
        // so it doesn't flash on load. The action is spelled out in the tooltip.
        <button
            class="toggle state-toggle scores-toggle"
            title=move || if show.get() { "Hide scores" } else { "Show scores" }
            aria-pressed=move || if show.get() { "true" } else { "false" }
            on:click=toggle
        >
            "scores"
        </button>
    }
}

/// Switch between esports and traditional sports (MLB). Sits by the calendar.
#[component]
pub(crate) fn SportToggle() -> impl IntoView {
    let traditional = use_context::<SportMode>().expect("sport mode context").0;
    let earlier = use_context::<EarlierDays>().expect("earlier context").0;
    let later = use_context::<LaterDays>().expect("later context").0;
    let games = use_context::<Games>().expect("games context").0;
    let leagues = use_context::<Leagues>().expect("leagues context").0;
    // Resolve the router hooks during render (they read context); the click handler
    // uses them to send the viewer back to the home schedule. Client-only — the
    // toggle never fires on the server.
    #[cfg(feature = "hydrate")]
    let navigate = leptos_router::hooks::use_navigate();
    #[cfg(feature = "hydrate")]
    let location = leptos_router::hooks::use_location();
    let toggle = move |_| {
        let next = !traditional.get_untracked();
        traditional.set(next);
        // Start each mode from its default window (the forward cap differs).
        earlier.set(0);
        later.set(0);
        // The sport/sport tabs share the `games` set, so clear it (and the esports
        // event chips) on a mode switch — a stale "cs2"/"mlb" slug would otherwise
        // filter out everything in the other mode.
        games.update(HashSet::clear);
        leagues.update(HashSet::clear);
        #[cfg(feature = "hydrate")]
        {
            save_sport_pref(next);
            save_str_set(keys::GAMES, &games.get_untracked());
            save_str_set(keys::LEAGUES, &leagues.get_untracked());
            // The toggle is a jump back to the main schedule in the chosen mode, not
            // just an in-place flip. Only the schedule pages mirror the mode into
            // their URL (via `FilterUrlSync`); on a match/event/team page nothing
            // does, so flipping the signal alone would strand the viewer on a
            // now-irrelevant page. Send them home in the picked mode from there. On
            // the schedule pages `FilterUrlSync` already keeps the URL in sync in
            // place, so skip the redundant navigation (preserves scroll, no extra
            // history entry) and let the in-place behaviour stand.
            let path = location.pathname.get_untracked();
            let on_schedule = path == "/" || path.is_empty() || path.starts_with("/day");
            if !on_schedule {
                navigate(
                    if next { "/?mode=sports" } else { "/" },
                    leptos_router::NavigateOptions::default(),
                );
            }
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
                aria-pressed=move || if traditional.get() { "true" } else { "false" }
                on:click=toggle
            >
                {move || if traditional.get() { "sports" } else { "esports" }}
            </button>
        </div>
    }
}
