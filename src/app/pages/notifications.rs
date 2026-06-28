//! The notifications page and the whole reminder model: timer presets, the
//! export/import backup codec, the per-entry timer pickers, and the hydrate-only
//! arming / reconciliation glue (incl. the Web Push JS interop).
use crate::app::*;

// ----- Reminder timers (lead offsets) --------------------------------------

/// One week in ms — the ceiling for a reminder lead offset.
pub(crate) const WEEK_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// At most this many timers per list (global or per-entry override).
pub(crate) const MAX_TIMERS: usize = 10;

/// The preset lead offsets (ms) shown as chips, smallest → largest (0 = at start).
pub(crate) const TIMER_PRESETS: [i64; 12] = [
    0,
    5 * 60_000,
    15 * 60_000,
    30 * 60_000,
    60 * 60_000,
    3 * 60 * 60_000,
    6 * 60 * 60_000,
    12 * 60 * 60_000,
    24 * 60 * 60_000,
    2 * 24 * 60 * 60_000,
    3 * 24 * 60 * 60_000,
    7 * 24 * 60 * 60_000,
];

/// A compact label for a lead offset: "at start", "15m", "3h", "2d", "1w" —
/// the largest whole unit that divides it, else minutes.
pub(crate) fn fmt_lead(ms: i64) -> String {
    if ms <= 0 {
        return "at start".to_string();
    }
    let mins = ms / 60_000;
    const WK: i64 = 7 * 24 * 60;
    const DAY: i64 = 24 * 60;
    const HR: i64 = 60;
    if mins % WK == 0 {
        format!("{}w", mins / WK)
    } else if mins % DAY == 0 {
        format!("{}d", mins / DAY)
    } else if mins % HR == 0 {
        format!("{}h", mins / HR)
    } else {
        format!("{mins}m")
    }
}

/// Normalize a timer list: keep `0..=1 week`, sort, dedup, cap at [`MAX_TIMERS`].
pub(crate) fn norm_leads(mut v: Vec<i64>) -> Vec<i64> {
    v.retain(|&ms| (0..=WEEK_MS).contains(&ms));
    v.sort_unstable();
    v.dedup();
    v.truncate(MAX_TIMERS);
    v
}

/// Parse a comma-separated ms list (the localStorage `timers` value / override).
#[cfg_attr(not(feature = "hydrate"), allow(dead_code))]
pub(crate) fn parse_lead_list(s: &str) -> Vec<i64> {
    norm_leads(
        s.split(',')
            .filter_map(|p| p.trim().parse::<i64>().ok())
            .collect(),
    )
}

/// Serialize a timer list as comma-separated ms (for localStorage).
#[cfg_attr(not(feature = "hydrate"), allow(dead_code))]
pub(crate) fn join_lead_list(v: &[i64]) -> String {
    v.iter().map(i64::to_string).collect::<Vec<_>>().join(",")
}

/// The effective timers for an entry: its override if set, else the global list.
/// (Client-only: resolved when arming a reminder/subscription.)
#[cfg_attr(not(feature = "hydrate"), allow(dead_code))]
pub(crate) fn effective_leads(
    key: &str,
    overrides: &BTreeMap<String, Vec<i64>>,
    global: &[i64],
) -> Vec<i64> {
    overrides
        .get(key)
        .cloned()
        .unwrap_or_else(|| global.to_vec())
}

/// Split a match uid (`"{slug}-{id}"`, see [`crate::types::match_uid`]) back into
/// its sport + id. Slugs carry no hyphen and the id is the trailing number.
#[cfg_attr(not(feature = "hydrate"), allow(dead_code))]
pub(crate) fn parse_uid(uid: &str) -> Option<(Sport, i64)> {
    let (slug, id) = uid.rsplit_once('-')?;
    Some((Sport::from_filter(slug)?, id.parse().ok()?))
}

// ----- Notifications export / import ----------------------------------------

/// A portable snapshot of every client-side notification setting, for transfer
/// between devices via a single copyable string.
#[derive(Default, PartialEq, Eq, Debug)]
pub(crate) struct NotifBackup {
    subs: Vec<String>,
    starred: Vec<String>,
    excluded: Vec<String>,
    timers: Vec<i64>,
    overrides: Vec<(String, Vec<i64>)>,
}

/// Lead offsets are always whole minutes (every preset + custom unit is a
/// minute-multiple), so the export stores minutes — far shorter than the ms used
/// internally. These convert between the two for the wire format.
pub(crate) fn join_lead_min(v: &[i64]) -> String {
    v.iter()
        .map(|ms| (ms / 60_000).to_string())
        .collect::<Vec<_>>()
        .join(",")
}

pub(crate) fn parse_lead_min(s: &str) -> Vec<i64> {
    norm_leads(
        s.split(',')
            .filter_map(|p| p.trim().parse::<i64>().ok())
            .map(|m| m * 60_000)
            .collect(),
    )
}

/// DEFLATE a byte slice (best compression). Empty on failure (the caller only
/// uses the result when it's actually shorter than the uncompressed form).
pub(crate) fn deflate(data: &[u8]) -> Vec<u8> {
    use flate2::write::DeflateEncoder;
    use flate2::Compression;
    use std::io::Write;
    let mut e = DeflateEncoder::new(Vec::new(), Compression::best());
    if e.write_all(data).is_err() {
        return Vec::new();
    }
    e.finish().unwrap_or_default()
}

