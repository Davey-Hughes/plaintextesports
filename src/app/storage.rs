//! Hydrate-only `localStorage` / cookie read-write glue for every persisted
//! preference and reminder set. Keys live in [`super::keys`]; these are the
//! load/save pairs the App effect and the toggles call.
use crate::app::*;

#[cfg(feature = "hydrate")]
pub(crate) fn detect_tz() -> Option<String> {
    use wasm_bindgen::JsValue;
    let fmt = js_sys::Intl::DateTimeFormat::new(&js_sys::Array::new(), &js_sys::Object::new());
    let opts = fmt.resolved_options();
    js_sys::Reflect::get(&opts, &JsValue::from_str("timeZone"))
        .ok()
        .and_then(|v| v.as_string())
        .filter(|s| !s.is_empty())
}

#[cfg(feature = "hydrate")]
pub(crate) fn load_hour24_pref() -> Option<bool> {
    let storage = web_sys::window()?.local_storage().ok().flatten()?;
    let v = storage.get_item(keys::HOUR24).ok().flatten()?;
    Some(v != "0")
}

/// The viewer's clock format for notification arming: their saved localStorage
/// preference, else the app default ([`DEFAULT_HOUR24`], 24h). This must match the
/// display seed ([`initial_hour24`]) so a push notification renders in the same
/// clock the schedule shows — a viewer on the untouched default gets 24h in both,
/// not 12h. (Bare `load_hour24_pref()` returns `None` when unset; don't default it
/// to 12h.)
#[cfg(feature = "hydrate")]
pub(crate) fn effective_hour24() -> bool {
    load_hour24_pref().unwrap_or(DEFAULT_HOUR24)
}

#[cfg(feature = "hydrate")]
pub(crate) fn save_hour24_pref(hour24: bool) {
    let val = if hour24 { "1" } else { "0" };
    if let Some(win) = web_sys::window() {
        if let Ok(Some(storage)) = win.local_storage() {
            let _ = storage.set_item(keys::HOUR24, val);
        }
    }
    // Mirror to a cookie so the next SSR render seeds the same value (and the
    // schedule resource doesn't re-key after hydration). See [`initial_hour24`].
    write_cookie(HOUR24_COOKIE_KEY, val);
}

#[cfg(feature = "hydrate")]
pub(crate) fn load_scores_pref() -> bool {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item(keys::SCORES).ok().flatten())
        .is_some_and(|v| v == "1")
}

/// Read a cookie value by name (client-side).
#[cfg(feature = "hydrate")]
pub(crate) fn read_cookie(name: &str) -> Option<String> {
    use wasm_bindgen::JsValue;
    let doc = web_sys::window()?.document()?;
    let cookies = js_sys::Reflect::get(&doc, &JsValue::from_str("cookie"))
        .ok()?
        .as_string()?;
    cookies.split(';').find_map(|kv| {
        let (k, v) = kv.trim().split_once('=')?;
        (k == name).then(|| v.to_string())
    })
}

/// Write a long-lived, root-scoped cookie (client-side).
#[cfg(feature = "hydrate")]
pub(crate) fn write_cookie(name: &str, value: &str) {
    use wasm_bindgen::JsValue;
    if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
        // ~1 year; lax so it rides top-level navigations (shared links).
        let v = format!("{name}={value}; path=/; max-age=31536000; samesite=lax");
        let _ = js_sys::Reflect::set(&doc, &JsValue::from_str("cookie"), &JsValue::from_str(&v));
    }
}

#[cfg(feature = "hydrate")]
pub(crate) fn save_sport_pref(traditional: bool) {
    write_cookie(
        SPORT_MODE_KEY,
        if traditional { "sports" } else { "esports" },
    );
}

/// Apply the scores preference: set `data-scores` on <html> (drives the scores
/// toggle's on-state, set pre-paint to avoid a flash) and persist it.
#[cfg(feature = "hydrate")]
pub(crate) fn apply_scores(show: bool) {
    let val = if show { "1" } else { "0" };
    if let Some(win) = web_sys::window() {
        if let Some(root) = win.document().and_then(|d| d.document_element()) {
            let _ = root.set_attribute("data-scores", val);
        }
        if let Ok(Some(storage)) = win.local_storage() {
            let _ = storage.set_item(keys::SCORES, val);
        }
    }
}

#[cfg(feature = "hydrate")]
pub(crate) fn save_subs(keys: &[String]) {
    if let Some(win) = web_sys::window() {
        if let Ok(Some(storage)) = win.local_storage() {
            let _ = storage.set_item(keys::SUBS, &keys.join("\n"));
        }
    }
}

