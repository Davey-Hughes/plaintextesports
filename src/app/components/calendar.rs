//! The calendar date picker and its civil-date math helpers.
use crate::app::prelude::*;
use leptos::prelude::*;

// ----- Calendar date-range picker ------------------------------------------

/// Outline calendar glyph (Feather-style); `currentColor` follows the button.
pub(crate) const CALENDAR_ICON: &str = r#"<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="5" width="18" height="16" rx="2"></rect><line x1="3" y1="9" x2="21" y2="9"></line><line x1="8" y1="3" x2="8" y2="6"></line><line x1="16" y1="3" x2="16" y2="6"></line></svg>"#;

pub(crate) fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// Days in month `m` (1-12) of year `y`.
pub(crate) fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        2 if is_leap(y) => 29,
        2 => 28,
        // 4, 6, 9, 11 — and any out-of-range month, which no caller passes.
        _ => 30,
    }
}

/// Day of week for a date (0 = Sunday), via Sakamoto's algorithm.
// `day` is a day-of-month, 1..=31 — every caller gets it from `days_in_month`
// or a parsed ISO date. Nowhere near i32's range.
#[allow(clippy::cast_possible_wrap)]
pub(crate) fn weekday(year: i32, month: u32, day: u32) -> u32 {
    // Sakamoto's per-month offset table, indexed by month - 1.
    let offsets = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    // January and February are treated as months 13 and 14 of the prior year.
    let y = if month < 3 { year - 1 } else { year };
    let sum = y + y / 4 - y / 100 + y / 400 + offsets[(month - 1) as usize] + day as i32;
    sum.rem_euclid(7) as u32
}

pub(crate) fn month_name(m: u32) -> &'static str {
    [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ]
    .get(m.saturating_sub(1) as usize)
    .copied()
    .unwrap_or("")
}

/// Today as (year, month, day) in the browser's local date.
#[cfg(feature = "hydrate")]
// `js_sys::Date::get_full_year` returns a u32 holding a four-digit year.
#[allow(clippy::cast_possible_wrap)]
pub(crate) fn today_ymd() -> (i32, u32, u32) {
    let d = js_sys::Date::new_0();
    (d.get_full_year() as i32, d.get_month() + 1, d.get_date())
}

/// A 📅 button that opens a monospace month-grid range picker. Click a start day
/// then an end day, Apply to filter the schedule to that range.
#[component]
pub(crate) fn CalendarPicker() -> impl IntoView {
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
    let pick = move |iso: String| {
        if let (Some(start), None) = (sel_start.get_untracked(), sel_end.get_untracked()) {
            if iso < start {
                sel_start.set(Some(iso));
                sel_end.set(Some(start));
            } else {
                sel_end.set(Some(iso));
            }
        } else {
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
                    Some((
                        iso_from_today(-earlier.get()),
                        iso_from_today(trad_forward(later.get())),
                    ))
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
                    <button class=cls title=format!("{y:04}-{m:02}-{day:02}") on:click=on_pick>
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
                                    <button class="cal-nav" aria-label="Previous month" on:click=prev_month>
                                        "‹"
                                    </button>
                                    <span class="cal-title">
                                        {move || {
                                            let (y, m) = ym.get();
                                            format!("{} {y}", month_name(m))
                                        }}
                                    </span>
                                    <button class="cal-nav" aria-label="Next month" on:click=next_month>
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

#[cfg(test)]
mod tests {
    use super::{days_in_month, is_leap, month_name, weekday};

    #[test]
    fn leap_years_follow_the_gregorian_rule() {
        // Divisible by 4, except centuries, except multiples of 400.
        assert!(is_leap(2024));
        assert!(!is_leap(2026));
        assert!(!is_leap(1900), "a century that is not a multiple of 400");
        assert!(is_leap(2000), "a century that is a multiple of 400");
    }

    #[test]
    fn february_is_the_only_month_whose_length_moves() {
        assert_eq!(days_in_month(2026, 2), 28);
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(1900, 2), 28);
        assert_eq!(days_in_month(2000, 2), 29);
        // The 31/30 split is fixed. Guards the wildcard arm, which covers the
        // 30-day months by falling through rather than by listing them.
        for m in [1, 3, 5, 7, 8, 10, 12] {
            assert_eq!(days_in_month(2026, m), 31, "month {m}");
        }
        for m in [4, 6, 9, 11] {
            assert_eq!(days_in_month(2026, m), 30, "month {m}");
        }
    }

    #[test]
    fn weekday_matches_known_dates() {
        // 0 = Sunday. Includes January and February, which Sakamoto's algorithm
        // treats as months 13 and 14 of the prior year — the branch a rewrite is
        // most likely to break.
        assert_eq!(weekday(2026, 8, 1), 6, "2026-08-01 is a Saturday");
        assert_eq!(weekday(2026, 8, 10), 1, "2026-08-10 is a Monday");
        assert_eq!(weekday(2026, 1, 1), 4, "2026-01-01 is a Thursday");
        assert_eq!(weekday(2026, 2, 28), 6, "2026-02-28 is a Saturday");
        assert_eq!(weekday(2024, 2, 29), 4, "leap day 2024 is a Thursday");
        assert_eq!(weekday(2000, 1, 1), 6, "2000-01-01 is a Saturday");
        // Carried over from `app::tests::calendar_date_math`, which used to hold
        // this file's coverage from the parent module.
        assert_eq!(weekday(2026, 6, 21), 0, "2026-06-21 is a Sunday");
        assert_eq!(weekday(2026, 6, 1), 1, "2026-06-01 is a Monday");
    }

    #[test]
    fn month_name_is_one_based_and_bounded() {
        assert_eq!(month_name(1), "January");
        assert_eq!(month_name(6), "June");
        assert_eq!(month_name(12), "December");
        // Past the end yields "" rather than panicking on the index.
        assert_eq!(month_name(13), "");
        // Below the start does NOT: `saturating_sub(1)` floors 0 at index 0, so
        // month 0 aliases to January. Harmless — every caller passes 1..=12 from
        // `ym` or a parsed date — but asserted so the aliasing is a known
        // property rather than a surprise if someone starts trusting the bound.
        assert_eq!(month_name(0), "January");
    }
}