pub(crate) fn inflate(data: &[u8]) -> Option<Vec<u8>> {
    use flate2::read::DeflateDecoder;
    use std::io::Read;
    let mut out = Vec::new();
    DeflateDecoder::new(data).read_to_end(&mut out).ok()?;
    Some(out)
}

/// Encode a backup as a single copyable string. The body is a tab/newline-
/// delimited text (entry keys never contain a tab or newline, so the framing is
/// unambiguous); it's base64url'd as `pte1:`, or DEFLATE'd first as `pte1z:` when
/// that comes out shorter — so tiny payloads stay tiny and large ones compress.
pub(crate) fn encode_backup(b: &NotifBackup) -> String {
    use base64::Engine;
    let row = |tag: &str, items: &[String]| {
        let mut s = String::from(tag);
        for it in items {
            s.push('\t');
            s.push_str(it);
        }
        s
    };
    let mut lines = vec![
        row("S", &b.subs),
        row("R", &b.starred),
        row("X", &b.excluded),
    ];
    lines.push(format!("T\t{}", join_lead_min(&b.timers)));
    for (k, v) in &b.overrides {
        lines.push(format!("O\t{k}\t{}", join_lead_min(v)));
    }
    let body = lines.join("\n");
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let plain = format!("pte1:{}", b64.encode(body.as_bytes()));
    let zipped = format!("pte1z:{}", b64.encode(deflate(body.as_bytes())));
    if zipped.len() < plain.len() {
        zipped
    } else {
        plain
    }
}

/// Inverse of [`encode_backup`]; `None` if the prefix/encoding is invalid.
pub(crate) fn decode_backup(s: &str) -> Option<NotifBackup> {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let s = s.trim();
    let body = if let Some(rest) = s.strip_prefix("pte1z:") {
        String::from_utf8(inflate(&b64.decode(rest).ok()?)?).ok()?
    } else {
        let rest = s.strip_prefix("pte1:")?;
        String::from_utf8(b64.decode(rest).ok()?).ok()?
    };
    let mut b = NotifBackup::default();
    for line in body.split('\n') {
        let mut f = line.split('\t');
        match f.next() {
            Some("S") => b.subs = f.filter(|p| !p.is_empty()).map(str::to_string).collect(),
            Some("R") => b.starred = f.filter(|p| !p.is_empty()).map(str::to_string).collect(),
            Some("X") => b.excluded = f.filter(|p| !p.is_empty()).map(str::to_string).collect(),
            Some("T") => b.timers = parse_lead_min(f.next().unwrap_or_default()),
            Some("O") => {
                if let Some(key) = f.next().filter(|k| !k.is_empty()) {
                    b.overrides.push((
                        key.to_string(),
                        parse_lead_min(f.next().unwrap_or_default()),
                    ));
                }
            }
            _ => {}
        }
    }
    Some(b)
}

/// A reusable timer-list editor: toggle preset chips or add an arbitrary offset
/// (number + unit), with a live summary. Calls `on_change` with the new
/// normalized list on every edit. Drives the global timers and per-entry overrides.
#[component]
pub(crate) fn TimerPicker(
    #[prop(into)] current: Signal<Vec<i64>>,
    on_change: Callback<Vec<i64>>,
) -> impl IntoView {
    // Custom-offset inputs: a number and a unit (ms multiplier).
    let num = RwSignal::new(String::new());
    let unit = RwSignal::new(60_000_i64); // minutes by default

    // The add panel is hidden until "+" is pressed; picking a time closes it again.
    let adding = RwSignal::new(false);

    let remove_lead = move |ms: i64| {
        let mut v = current.get_untracked();
        v.retain(|&x| x != ms);
        on_change.run(norm_leads(v));
    };
    // Add one offset, then collapse the selection (so a pick is one-and-done).
    let add_lead = move |ms: i64| {
        if (0..=WEEK_MS).contains(&ms) {
            let mut v = current.get_untracked();
            if !v.contains(&ms) && v.len() < MAX_TIMERS {
                v.push(ms);
                on_change.run(norm_leads(v));
            }
        }
        adding.set(false);
    };
    let add_custom = move |_| {
        let Ok(n) = num.get_untracked().trim().parse::<i64>() else {
            return;
        };
        num.set(String::new());
        add_lead(n.max(0).saturating_mul(unit.get_untracked()));
    };

    view! {
        <div class="timer-picker">
            <div class="timer-row">
                // Active timers — each a box you click to remove.
                {move || {
                    let mut v = current.get();
                    v.sort_unstable();
                    v.into_iter()
                        .map(|ms| {
                            view! {
                                <button
                                    class="chip timer-chip on"
                                    title="Remove this reminder time"
                                    on:click=move |_| remove_lead(ms)
                                >
                                    {fmt_lead(ms)}
                                    <span class="timer-x" aria-hidden="true">"×"</span>
                                </button>
                            }
                        })
                        .collect_view()
                }}
                <button
                    class="chip timer-add-toggle"
                    class:on=move || adding.get()
                    title="Add a reminder time"
                    aria-label="Add a reminder time"
                    on:click=move |_| adding.update(|a| *a = !*a)
                >
                    "+"
                </button>
            </div>
            {move || {
                adding
                    .get()
                    .then(|| {
                        view! {
                            // Pick a preset, or set a custom amount — either closes the panel.
                            <div class="timer-add-panel">
                                {move || {
                                    let cur = current.get();
                                    TIMER_PRESETS
                                        .iter()
                                        .copied()
                                        .filter(|ms| !cur.contains(ms))
                                        .map(|ms| {
                                            view! {
                                                <button
                                                    class="chip timer-chip"
                                                    on:click=move |_| add_lead(ms)
                                                >
                                                    {fmt_lead(ms)}
                                                </button>
                                            }
                                        })
                                        .collect_view()
                                }}
                                <span class="timer-custom-label">"or"</span>
                                <input
                                    class="timer-num"
                                    type="number"
                                    min="0"
                                    placeholder="2"
                                    prop:value=move || num.get()
                                    on:input=move |ev| num.set(event_target_value(&ev))
                                    on:keydown=move |ev| {
                                        if ev.key() == "Enter" {
                                            add_custom(());
                                        }
                                    }
                                />
                                <select
                                    class="timer-unit"
                                    on:change=move |ev| {
                                        if let Ok(v) = event_target_value(&ev).parse::<i64>() {
                                            unit.set(v);
                                        }
                                    }
                                >
                                    <option value="60000">"min"</option>
                                    <option value="3600000">"hr"</option>
                                    <option value="86400000">"days"</option>
                                    <option value="604800000">"wk"</option>
                                </select>
                                <button class="chip timer-add" on:click=move |_| add_custom(())>
                                    "add"
                                </button>
                            </div>
                        }
                    })
            }}
        </div>
    }
}