#[cfg(feature = "hydrate")]
pub(crate) fn load_subs() -> HashSet<String> {
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

#[cfg(feature = "hydrate")]
pub(crate) fn save_timers(timers: &[i64]) {
    if let Some(win) = web_sys::window() {
        if let Ok(Some(storage)) = win.local_storage() {
            let _ = storage.set_item(keys::TIMERS, &join_lead_list(timers));
        }
    }
}

/// The saved global timers — `None` if the key was never written (use the config
/// default), `Some([])` if the user explicitly cleared them all.
#[cfg(feature = "hydrate")]
pub(crate) fn load_timers() -> Option<Vec<i64>> {
    let storage = web_sys::window()?.local_storage().ok().flatten()?;
    let v = storage.get_item(keys::TIMERS).ok().flatten()?;
    Some(parse_lead_list(&v))
}

#[cfg(feature = "hydrate")]
pub(crate) fn save_overrides(ov: &BTreeMap<String, Vec<i64>>) {
    if let Some(win) = web_sys::window() {
        if let Ok(Some(storage)) = win.local_storage() {
            let body = ov
                .iter()
                .map(|(k, v)| format!("{k}\t{}", join_lead_list(v)))
                .collect::<Vec<_>>()
                .join("\n");
            let _ = storage.set_item(keys::OVERRIDES, &body);
        }
    }
}

#[cfg(feature = "hydrate")]
pub(crate) fn load_overrides() -> BTreeMap<String, Vec<i64>> {
    let mut out = BTreeMap::new();
    if let Some(win) = web_sys::window() {
        if let Ok(Some(storage)) = win.local_storage() {
            if let Ok(Some(v)) = storage.get_item(keys::OVERRIDES) {
                for line in v.split('\n').filter(|l| !l.is_empty()) {
                    if let Some((k, list)) = line.split_once('\t') {
                        out.insert(k.to_string(), parse_lead_list(list));
                    }
                }
            }
        }
    }
    out
}

#[cfg(feature = "hydrate")]
pub(crate) fn save_range(range: Option<(String, String)>) {
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
pub(crate) fn load_range() -> Option<(String, String)> {
    let storage = web_sys::window()?.local_storage().ok().flatten()?;
    let v = storage.get_item(keys::RANGE).ok().flatten()?;
    let (s, e) = v.split_once('|')?;
    (!s.is_empty() && !e.is_empty()).then(|| (s.to_string(), e.to_string()))
}

// Starred / excluded / revealed are sets of match uids ("{sport}-{id}"), persisted
// as comma-separated strings via the shared string-set helpers below.
#[cfg(feature = "hydrate")]
pub(crate) fn save_starred(uids: &HashSet<String>) {
    save_str_set(keys::STARRED, uids);
}

#[cfg(feature = "hydrate")]
pub(crate) fn load_starred() -> HashSet<String> {
    load_str_set(keys::STARRED)
}

#[cfg(feature = "hydrate")]
pub(crate) fn save_excluded(uids: &HashSet<String>) {
    save_str_set(keys::EXCLUDED, uids);
}

#[cfg(feature = "hydrate")]
pub(crate) fn load_excluded() -> HashSet<String> {
    load_str_set(keys::EXCLUDED)
}

/// Current wall-clock time (ms since epoch) from the browser, for stamping and
/// pruning reveal records.
#[cfg(feature = "hydrate")]
pub(crate) fn now_ms() -> i64 {
    js_sys::Date::now() as i64
}

/// Load a reveal store (`revealed` or `sections`), pruning any record that's been
/// both finished and untouched for over a week, persisting the survivors (which
/// also migrates the legacy key-only format in place), and returning the surviving
/// keys to seed the in-memory reveal context.
#[cfg(feature = "hydrate")]
pub(crate) fn prune_and_load(storage_key: &str, now: i64) -> HashSet<String> {
    let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) else {
        return HashSet::new();
    };
    let raw = storage
        .get_item(storage_key)
        .ok()
        .flatten()
        .unwrap_or_default();
    let recs = crate::reveal::prune_records(
        crate::reveal::parse_records(&raw, now),
        now,
        crate::reveal::REVEAL_TTL_MS,
    );
    let _ = storage.set_item(storage_key, &crate::reveal::serialize_records(&recs));
    crate::reveal::keys(&recs)
}

/// Upsert a batch of reveals (toggle-on or view): set each key's end and stamp
/// `touched = now`, in a single read-modify-write.
#[cfg(feature = "hydrate")]
pub(crate) fn touch_reveals(storage_key: &str, items: &[(String, i64)], now: i64) {
    if items.is_empty() {
        return;
    }
    let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) else {
        return;
    };
    let raw = storage
        .get_item(storage_key)
        .ok()
        .flatten()
        .unwrap_or_default();
    let mut recs = crate::reveal::parse_records(&raw, now);
    for (key, end_ms) in items {
        crate::reveal::upsert(&mut recs, key, *end_ms, now);
    }
    let _ = storage.set_item(storage_key, &crate::reveal::serialize_records(&recs));
}

/// Drop one reveal (toggle-off / hide).
#[cfg(feature = "hydrate")]
pub(crate) fn remove_reveal(storage_key: &str, key: &str) {
    let now = now_ms();
    let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) else {
        return;
    };
    let raw = storage
        .get_item(storage_key)
        .ok()
        .flatten()
        .unwrap_or_default();
    let mut recs = crate::reveal::parse_records(&raw, now);
    crate::reveal::remove(&mut recs, key);
    let _ = storage.set_item(storage_key, &crate::reveal::serialize_records(&recs));
}

/// Persist a set of strings under `key` (newline-separated).
#[cfg(feature = "hydrate")]
pub(crate) fn save_str_set(key: &str, set: &HashSet<String>) {
    if let Some(win) = web_sys::window() {
        if let Ok(Some(storage)) = win.local_storage() {
            let joined = set.iter().cloned().collect::<Vec<_>>().join("\n");
            let _ = storage.set_item(key, &joined);
        }
    }
}

#[cfg(feature = "hydrate")]
pub(crate) fn load_str_set(key: &str) -> HashSet<String> {
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
