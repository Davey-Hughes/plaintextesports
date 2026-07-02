//! Small shared free functions: URL-segment & path encoding, team/logo view
//! cells, league colours, sport-mode resolution, date math, and app-bootstrap
//! initial-state readers. Moved out of `super` (the App shell) verbatim.
use crate::app::*;

/// The sport-mode cookie key (and `?mode` query param). Unlike the other storage
/// keys it's read on the server too (to render the right mode on the first paint),
/// so it lives outside the hydrate-only `keys` module.
#[cfg(any(feature = "ssr", feature = "hydrate"))]
pub(crate) const SPORT_MODE_KEY: &str = "mode";

/// The detected-timezone cookie key (browser IANA zone). Read on the server too so
/// the first paint already renders in the viewer's zone — without it the schedule
/// resource re-keys (tz default "" → real zone) right after hydration and refetches
/// the whole schedule. Lives beside [`SPORT_MODE_KEY`] for the same reason.
#[cfg(any(feature = "ssr", feature = "hydrate"))]
pub(crate) const TZ_COOKIE_KEY: &str = "tz";

/// The 24-hour-clock cookie key ("1"/"0"). Mirrors the localStorage [`keys::HOUR24`]
/// preference into a cookie so the server can seed it on the first paint (same
/// re-key-and-refetch avoidance as [`TZ_COOKIE_KEY`]).
#[cfg(any(feature = "ssr", feature = "hydrate"))]
pub(crate) const HOUR24_COOKIE_KEY: &str = "h24";

/// Read a cookie value from the SSR request's `Cookie` header.
#[cfg(feature = "ssr")]
pub(crate) fn ssr_cookie(parts: &http::request::Parts, key: &str) -> Option<String> {
    parts
        .headers
        .get(http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|cookies| {
            cookies.split(';').find_map(|kv| {
                let (k, v) = kv.trim().split_once('=')?;
                (k == key).then(|| v.to_string())
            })
        })
}

/// Update the URL hash to the `.spy` section currently scrolled into view (its
/// `id` becomes the hash) via `replaceState`, so it neither scrolls nor stacks
/// history. Targets are the homepage day groups and the event page's sections.
#[cfg(feature = "hydrate")]
pub(crate) fn scrollspy_update() {
    use wasm_bindgen::JsCast;
    let Some(win) = web_sys::window() else {
        return;
    };
    let Some(doc) = win.document() else {
        return;
    };
    let Ok(nodes) = doc.query_selector_all(".spy[id]") else {
        return;
    };
    // Sections come in document order. The active one spans the line just below
    // the sticky header (matching the jump anchors' scroll-margin); if that line
    // falls in a gap, keep the section above it.
    let trigger = 80.0;
    let mut last_above = String::new();
    let mut contained = String::new();
    for i in 0..nodes.length() {
        let Some(el) = nodes
            .item(i)
            .and_then(|n| n.dyn_into::<web_sys::Element>().ok())
        else {
            continue;
        };
        let r = el.get_bounding_client_rect();
        if r.top() <= trigger {
            last_above = el.id();
            if r.bottom() > trigger {
                contained = last_above.clone();
            }
        }
    }
    let active = if contained.is_empty() {
        last_above
    } else {
        contained
    };
    let loc = win.location();
    let want = if active.is_empty() {
        String::new()
    } else {
        format!("#{active}")
    };
    if loc.hash().unwrap_or_default() == want {
        return;
    }
    let url = if want.is_empty() {
        // Strip the hash, keeping the path + query.
        format!(
            "{}{}",
            loc.pathname().unwrap_or_default(),
            loc.search().unwrap_or_default()
        )
    } else {
        want
    };
    if let Ok(hist) = win.history() {
        let _ = hist.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&url));
    }
}

/// Install a global, rAF-throttled scroll listener that keeps the URL hash in
/// sync with the visible `.spy` section. Lives for the app's lifetime.
#[cfg(feature = "hydrate")]
pub(crate) fn setup_scrollspy() {
    use std::cell::Cell;
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::JsCast;
    thread_local! {
        static TICKING: Cell<bool> = const { Cell::new(false) };
    }
    let Some(win) = web_sys::window() else {
        return;
    };
    let cb = Closure::<dyn FnMut()>::new(|| {
        if !TICKING.with(Cell::get) {
            TICKING.with(|t| t.set(true));
            request_animation_frame(|| {
                scrollspy_update();
                TICKING.with(|t| t.set(false));
            });
        }
    });
    let _ = win.add_event_listener_with_callback("scroll", cb.as_ref().unchecked_ref());
    cb.forget();
}