/// A per-entry timer control, shown inline on a subscription/match row. While the
/// entry follows the global timers it's just a small "custom" link (no boxes — the
/// global list isn't repeated on every row). Clicking it forks an override seeded
/// from the current global timers, revealing the same boxes + "+" picker plus a
/// "↺ global" reset. `rearm` re-applies the resulting list server-side.
#[component]
pub(crate) fn EntryTimers(entry_key: String, rearm: Callback<Vec<i64>>) -> impl IntoView {
    let global = use_context::<GlobalTimers>()
        .expect("global timers context")
        .0;
    let overrides = use_context::<Overrides>().expect("overrides context").0;
    let vapid = use_context::<RwSignal<Option<String>>>().expect("vapid context");
    // Nothing to schedule when push is off.
    let hidden = move || vapid.with(|v| v.is_none());

    let k_has = entry_key.clone();
    let has_override = Memo::new(move |_| overrides.with(|o| o.contains_key(&k_has)));
    let k_cur = entry_key.clone();
    let current = Signal::derive(move || {
        overrides
            .with(|o| o.get(&k_cur).cloned())
            .unwrap_or_else(|| global.get())
    });

    let k_set = entry_key.clone();
    let on_change = Callback::new(move |leads: Vec<i64>| {
        // Removing the last custom timer reverts the entry to the global timers
        // (rather than leaving it with no reminders at all).
        let revert = leads.is_empty();
        overrides.update(|o| {
            if revert {
                o.remove(&k_set);
            } else {
                o.insert(k_set.clone(), leads.clone());
            }
        });
        #[cfg(feature = "hydrate")]
        save_overrides(&overrides.get_untracked());
        rearm.run(if revert {
            global.get_untracked()
        } else {
            leads
        });
    });

    // Start customizing: fork an override from the current global timers. A
    // Callback (Copy) so the reactive block can re-render it freely.
    let k_start = entry_key.clone();
    let start_custom = Callback::new(move |()| {
        let g = global.get_untracked();
        overrides.update(|o| {
            o.insert(k_start.clone(), g.clone());
        });
        #[cfg(feature = "hydrate")]
        save_overrides(&overrides.get_untracked());
        rearm.run(g);
    });

    let k_reset = entry_key;
    let reset = Callback::new(move |()| {
        overrides.update(|o| {
            o.remove(&k_reset);
        });
        #[cfg(feature = "hydrate")]
        save_overrides(&overrides.get_untracked());
        rearm.run(global.get_untracked());
    });

    view! {
        <div class="entry-timers" class:event-hidden=hidden>
            {move || {
                if has_override.get() {
                    view! {
                        <span class="entry-timers-label on">"custom"</span>
                        <TimerPicker current=current on_change=on_change />
                        <button
                            class="linkish entry-timers-reset"
                            title="Follow the global timers instead"
                            on:click=move |_| reset.run(())
                        >
                            "↺ global"
                        </button>
                    }
                        .into_any()
                } else {
                    view! {
                        <button
                            class="linkish entry-timers-custom"
                            title="Set timers just for this — overriding the global timers"
                            on:click=move |_| start_custom.run(())
                        >
                            "custom"
                        </button>
                    }
                        .into_any()
                }
            }}
        </div>
    }
}

