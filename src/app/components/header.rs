//! Site header: the bar, brand/back-to-top slot, and refresh button.
use crate::app::*;

#[component]
pub(crate) fn SiteHeader() -> impl IntoView {
    // The brand and the back-to-top arrow share the first slot of the sticky
    // toggle bar: the brand shows at the top, and once scrolled it swaps to the ↑
    // arrow in its place. The brand is a real bar item (it reserves its space), so
    // nothing overlaps and the controls keep their place at any width.
    view! {
        // `display: contents` keeps this landmark out of the box tree so the
        // sticky `.toggles` bar's containing block stays `.page` (not this header)
        // — the banner landmark is added with zero layout/behavior change.
        <header class="site-header" style="display: contents">
            <div class="toggles">
                <BrandSlot />
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
                <RefreshButton />
                <SportToggle />
                <CalendarPicker />
                // The notifications link sits with the schedule controls, just right of
                // the calendar.
                <ClockLink />
                <ScoresToggle />
            </div>
        </header>
    }
}

/// Shows when the server's schedule data was last refreshed, and acts as a button
/// to pull the latest from the server (a client refetch — it never forces the
/// server to re-poll its own data source). Hidden until a schedule has loaded.
#[component]
pub(crate) fn RefreshButton() -> impl IntoView {
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
                // The visible "updated HH:MM" text is the button's accessible name;
                // the longer hint stays in `title` only (an aria-label that omits the
                // visible text would trip WCAG 2.5.3, label-in-name).
                <button class="refresh-btn" title=title on:click=bump>
                    <span class="refresh-icon">"\u{21bb}"</span>
                    // The "updated" prefix is dropped on mobile (just the icon +
                    // time) so the schedule controls + clock stay on one row.
                    <span class="refresh-pre">"updated "</span>
                    {label}
                </button>
            }
        })
    }
}

/// The brand and a "back to top" arrow sharing the first slot of the sticky
/// toggle bar: the brand shows at the top of the page, then swaps to the ↑ arrow
/// once scrolled (a click jumps back up). Kept as one component so a single scroll
/// listener drives the swap; the arrow lands exactly where the brand was.
#[component]
pub(crate) fn BrandSlot() -> impl IntoView {
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
        <div class="brand-slot">
            <div class="brand" class:brand-off=move || scrolled.get()>
                <A href="/">"plaintextesports"</A>
            </div>
            <button
                class="toggle scrolltop"
                class:scrolltop-off=move || !scrolled.get()
                title="Back to top"
                aria-label="Back to top"
                on:click=to_top
            >
                "↑"
            </button>
        </div>
    }
}