/// If the match's schedule row is on the page, scroll to it and flash it; returns
/// whether it was found (so a link can suppress its own navigation). Shared by the
/// bracket team links and the "up next" bar.
#[cfg(feature = "hydrate")]
pub(crate) fn flash_match_row(uid: &str) -> bool {
    let Some(row) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(&format!("m-{uid}")))
    else {
        return false;
    };
    row.scroll_into_view();
    // Remove + reflow + re-add restarts the animation on a repeat click.
    let cl = row.class_list();
    let _ = cl.remove_1("row-flash");
    let _ = row.client_width();
    let _ = cl.add_1("row-flash");
    true
}

/// Percent-encode a league name for use as a URL path segment.
pub(crate) fn enc_segment(s: &str) -> String {
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
pub(crate) fn dec_segment(s: &str) -> String {
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

/// The team page path, `"/team/{slug}/{name}"`. The sport segment scopes the
/// URL; the page lookup is still keyed by `name` alone.
pub(crate) fn team_path(sport: Sport, name: &str) -> String {
    format!("/team/{}/{}", sport.slug(), enc_segment(name))
}

/// The event page path, `"/event/{slug}/{name}"`. Sport-scoped, name-keyed.
pub(crate) fn event_path(sport: Sport, name: &str) -> String {
    format!("/event/{}/{}", sport.slug(), enc_segment(name))
}

/// A team name rendered as a link to its team page — or plain text when the
/// opponent isn't a real team yet (TBD / empty name). `name` keys the page;
/// `sport` scopes the URL (see [`team_path`]).
pub(crate) fn team_link(sport: Sport, label: String, name: String) -> AnyView {
    if name.is_empty() || name == "TBD" {
        label.into_any()
    } else {
        view! { <A href=team_path(sport, &name)>{label}</A> }.into_any()
    }
}

/// The vector logo for `name` found among a schedule's matches (it appears as
/// either side), or empty when none of its matches carry one.
pub(crate) fn team_logo_in_schedule(s: &ScheduleView, name: &str) -> String {
    for d in &s.days {
        for lg in &d.leagues {
            for m in &lg.matches {
                if m.team_a.name == name && !m.team_a.logo.is_empty() {
                    return m.team_a.logo.clone();
                }
                if m.team_b.name == name && !m.team_b.logo.is_empty() {
                    return m.team_b.logo.clone();
                }
            }
        }
    }
    String::new()
}

/// A team's vector (SVG) logo as a small `<img>`, or nothing when there's no
/// vector source for it (most leagues — only NHL/MLB expose one). `extra` is an
/// extra class for sizing per context.
pub(crate) fn team_logo(logo: &str, extra: &str) -> AnyView {
    if logo.is_empty() {
        return ().into_any();
    }
    view! { <img class=format!("team-logo {extra}") src=logo.to_string() alt="" loading="lazy" /> }
        .into_any()
}

/// An F1 constructor's logo rendered as a `currentColor` CSS mask of F1's official
/// 2026 white silhouette, so it tracks the theme's foreground and stays visible on
/// every theme (the colour artwork is near-black for some teams). Nothing when the
/// constructor has no mapped logo. Keeps the `team-logo` class so the global icons
/// toggle hides it alongside the raster logos.
pub(crate) fn clogo_icon(logo: &str) -> AnyView {
    if logo.is_empty() {
        return ().into_any();
    }
    view! { <span class="team-logo f1-clogo" style=format!("--clogo:url('{logo}')")></span> }
        .into_any()
}

/// A team's name for a compact row: its label, with the official abbreviation as a
/// second copy that CSS swaps in where the row is too narrow for the full label
/// (see `.tname`/`.tabbr`). When the source has no abbreviation (esports, whose
/// label is already an acronym) it's just the label.
pub(crate) fn team_cell(label: String, abbrev: String) -> AnyView {
    if abbrev.is_empty() {
        view! { {label} }.into_any()
    } else {
        let full = label.clone();
        view! {
            <span title=full>
                <span class="tname">{label}</span>
                <span class="tabbr">{abbrev}</span>
            </span>
        }
        .into_any()
    }
}

/// The persisted subscription key for a scope. A whole-sport sub keys by slug
/// alone (`sport|cs2`); a team/league/event sub is sport-scoped
/// (`team|mlb|Detroit Tigers`) so the notifications page can rebuild its
/// (now sport-scoped) page link. The bare `value` is still what's sent to — and
/// matched by — the server; the sport only enriches the client-side key.
pub(crate) fn sub_key(kind: &str, sport: Sport, value: &str) -> String {
    if kind == "sport" {
        format!("sport|{value}")
    } else {
        format!("{kind}|{}|{value}", sport.slug())
    }
}

/// Inverse of [`sub_key`]: split a persisted subscription key into its kind, the
/// sport it's scoped to (if any — a `sport|<slug>` key resolves to that sport),
/// and the bare `value` (the name) the server matches on. Drives the
/// notifications page's label + (sport-scoped) page link.
pub(crate) fn parse_sub_key(key: &str) -> (String, Option<Sport>, String) {
    let mut parts = key.splitn(3, '|');
    let kind = parts.next().unwrap_or_default().to_string();
    let a = parts.next().unwrap_or_default().to_string();
    match (kind.as_str(), parts.next()) {
        // `sport|<slug>` — the value is the slug itself; no extra name.
        ("sport", _) => (kind, Sport::from_filter(&a), a),
        // `<kind>|<slug>|<name>` — name keys the page, slug scopes its URL.
        (_, Some(name)) => (kind, Sport::from_filter(&a), name.to_string()),
        // Defensive: a sport-less non-sport key — show the label, but no link.
        (_, None) => (kind, None, a),
    }
}

/// Resolve the initial sport mode (`true` = traditional sports) from the page's
/// inputs. Shared by SSR (request URL + `Cookie` header) and hydrate (location +
/// `document.cookie`) so both render the same mode — no first-paint flash and no
/// hydration mismatch. Precedence: an explicit `?mode` wins (a shared link); else
/// infer from a known sport slug in `?g` (e.g. `?g=nhl` ⇒ sports); else the saved
/// `mode` cookie ("sports"/"esports"), defaulting to esports.
#[cfg(any(feature = "ssr", feature = "hydrate"))]
pub(crate) fn resolve_sport_mode(
    mode_param: Option<&str>,
    g_param: Option<&str>,
    cookie_mode: Option<&str>,
) -> bool {
    if let Some(m) = mode_param.filter(|v| !v.is_empty()) {
        return m == "sports";
    }
    if let Some(v) = g_param.filter(|v| !v.is_empty()) {
        let known: Vec<Sport> = v.split(',').filter_map(Sport::from_filter).collect();
        if !known.is_empty() {
            return known.iter().any(|g| g.traditional());
        }
    }
    cookie_mode == Some("sports")
}

/// The first `key`'s value in an `&`-separated query string (values stay URL-
/// encoded; the params we read — `mode`/`g` — are ASCII slugs).
#[cfg(any(feature = "ssr", feature = "hydrate"))]
pub(crate) fn query_param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then(|| v.to_string())
    })
}