/// Manage what you're notified about: your sport/event/team subscriptions, plus
/// individually-starred upcoming matches. Matches that have already started or
/// finished are left out (their reminders can no longer fire).
#[component]
pub(crate) fn NotificationsPage() -> impl IntoView {
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
                Ok(crate::types::NotificationsView::default())
            } else {
                get_notifications(uids, z, h).await
            }
        },
    );

    // Silently drop starred matches the server reports as finished/started, so
    // they neither show below nor linger in the export string.
    Effect::new(move |_| {
        if let Some(Ok(view)) = starred_matches.get() {
            if !view.stale.is_empty() {
                let stale: HashSet<String> = view.stale.into_iter().collect();
                let mut changed = false;
                starred.update(|s| {
                    let before = s.len();
                    s.retain(|u| !stale.contains(u));
                    changed = s.len() != before;
                });
                if changed {
                    #[cfg(feature = "hydrate")]
                    save_starred(&starred.get_untracked());
                }
            }
        }
    });

    let global = use_context::<GlobalTimers>()
        .expect("global timers context")
        .0;
    let overrides = use_context::<Overrides>().expect("overrides context").0;
    let push_on = move || vapid.with(|v| v.is_some());

    // Anything to clear? (subscriptions or stars; exclusions alone aren't shown).
    let has_any = move || !subscribed.with(HashSet::is_empty) || !starred.with(HashSet::is_empty);
    let clear_all = move |_| {
        subscribed.set(HashSet::new());
        starred.set(HashSet::new());
        excluded.set(HashSet::new());
        overrides.set(BTreeMap::new());
        #[cfg(feature = "hydrate")]
        {
            let empty: Vec<String> = Vec::new();
            save_subs(&empty);
            save_starred(&HashSet::new());
            save_excluded(&HashSet::new());
            save_overrides(&BTreeMap::new());
            clear_all_notifications(vapid.get_untracked());
        }
        #[cfg(not(feature = "hydrate"))]
        {
            let _ = vapid;
        }
    };

    // Editing the global timers re-arms every entry that has no override of its
    // own, so the change takes effect on the server immediately.
    let on_global_change = Callback::new(move |next: Vec<i64>| {
        global.set(next.clone());
        #[cfg(feature = "hydrate")]
        {
            save_timers(&next);
            let ov = overrides.get_untracked();
            let subs_re: Vec<(String, String, Vec<i64>)> = subscribed
                .get_untracked()
                .iter()
                .filter(|k| !ov.contains_key(*k))
                .map(|k| {
                    let (kind, _s, value) = parse_sub_key(k);
                    (kind, value, next.clone())
                })
                .collect();
            let stars_re: Vec<(i64, String, Vec<i64>)> = starred
                .get_untracked()
                .iter()
                .filter(|u| !ov.contains_key(*u))
                .filter_map(|u| {
                    parse_uid(u).map(|(sp, id)| (id, sp.slug().to_string(), next.clone()))
                })
                .collect();
            sync_entries(false, subs_re, stars_re, Vec::new(), vapid.get_untracked());
        }
        #[cfg(not(feature = "hydrate"))]
        let _ = (overrides, subscribed, starred, vapid);
    });

    // The single export string for the whole notification set.
    let export_str = Memo::new(move |_| {
        let mut subs: Vec<String> = subscribed.get().into_iter().collect();
        subs.sort();
        let mut star: Vec<String> = starred.get().into_iter().collect();
        star.sort();
        let mut excl: Vec<String> = excluded.get().into_iter().collect();
        excl.sort();
        let ov: Vec<(String, Vec<i64>)> = overrides.get().into_iter().collect();
        encode_backup(&NotifBackup {
            subs,
            starred: star,
            excluded: excl,
            timers: global.get(),
            overrides: ov,
        })
    });
    let import_text = RwSignal::new(String::new());
    let import_status = RwSignal::new(String::new());
    let copy_hint = RwSignal::new(String::new());

    // Shared so both the click and the keyboard (Enter/Space) handlers can copy.
    let do_copy = Callback::new(move |()| {
        #[cfg(feature = "hydrate")]
        {
            let s = export_str.get_untracked();
            if let Some(clip) = web_sys::window().map(|w| w.navigator().clipboard()) {
                let _ = clip.write_text(&s);
                copy_hint.set("Copied to clipboard ✓".to_string());
            }
        }
        #[cfg(not(feature = "hydrate"))]
        let _ = copy_hint;
    });
    let copy_export = move |_| do_copy.run(());

    // Import: "override" replaces the whole set (and clears the server first);
    // "add" unions the new entries onto the existing ones (existing overrides win,
    // global timers are left as this device's own).
    let do_import = Callback::new(move |replace: bool| {
        let Some(b) = decode_backup(&import_text.get_untracked()) else {
            import_status.set("That doesn't look like a valid code.".to_string());
            return;
        };
        if replace {
            subscribed.set(b.subs.iter().cloned().collect());
            starred.set(b.starred.iter().cloned().collect());
            excluded.set(b.excluded.iter().cloned().collect());
            global.set(b.timers.clone());
            overrides.set(b.overrides.iter().cloned().collect());
        } else {
            subscribed.update(|s| s.extend(b.subs.iter().cloned()));
            starred.update(|s| s.extend(b.starred.iter().cloned()));
            excluded.update(|s| s.extend(b.excluded.iter().cloned()));
            overrides.update(|o| {
                for (k, v) in &b.overrides {
                    o.entry(k.clone()).or_insert_with(|| v.clone());
                }
            });
        }
        #[cfg(feature = "hydrate")]
        {
            save_subs(&subscribed.get_untracked().into_iter().collect::<Vec<_>>());
            save_starred(&starred.get_untracked());
            save_excluded(&excluded.get_untracked());
            save_timers(&global.get_untracked());
            save_overrides(&overrides.get_untracked());
            // Re-arm the imported entries with their effective leads (merged state).
            let ov = overrides.get_untracked();
            let g = global.get_untracked();
            let subs_re: Vec<_> = b
                .subs
                .iter()
                .map(|k| {
                    let (kind, _s, value) = parse_sub_key(k);
                    (kind, value, effective_leads(k, &ov, &g))
                })
                .collect();
            let stars_re: Vec<_> = b
                .starred
                .iter()
                .filter_map(|u| {
                    parse_uid(u)
                        .map(|(sp, id)| (id, sp.slug().to_string(), effective_leads(u, &ov, &g)))
                })
                .collect();
            let excl_re: Vec<_> = b
                .excluded
                .iter()
                .filter_map(|u| parse_uid(u).map(|(sp, id)| (id, sp.slug().to_string())))
                .collect();
            sync_entries(replace, subs_re, stars_re, excl_re, vapid.get_untracked());
        }
        import_status.set(format!(
            "Imported {} subscription(s) and {} match(es).",
            b.subs.len(),
            b.starred.len()
        ));
        import_text.set(String::new());
    });

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

            {move || {
                if !push_on() {
                    return view! {
                        <p class="empty">
                            "Push notifications aren't enabled on this site, so there's nothing else to manage here."
                        </p>
                    }
                        .into_any();
                }
                view! {
                    <p class="notif-intro">
                        "Everything you'll be reminded about. Subscriptions cover every "
                        "upcoming match for a sport, competition, event, or team."
                    </p>
                    // Global timers + the override explainer; the divider sits
                    // under the explanatory text, above the picker.
                    <section class="notif-section notif-timers-section">
                        <h2 class="notif-head">"Notification timers"</h2>
                        <p class="notif-note">
                            "The global reminder times for every subscription and starred match. "
                            "Any one of them can override these with its own "
                            <strong>"custom"</strong>
                            " timer."
                        </p>
                        <hr class="notif-divider" />
                        <TimerPicker
                            current=Signal::derive(move || global.get())
                            on_change=on_global_change
                        />
                    </section>

                    // Subscriptions + individually-starred matches in one list (a
                    // starred match carries a "match" tag, like a subscription's kind).
                    <section class="notif-section">
                        <ul class="notif-list">
                            {move || {
                                let mut keys: Vec<String> = subscribed.get().into_iter().collect();
                                keys.sort();
                                keys.into_iter().map(|k| view! { <SubRow key=k /> }).collect_view()
                            }}
                            // Transition (not Suspense) so re-fetching on a 12h/24h
                            // switch keeps the current rows visible instead of
                            // blanking the list (which made the page flash).
                            <Transition>
                                {move || {
                                    starred_matches
                                        .get()
                                        .map(|res| {
                                            res.unwrap_or_default()
                                                .upcoming
                                                .into_iter()
                                                .map(|m| view! { <StarredRow m=m /> })
                                                .collect_view()
                                        })
                                }}
                            </Transition>
                        </ul>
                    </section>
                    {move || {
                        let none = subscribed.with(HashSet::is_empty)
                            && starred.with(HashSet::is_empty);
                        none.then(|| {
                            view! {
                                <p class="empty">
                                    "Nothing yet. Subscribe to a sport, competition, event, or team "
                                    "from its page (the ★), or star an upcoming match to be reminded."
                                </p>
                            }
                        })
                    }}

                    // Export / import — transfer the whole set between devices.
                    <section class="notif-section notif-io">
                        <h2 class="notif-head">"Transfer to another device"</h2>
                        <p class="notif-note">
                            "Your notifications live in this browser. Click the code to copy it, "
                            "then import it on another device to carry them over."
                        </p>
                        // Click to copy. The code is truncated to one line; hovering
                        // reveals the whole thing in a popover.
                        <div
                            class="notif-io-copy"
                            role="button"
                            tabindex="0"
                            title="Click to copy"
                            on:click=copy_export
                            on:keydown=move |ev| {
                                if ev.key() == "Enter" || ev.key() == " " {
                                    ev.prevent_default();
                                    do_copy.run(());
                                }
                            }
                        >
                            <span class="notif-io-code">{move || export_str.get()}</span>
                            <span class="notif-io-pop">{move || export_str.get()}</span>
                        </div>
                        // Always present (a reserved line) so the hint appearing
                        // after a copy doesn't shift the layout.
                        <div class="notif-io-copy-status">{move || copy_hint.get()}</div>
                        <p class="notif-note">
                            "Or paste a code here, then "
                            <strong>"add"</strong>
                            " it to your current set or "
                            <strong>"override"</strong>
                            " to replace everything."
                        </p>
                        <input
                            class="notif-io-input"
                            type="text"
                            placeholder="paste a code…"
                            prop:value=move || import_text.get()
                            on:input=move |ev| import_text.set(event_target_value(&ev))
                        />
                        <div class="notif-io-actions">
                            <button class="chip" on:click=move |_| do_import.run(false)>
                                "add"
                            </button>
                            <button class="chip" on:click=move |_| do_import.run(true)>
                                "override"
                            </button>
                            <span class="notif-io-status">{move || import_status.get()}</span>
                        </div>
                    </section>
                }
                    .into_any()
            }}
        </article>
    }
}