/// The initial sport mode for this render, read server-side from the request so
/// the first paint already matches the client (see [`resolve_sport_mode`]).
#[cfg(feature = "ssr")]
pub(crate) fn initial_sport_mode() -> bool {
    let Some(parts) = use_context::<http::request::Parts>() else {
        return false;
    };
    let query = parts.uri.query().unwrap_or_default();
    resolve_sport_mode(
        query_param(query, SPORT_MODE_KEY).as_deref(),
        query_param(query, "g").as_deref(),
        ssr_cookie(&parts, SPORT_MODE_KEY).as_deref(),
    )
}

/// The timezone IANA name for this render, read from the `tz` cookie so SSR and
/// hydrate seed the schedule resource with the same value (the browser-detected
/// zone is later confirmed/updated by the post-hydration effect). Empty until the
/// browser has reported one (the server then falls back to the configured default).
#[cfg(feature = "ssr")]
pub(crate) fn initial_tz() -> String {
    use_context::<http::request::Parts>()
        .and_then(|parts| ssr_cookie(&parts, TZ_COOKIE_KEY))
        .unwrap_or_default()
}

#[cfg(feature = "hydrate")]
pub(crate) fn initial_tz() -> String {
    read_cookie(TZ_COOKIE_KEY).unwrap_or_default()
}

#[cfg(not(any(feature = "ssr", feature = "hydrate")))]
pub(crate) fn initial_tz() -> String {
    String::new()
}

/// The 24-hour-clock preference for this render, read from the `h24` cookie so SSR
/// and hydrate agree on the first paint. Defaults to true (24h); the localStorage
/// pref still takes over (and back-fills the cookie) in the post-hydration effect.
#[cfg(feature = "ssr")]
pub(crate) fn initial_hour24() -> bool {
    use_context::<http::request::Parts>()
        .and_then(|parts| ssr_cookie(&parts, HOUR24_COOKIE_KEY))
        .is_none_or(|v| v != "0")
}

#[cfg(feature = "hydrate")]
pub(crate) fn initial_hour24() -> bool {
    read_cookie(HOUR24_COOKIE_KEY).is_none_or(|v| v != "0")
}

#[cfg(not(any(feature = "ssr", feature = "hydrate")))]
pub(crate) fn initial_hour24() -> bool {
    true
}

#[cfg(feature = "hydrate")]
pub(crate) fn initial_sport_mode() -> bool {
    let search = web_sys::window()
        .and_then(|w| w.location().search().ok())
        .unwrap_or_default();
    let query = search.trim_start_matches('?');
    resolve_sport_mode(
        query_param(query, SPORT_MODE_KEY).as_deref(),
        query_param(query, "g").as_deref(),
        read_cookie(SPORT_MODE_KEY).as_deref(),
    )
}

#[cfg(not(any(feature = "ssr", feature = "hydrate")))]
pub(crate) fn initial_sport_mode() -> bool {
    false
}

/// The VAPID public key for this render, if Web Push is configured. Read from the
/// server env during SSR (and to embed in the shell `<meta>`), and from that same
/// `<meta name="pte-vapid">` during hydrate — so both renders agree and the
/// subscribe ★s are present on the first paint instead of popping in after a
/// `get_vapid_key` round-trip. `None` keeps them hidden.
#[cfg(feature = "ssr")]
pub(crate) fn initial_vapid() -> Option<String> {
    let cfg = crate::config::Config::from_env();
    cfg.push_enabled().then_some(cfg.vapid_public)
}

#[cfg(feature = "hydrate")]
pub(crate) fn initial_vapid() -> Option<String> {
    web_sys::window()?
        .document()?
        .query_selector("meta[name=\"pte-vapid\"]")
        .ok()
        .flatten()
        .and_then(|el| el.get_attribute("content"))
        .filter(|s| !s.is_empty())
}

#[cfg(not(any(feature = "ssr", feature = "hydrate")))]
pub(crate) fn initial_vapid() -> Option<String> {
    None
}

/// The config's default reminder lead (ms) for this render — read from the server
/// env during SSR (and embedded in the shell `<meta name="pte-lead-ms">`), and
/// from that meta on hydrate. Seeds the global timers so they start at the
/// configured default (15m) with no round-trip. Falls back to 15m.
#[cfg(feature = "ssr")]
pub(crate) fn initial_lead_ms() -> i64 {
    crate::config::Config::from_env().reminder_lead_ms
}

#[cfg(feature = "hydrate")]
pub(crate) fn initial_lead_ms() -> i64 {
    web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| {
            d.query_selector("meta[name=\"pte-lead-ms\"]")
                .ok()
                .flatten()
        })
        .and_then(|el| el.get_attribute("content"))
        .and_then(|s| s.parse::<i64>().ok())
        .filter(|&ms| (0..=WEEK_MS).contains(&ms))
        .unwrap_or(900_000)
}

#[cfg(not(any(feature = "ssr", feature = "hydrate")))]
pub(crate) fn initial_lead_ms() -> i64 {
    900_000
}