/// One subscription row on the notifications page — its label (linking to the
/// team/event page where there is one) and a remove button.
#[component]
pub(crate) fn SubRow(key: String) -> impl IntoView {
    let subscribed = use_context::<Subscribed>().expect("subscribed context").0;
    let vapid = use_context::<RwSignal<Option<String>>>().expect("vapid context");
    let overrides = use_context::<Overrides>().expect("overrides context").0;
    // Keys are `sport|<slug>` or `<kind>|<slug>|<name>` (team/league/event are
    // sport-scoped, see `sub_key`, so their page link can be rebuilt here).
    let (kind, sport, value) = parse_sub_key(&key);
    // Re-arm this scope with the given lead list (its override or the global one),
    // called by its per-entry timer control.
    let rearm = {
        let kind = kind.clone();
        let value = value.clone();
        Callback::new(move |leads: Vec<i64>| {
            #[cfg(feature = "hydrate")]
            {
                let keys: Vec<String> = subscribed.with_untracked(|s| s.iter().cloned().collect());
                subscribe_scope(
                    kind.clone(),
                    value.clone(),
                    true,
                    keys,
                    vapid.get_untracked(),
                    leads,
                );
            }
            #[cfg(not(feature = "hydrate"))]
            {
                let _ = (&kind, &value, vapid, leads);
            }
        })
    };
    let entry_key = key.clone();
    // Label + optional link to the subscribed thing's own (sport-scoped) page.
    let (tag, label, link): (&str, String, Option<String>) = match kind.as_str() {
        "sport" => (
            "sport",
            sport.map_or_else(|| value.clone(), |g| g.label().to_string()),
            None,
        ),
        "team" => ("team", value.clone(), sport.map(|sp| team_path(sp, &value))),
        // A whole competition (MLB, F1, World Cup, LCK) — tagged with its best-fit
        // noun (league / tournament / series), distinct from a single event/edition
        // below; both link to the /event/ page for the name.
        "league" => (
            competition_kind(&value),
            value.clone(),
            sport.map(|sp| event_path(sp, &value)),
        ),
        _ => (
            "event",
            value.clone(),
            sport.map(|sp| event_path(sp, &value)),
        ),
    };

    let key_for_click = key.clone();
    let remove = move |_| {
        subscribed.update(|s| {
            s.remove(&key_for_click);
        });
        #[cfg(feature = "hydrate")]
        {
            // Drop any per-entry override for this scope too.
            overrides.update(|o| {
                o.remove(&key_for_click);
            });
            save_overrides(&overrides.get_untracked());
            let keys: Vec<String> = subscribed.with_untracked(|s| s.iter().cloned().collect());
            subscribe_scope(
                kind.clone(),
                value.clone(),
                false,
                keys,
                vapid.get_untracked(),
                Vec::new(),
            );
        }
        #[cfg(not(feature = "hydrate"))]
        {
            let _ = (&kind, &value, vapid, overrides, &key_for_click);
        }
    };

    let label_view = match link {
        Some(href) => view! { <A href=href>{label}</A> }.into_any(),
        None => label.into_any(),
    };

    view! {
        <li class="notif-item">
            <div class="notif-row">
                <span class="notif-kind">{tag}</span>
                <span class="notif-label">{label_view}</span>
                <EntryTimers entry_key=entry_key rearm=rearm />
                <button class="notif-remove" on:click=remove aria-label="Remove subscription">
                    "×"
                </button>
            </div>
        </li>
    }
}