/// The sport/league filter sets (`?g=`/`?e=`, comma-separated, percent-encoded) for
/// this render — read from the request on the server and the location on hydrate,
/// so the schedule is already filtered on the first paint instead of flashing the
/// full schedule until the client applies the filter. Only the URL is read here
/// (localStorage isn't available on the server); a saved-but-unlinked filter is
/// still applied by `FilterUrlSync` after hydration.
#[cfg(any(feature = "ssr", feature = "hydrate"))]
pub(crate) fn parse_filter_param(query: &str, key: &str) -> HashSet<String> {
    query_param(query, key)
        .map(|v| {
            v.split(',')
                .filter(|p| !p.is_empty())
                .map(dec_segment)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(feature = "ssr")]
pub(crate) fn initial_filter(key: &str) -> HashSet<String> {
    use_context::<http::request::Parts>()
        .map(|parts| parse_filter_param(parts.uri.query().unwrap_or_default(), key))
        .unwrap_or_default()
}

#[cfg(feature = "hydrate")]
pub(crate) fn initial_filter(key: &str) -> HashSet<String> {
    let search = web_sys::window()
        .and_then(|w| w.location().search().ok())
        .unwrap_or_default();
    parse_filter_param(search.trim_start_matches('?'), key)
}

#[cfg(not(any(feature = "ssr", feature = "hydrate")))]
pub(crate) fn initial_filter(_key: &str) -> HashSet<String> {
    HashSet::new()
}

pub(crate) fn initial_games() -> HashSet<String> {
    initial_filter("g")
}

pub(crate) fn initial_leagues() -> HashSet<String> {
    initial_filter("e")
}

/// `YYYY-MM-DD` for today plus `offset_days` (browser-local date).
#[cfg(feature = "hydrate")]
pub(crate) fn iso_from_today(offset_days: i64) -> String {
    let ms = js_sys::Date::now() + (offset_days as f64) * 86_400_000.0;
    let d = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms));
    format!(
        "{:04}-{:02}-{:02}",
        d.get_full_year() as i32,
        d.get_month() + 1,
        d.get_date()
    )
}

/// Add `delta` days to an ISO `YYYY-MM-DD` date, returning ISO. Pure (no JS) so
/// it's deterministic on both SSR and hydrate — used to window an event's days
/// relative to the server-provided `today_key`, which needs no browser clock.
pub(crate) fn iso_add_days(iso: &str, delta: i64) -> String {
    let mut it = iso.split('-');
    let y: i64 = it.next().and_then(|s| s.parse().ok()).unwrap_or(1970);
    let m: i64 = it.next().and_then(|s| s.parse().ok()).unwrap_or(1);
    let d: i64 = it.next().and_then(|s| s.parse().ok()).unwrap_or(1);
    let (ny, nm, nd) = civil_from_days(days_from_civil(y, m, d) + delta);
    format!("{ny:04}-{nm:02}-{nd:02}")
}

/// Days since 1970-01-01 for a proleptic-Gregorian date (Howard Hinnant's algo).
pub(crate) fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146097 + doe - 719468
}

/// Inverse of [`days_from_civil`]: `(year, month, day)` for a days-since-epoch.
pub(crate) fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Refetch the schedule periodically so a left-open tab stays current.
pub(crate) fn setup_autorefresh(resource: Resource<Result<ScheduleView, ServerFnError>>) {
    // The silent refresh re-renders (replaces) the schedule subtree, which drops
    // the browser's scroll anchor and can jump the page. Remember the scroll
    // position when the timer fires and restore it once the new schedule renders.
    let pending = StoredValue::new(None::<f64>);
    // Save the scroll position, then refetch the schedule from the server (so the
    // re-render doesn't jump the page). Shared by the 60s timer and the header's
    // manual button — neither forces the *server* to re-poll its own source.
    let refresh = move || {
        #[cfg(feature = "hydrate")]
        {
            pending.set_value(web_sys::window().and_then(|w| w.scroll_y().ok()));
            resource.refetch();
        }
        #[cfg(not(feature = "hydrate"))]
        let _ = (pending, resource);
    };
    Effect::new(move |_| {
        #[cfg(feature = "hydrate")]
        {
            use std::time::Duration;
            // Keep the handle so client-side route changes clear the timer instead
            // of leaking a new 60s refetch loop on every page mount.
            if let Ok(handle) = set_interval_with_handle(refresh, Duration::from_secs(60)) {
                on_cleanup(move || handle.clear());
            }
        }
        let _ = (resource, pending, refresh);
    });
    // Manual refresh from the header button: a shared, incrementing trigger.
    if let Some(RefreshTrigger(trigger)) = use_context::<RefreshTrigger>() {
        Effect::new(move |prev: Option<u64>| {
            let n = trigger.get();
            if prev.is_some() {
                refresh(); // skip the initial registration run
            }
            n
        });
    }
    // After the refreshed schedule renders, put the scroll back — only for the
    // silent auto-refresh; user-driven refetches leave `pending` unset.
    Effect::new(move |_| {
        resource.track();
        #[cfg(feature = "hydrate")]
        if let Some(y) = pending.get_value() {
            pending.set_value(None);
            request_animation_frame(move || {
                if let Some(w) = web_sys::window() {
                    w.scroll_to_with_x_and_y(0.0, y);
                }
            });
        }
        let _ = pending;
    });
}