/// One individually-starred upcoming match on the notifications page.
#[component]
pub(crate) fn StarredRow(m: MatchView) -> impl IntoView {
    let starred = use_context::<RwSignal<HashSet<String>>>().expect("starred context");
    let excluded = use_context::<Excluded>().expect("excluded context").0;
    let subscribed = use_context::<Subscribed>().expect("subscribed context").0;
    let vapid = use_context::<RwSignal<Option<String>>>().expect("vapid context");
    let overrides = use_context::<Overrides>().expect("overrides context").0;
    let id = m.id;
    let sport = m.sport;
    let uid = m.uid();
    // Re-arm this match with the given lead list, called by its timer control.
    let rearm = Callback::new(move |leads: Vec<i64>| {
        #[cfg(feature = "hydrate")]
        sync_match_reminder(id, sport, true, false, vapid.get_untracked(), leads);
        #[cfg(not(feature = "hydrate"))]
        {
            let _ = (id, sport, vapid, leads);
        }
    });
    let entry_key = uid.clone();
    // F1 (single-entity) has no opponent: show the GP and session (e.g.
    // "Austrian Grand Prix · Race") instead of a "Race at " matchup.
    let line = if m.sport.single_entity() {
        if m.series_name.is_empty() {
            m.team_a.label.clone()
        } else {
            format!("{} · {}", m.series_name, m.team_a.label)
        }
    } else {
        format!(
            "{} {} {}",
            m.team_a.label,
            versus_sep(m.sport),
            m.team_b.label
        )
    };
    // The scopes that also cover this match — so removing it actually stops the
    // notification (opt out) rather than just dropping a redundant star.
    let keys = [
        sub_key("sport", m.sport, m.sport.slug()),
        sub_key("league", m.sport, &m.league),
        sub_key("team", m.sport, &m.team_a.name),
        sub_key("team", m.sport, &m.team_b.name),
        sub_key(
            "event",
            m.sport,
            &full_event_name(&m.league, &m.series_name),
        ),
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
                // Drop any per-entry override for this match too.
                overrides.update(|o| {
                    o.remove(&uid);
                });
                save_overrides(&overrides.get_untracked());
                save_starred(&starred.get_untracked());
                save_excluded(&excluded.get_untracked());
                sync_match_reminder(id, sport, false, covered, vapid.get_untracked(), Vec::new());
            }
            #[cfg(not(feature = "hydrate"))]
            {
                let _ = (covered, vapid, sport, id, overrides);
            }
        }
    };

    view! {
        <li class="notif-item">
            <div class="notif-row">
                <span class="notif-kind">"match"</span>
                <A href=crate::types::match_path(sport, id)>
                    <span class="notif-time">{m.clock_label}</span>
                    <span class="notif-label">{line}</span>
                </A>
                <EntryTimers entry_key=entry_key rearm=rearm />
                <button class="notif-remove" on:click=remove aria-label="Remove reminder">
                    "×"
                </button>
            </div>
        </li>
    }
}

/// Re-arm a batch of entries server-side under a single push handshake:
/// optionally clear everything first (import "override"), then (re)subscribe each
/// scope, (re)arm each starred match, and re-apply each opt-out — all with the
/// caller's already-resolved lead lists. Backs global-timer edits and import.
#[cfg(feature = "hydrate")]
pub(crate) fn sync_entries(
    clear_first: bool,
    subs: Vec<(String, String, Vec<i64>)>,
    stars: Vec<(i64, String, Vec<i64>)>,
    excludes: Vec<(i64, String)>,
    vapid: Option<String>,
) {
    let Some(vapid) = vapid else { return };
    if !clear_first && subs.is_empty() && stars.is_empty() && excludes.is_empty() {
        return;
    }
    // The viewer's zone + clock format, so the server bakes each body's start
    // time locally in the format the rest of the UI uses.
    let tz = detect_tz().unwrap_or_default();
    let hour24 = load_hour24_pref().unwrap_or(false);
    leptos::task::spawn_local(async move {
        // A user action, so it's fine to prompt + create a subscription.
        let sub = match pte_subscribe(&vapid).await {
            Ok(s) => s,
            Err(_) => return,
        };
        let Some(push) = push_from_js(&sub) else {
            return;
        };
        arm_entries(push, clear_first, subs, stars, excludes, tz, hour24).await;
    });
}

/// Arm exactly `subs`/`stars`/`excludes` for an already-resolved push
/// subscription, optionally clearing this endpoint's server state first (a clean
/// replace). Shared by the user-action sync (which prompts + subscribes) and the
/// on-load reconcile (which uses the existing subscription only). Runs inside a
/// caller's `spawn_local`. `clear_notifications` keeps already-sent reminders, so
/// a replace never re-fires a delivered one.
#[cfg(feature = "hydrate")]
pub(crate) async fn arm_entries(
    push: crate::types::PushSub,
    clear_first: bool,
    subs: Vec<(String, String, Vec<i64>)>,
    stars: Vec<(i64, String, Vec<i64>)>,
    excludes: Vec<(i64, String)>,
    tz: String,
    hour24: bool,
) {
    if clear_first {
        let _ = crate::server::clear_notifications(push.endpoint.clone()).await;
    }
    for (kind, value, leads) in subs {
        let _ = crate::server::add_subscription(crate::types::SubscribeReq {
            sub: push.clone(),
            kind,
            value,
            leads,
            tz: tz.clone(),
            hour24,
        })
        .await;
    }
    for (match_id, sport, leads) in stars {
        let _ = crate::server::add_reminder(crate::types::ReminderReq {
            sub: push.clone(),
            match_id,
            sport,
            leads,
            tz: tz.clone(),
            hour24,
        })
        .await;
    }
    for (match_id, sport) in excludes {
        // Exclusions don't carry a body, so the format is irrelevant.
        let _ = crate::server::exclude_reminder(crate::types::ReminderReq {
            sub: push.clone(),
            match_id,
            sport,
            leads: Vec::new(),
            tz: String::new(),
            hour24: false,
        })
        .await;
    }
}

/// On load, bring the server's state for this browser's push endpoint into line
/// with localStorage (the source of truth), so the two never drift — e.g. after
/// the DB was cleared, or a star/timer was changed on another device. Uses the
/// *existing* subscription only, so it never prompts a user who hasn't opted in;
/// if there's none (no permission yet), it's a no-op. The replace is keyed by the
/// endpoint and idempotent, and the arming gate means only still-ahead timers are
/// re-armed (no retroactive burst).
#[cfg(feature = "hydrate")]
pub(crate) fn reconcile_notifications(
    subscribed: HashSet<String>,
    starred: HashSet<String>,
    excluded: HashSet<String>,
    global: Vec<i64>,
    overrides: BTreeMap<String, Vec<i64>>,
    vapid: Option<String>,
) {
    // Push must be configured for any of this to mean anything.
    if vapid.is_none() {
        return;
    }
    // Re-arm with the current zone + clock format, so a since-changed 12h/24h
    // preference re-renders existing reminders' bodies on the next load.
    let tz = detect_tz().unwrap_or_default();
    let hour24 = load_hour24_pref().unwrap_or(false);
    // Resolve each entry's effective leads (its override else the global list) —
    // the same shape the import/global-edit paths arm with.
    let subs_re: Vec<(String, String, Vec<i64>)> = subscribed
        .iter()
        .map(|k| {
            let (kind, _s, value) = parse_sub_key(k);
            (kind, value, effective_leads(k, &overrides, &global))
        })
        .collect();
    let stars_re: Vec<(i64, String, Vec<i64>)> = starred
        .iter()
        .filter_map(|u| {
            parse_uid(u).map(|(sp, id)| {
                (
                    id,
                    sp.slug().to_string(),
                    effective_leads(u, &overrides, &global),
                )
            })
        })
        .collect();
    let excl_re: Vec<(i64, String)> = excluded
        .iter()
        .filter_map(|u| parse_uid(u).map(|(sp, id)| (id, sp.slug().to_string())))
        .collect();
    leptos::task::spawn_local(async move {
        let sub = match pte_existing_sub().await {
            Ok(s) => s,
            Err(_) => return,
        };
        // `null` (no permission / no subscription yet) ⇒ nothing to reconcile, and
        // crucially no prompt.
        let Some(push) = push_from_js(&sub) else {
            return;
        };
        arm_entries(push, true, subs_re, stars_re, excl_re, tz, hour24).await;
    });
}