/// The separator between the two teams of a match. Traditional sports list the
/// away team "at" the home team (team_a is away, team_b home — see `mlb.rs`);
/// esports use "vs".
pub(crate) const fn versus_sep(sport: Sport) -> &'static str {
    match sport {
        // The American team sports read "away at home"; soccer (its World Cup
        // games are at neutral venues, with no home side) and esports read "vs".
        Sport::Mlb | Sport::Nhl | Sport::Nba | Sport::Nfl => "at",
        _ => "vs",
    }
}

/// Number of palette colors leagues are hashed into (see `.lc-*` in the SCSS).
pub(crate) const LEAGUE_COLORS: u32 = 10;

/// Stable color class for a league name. Uses FNV-1a so the server and client
/// (WASM) compute the identical class — no hydration mismatch — and each event
/// keeps the same color across days.
pub(crate) fn league_color_class(name: &str) -> String {
    let mut h: u32 = 2_166_136_261;
    for b in name.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(16_777_619);
    }
    format!("lc-{}", h % LEAGUE_COLORS)
}

/// Which optional site-icon files exist in the configured `icons_dir`. Drives the
/// conditional `<head>` links: present files get real `<link>`s; when no favicon
/// file is present the shell keeps today's empty-data icon placeholder.
pub(crate) struct IconPresence {
    pub(crate) ico: bool,
    pub(crate) svg: bool,
    pub(crate) apple: bool,
    pub(crate) manifest: bool,
}

impl IconPresence {
    /// Any favicon-family file present (gates the placeholder + theme-color).
    pub(crate) fn has_favicon(&self) -> bool {
        self.ico || self.svg || self.apple
    }
}

/// Scan `dir` for the optional icon files relevant to the `<head>`. Pure over the
/// filesystem so it's directly testable. (The 192/512/maskable PNGs are referenced
/// only by the manifest, not the head, so they aren't scanned here.)
#[cfg(feature = "ssr")]
pub(crate) fn scan_icons(dir: &std::path::Path) -> IconPresence {
    let has = |name: &str| dir.join(name).is_file();
    IconPresence {
        ico: has("favicon.ico"),
        svg: has("favicon.svg"),
        apple: has("apple-touch-icon.png"),
        manifest: has("manifest.webmanifest"),
    }
}

/// The icon presence for this process, scanned once from `config().icons_dir`.
/// Cached, so adding/removing icon files is reflected in the `<head>` only after a
/// restart (the file routes themselves serve live from disk).
#[cfg(feature = "ssr")]
pub(crate) fn initial_icons() -> &'static IconPresence {
    use std::sync::OnceLock;
    static PRESENCE: OnceLock<IconPresence> = OnceLock::new();
    PRESENCE.get_or_init(|| scan_icons(std::path::Path::new(&crate::config::config().icons_dir)))
}

#[cfg(not(feature = "ssr"))]
pub(crate) fn initial_icons() -> &'static IconPresence {
    static NONE: IconPresence = IconPresence {
        ico: false,
        svg: false,
        apple: false,
        manifest: false,
    };
    &NONE
}

/// The cache-busting icon version for this render — appended as `?v=…` to icon URLs
/// in the `<head>` so an icon change yields fresh URLs. Empty when no icons exist
/// (append nothing). Server-derived (see `crate::icons`); the shell is server-only,
/// so the non-ssr build just returns "".
#[cfg(feature = "ssr")]
pub(crate) fn icon_version() -> &'static str {
    crate::icons::version()
}

#[cfg(not(feature = "ssr"))]
pub(crate) fn icon_version() -> &'static str {
    ""
}

#[cfg(all(test, feature = "ssr"))]
mod icon_tests {
    use super::scan_icons;
    use std::fs;

    #[test]
    fn scan_reports_only_present_files() {
        let dir = std::env::temp_dir().join(format!("pte_icons_present_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("favicon.ico"), b"x").unwrap();
        fs::write(dir.join("manifest.webmanifest"), b"{}").unwrap();

        let p = scan_icons(&dir);
        assert!(p.ico);
        assert!(!p.svg);
        assert!(!p.apple);
        assert!(p.manifest);
        assert!(p.has_favicon());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_empty_dir_reports_nothing() {
        let dir = std::env::temp_dir().join(format!("pte_icons_empty_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let p = scan_icons(&dir);
        assert!(!p.has_favicon());
        assert!(!p.manifest);

        let _ = fs::remove_dir_all(&dir);
    }
}