/// Persist the subscribed set and (un)register the sport/event subscription with
/// its effective lead offsets (its override else the global list).
#[cfg(feature = "hydrate")]
pub(crate) fn subscribe_scope(
    kind: String,
    value: String,
    subscribing: bool,
    keys: Vec<String>,
    vapid: Option<String>,
    leads: Vec<i64>,
) {
    save_subs(&keys);
    let Some(vapid) = vapid else { return };
    // The viewer's zone + clock format, stored with the subscription so expansion
    // bakes each match's reminder body in their local time and chosen format.
    let tz = detect_tz().unwrap_or_default();
    let hour24 = load_hour24_pref().unwrap_or(false);
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
                leads,
                tz,
                hour24,
            })
            .await;
        } else {
            let _ = crate::server::remove_subscription(endpoint, kind, value).await;
        }
    });
}

#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen]
extern "C" {
    // Defined in the shell <script>: registers the SW, requests permission,
    // subscribes, and returns `{ endpoint, p256dh, auth }`.
    #[wasm_bindgen(catch, js_name = pteSubscribe)]
    async fn pte_subscribe(vapid: &str) -> Result<wasm_bindgen::JsValue, wasm_bindgen::JsValue>;

    // Like pteSubscribe but never prompts/creates: returns the existing
    // subscription `{ endpoint, p256dh, auth }`, or `null` if there isn't one.
    #[wasm_bindgen(catch, js_name = pteExistingSub)]
    async fn pte_existing_sub() -> Result<wasm_bindgen::JsValue, wasm_bindgen::JsValue>;
}

#[cfg(feature = "hydrate")]
pub(crate) fn reflect_str(obj: &wasm_bindgen::JsValue, key: &str) -> Option<String> {
    js_sys::Reflect::get(obj, &wasm_bindgen::JsValue::from_str(key))
        .ok()?
        .as_string()
}

/// Build a [`PushSub`](crate::types::PushSub) from a `{ endpoint, p256dh, auth }`
/// JS object (`pteSubscribe`/`pteExistingSub`'s return). `None` if any field is
/// missing — e.g. `pteExistingSub` returned `null` (no permission/subscription).
#[cfg(feature = "hydrate")]
pub(crate) fn push_from_js(sub: &wasm_bindgen::JsValue) -> Option<crate::types::PushSub> {
    Some(crate::types::PushSub {
        endpoint: reflect_str(sub, "endpoint")?,
        p256dh: reflect_str(sub, "p256dh")?,
        auth: reflect_str(sub, "auth")?,
    })
}

/// Persist the starred set and (un)register the reminder on the server. The
/// server derives the notification details from `match_id`, so we send just that.
#[cfg(feature = "hydrate")]
pub(crate) fn sync_match_reminder(
    match_id: i64,
    sport: Sport,
    want_on: bool,
    covered: bool,
    vapid: Option<String>,
    leads: Vec<i64>,
) {
    let Some(vapid) = vapid else { return };
    let sport = sport.slug().to_string();
    // The viewer's zone + clock format, so the server bakes the body's start time
    // locally in the format the rest of the UI uses.
    let tz = detect_tz().unwrap_or_default();
    let hour24 = load_hour24_pref().unwrap_or(false);
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
            // Arm one reminder per lead offset (and clear any opt-out server-side).
            let _ = crate::server::add_reminder(crate::types::ReminderReq {
                sub: push,
                match_id,
                sport,
                leads,
                tz,
                hour24,
            })
            .await;
        } else if covered {
            // Opt this match out of its covering subscription, keeping the scope.
            let _ = crate::server::exclude_reminder(crate::types::ReminderReq {
                sub: push,
                match_id,
                sport,
                leads: Vec::new(),
                tz: String::new(),
                hour24: false,
            })
            .await;
        } else {
            // Plain un-star: drop every reminder row for the match.
            let _ = crate::server::remove_reminder(endpoint, match_id, sport).await;
        }
    });
}

/// Clear every subscription + pending reminder for this browser's push endpoint.
#[cfg(feature = "hydrate")]
pub(crate) fn clear_all_notifications(vapid: Option<String>) {
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

    #[test]
    fn backup_round_trips_through_the_export_string() {
        let mut overrides = vec![
            ("sport|cs2".to_string(), vec![60_000, 900_000]),
            ("lol-99".to_string(), vec![0]),
        ];
        overrides.sort();
        let b = NotifBackup {
            subs: vec![
                "sport|cs2".to_string(),
                "team|mlb|Detroit Tigers".to_string(),
            ],
            starred: vec!["lol-99".to_string()],
            excluded: vec!["mlb-7".to_string()],
            timers: vec![900_000, 86_400_000],
            overrides,
        };
        let decoded = decode_backup(&encode_backup(&b)).expect("decodes");
        assert_eq!(decoded, b);
        // Tab/space-bearing keys (team names) survive the framing.
        assert!(decoded
            .subs
            .contains(&"team|mlb|Detroit Tigers".to_string()));
        // A bad prefix is rejected.
        assert!(decode_backup("nope").is_none());
        assert!(decode_backup("pte1:!!!notbase64").is_none());
    }
}
