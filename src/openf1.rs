//! Free-practice classifications from the keyless OpenF1 API (`api.openf1.org`).
//! Jolpica (Ergast) carries no practice data, so the GP event page sources its
//! practice timing sheets here instead: best lap per driver in FP1/FP2/FP3.
//!
//! OpenF1 keys everything by `meeting_key` (a Grand Prix weekend) and exposes no
//! round number, and its meeting order doesn't line up with Jolpica's rounds, so
//! we resolve the weekend by date — the meeting whose start falls in the few days
//! before the race. From there: practice sessions → driver map → each session's
//! classification (fastest lap per driver).

use crate::types::{F1Result, F1ResultRow};
use chrono::{Duration, NaiveDate};
use serde::de::DeserializeOwned;
use serde::Deserialize;

const BASE: &str = "https://api.openf1.org/v1";
const USER_AGENT: &str =
    "plaintextesports/0.1 (https://github.com/ralphpotato/plaintextesports; ralphpotato@gmail.com)";

#[derive(Deserialize)]
struct Meeting {
    meeting_key: i64,
    #[serde(default)]
    meeting_name: String,
    #[serde(default)]
    date_start: String,
}

#[derive(Deserialize)]
struct Session {
    session_key: i64,
    #[serde(default)]
    session_name: String,
    #[serde(default)]
    date_start: String,
}

#[derive(Deserialize)]
struct Driver {
    driver_number: i64,
    #[serde(default)]
    full_name: String,
    #[serde(default)]
    first_name: Option<String>,
    #[serde(default)]
    last_name: Option<String>,
    #[serde(default)]
    team_name: String,
}

#[derive(Deserialize)]
struct SessionResult {
    session_key: i64,
    #[serde(default)]
    position: Option<i64>,
    #[serde(default)]
    driver_number: i64,
    /// The driver's fastest lap, in seconds (e.g. 76.363). Lenient because the
    /// meeting-wide result list mixes session types: practice is a single number,
    /// but qualifying is an array (Q1/Q2/Q3) — kept as raw JSON and read as a
    /// scalar only where it is one (the practice rows we use).
    #[serde(default)]
    duration: serde_json::Value,
}

/// GET + parse JSON, retrying a few times on a rate-limit (429) or server hiccup
/// — OpenF1 throttles bursts, so a short backoff lets the weekend's handful of
/// calls through. Returns `None` on a clean client error or once retries run out.
async fn get_json<T: DeserializeOwned>(client: &reqwest::Client, url: &str) -> Option<T> {
    for attempt in 0u32..4 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(500 * u64::from(attempt))).await;
        }
        let Ok(resp) = client
            .get(url)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .send()
            .await
        else {
            continue; // network blip — retry
        };
        let status = resp.status();
        if status.as_u16() == 429 || status.is_server_error() {
            continue; // throttled / transient — retry
        }
        return resp.error_for_status().ok()?.json().await.ok();
    }
    None
}

/// The date part of an OpenF1 "2026-06-12T11:30:00+00:00" timestamp.
fn date_of(ts: &str) -> Option<NaiveDate> {
    ts.get(..10).and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
}

/// Format a lap time given in seconds, e.g. 76.363 → "1:16.363".
fn fmt_laptime(secs: f64) -> String {
    if secs <= 0.0 {
        return String::new();
    }
    let total_ms = (secs * 1000.0).round() as i64;
    let m = total_ms / 60_000;
    let s = (total_ms % 60_000) / 1000;
    let ms = total_ms % 1000;
    if m > 0 {
        format!("{m}:{s:02}.{ms:03}")
    } else {
        format!("{s}.{ms:03}")
    }
}

/// Title-case an ALL-CAPS surname fallback (e.g. "Oliver BEARMAN" → "Oliver
/// Bearman"); used only when first/last name fields are absent.
fn title_case(s: &str) -> String {
    s.split_whitespace()
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(first) => first
                    .to_uppercase()
                    .chain(chars.flat_map(char::to_lowercase))
                    .collect::<String>(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn driver_display(d: &Driver) -> String {
    // OpenF1's names are inconsistently cased (some surnames are ALL-CAPS), so
    // title-case whichever form we use for a uniform look alongside the Jolpica
    // race/quali names.
    let name = match (&d.first_name, &d.last_name) {
        (Some(f), Some(l)) if !f.is_empty() && !l.is_empty() => format!("{f} {l}"),
        _ => d.full_name.clone(),
    };
    title_case(&name)
}

/// The OpenF1 meeting (GP weekend) for a Jolpica race date: a non-testing meeting
/// whose start falls within the few days before the race (FP1 is ~2 days before).
async fn meeting_for_date(
    client: &reqwest::Client,
    season: i64,
    race_date: NaiveDate,
) -> Option<i64> {
    let url = format!("{BASE}/meetings?year={season}");
    let meetings: Vec<Meeting> = get_json(client, &url).await?;
    let lo = race_date - Duration::days(4);
    meetings
        .into_iter()
        .filter(|m| !m.meeting_name.to_lowercase().contains("testing"))
        .find(|m| date_of(&m.date_start).is_some_and(|d| d >= lo && d <= race_date))
        .map(|m| m.meeting_key)
}

/// Build one driver-number → (name, team) map for the weekend.
async fn driver_map(
    client: &reqwest::Client,
    meeting_key: i64,
) -> std::collections::HashMap<i64, (String, String)> {
    let url = format!("{BASE}/drivers?meeting_key={meeting_key}");
    let drivers: Vec<Driver> = get_json(client, &url).await.unwrap_or_default();
    let mut map = std::collections::HashMap::new();
    for d in drivers {
        map.entry(d.driver_number)
            .or_insert_with(|| (driver_display(&d), d.team_name.clone()));
    }
    map
}

/// One practice session's classification from the meeting-wide result list: best
/// lap per driver, ordered by position (detail = the fastest lap time).
fn session_rows(
    all: &[SessionResult],
    session_key: i64,
    drivers: &std::collections::HashMap<i64, (String, String)>,
) -> Vec<F1ResultRow> {
    let mut rows: Vec<&SessionResult> =
        all.iter().filter(|r| r.session_key == session_key).collect();
    // Classified order: by position, the unplaced (no lap) last.
    rows.sort_by_key(|r| r.position.unwrap_or(i64::MAX));
    rows.iter()
        .map(|r| {
            let (driver, constructor) = drivers
                .get(&r.driver_number)
                .cloned()
                .unwrap_or_else(|| (format!("#{}", r.driver_number), String::new()));
            F1ResultRow {
                pos: r.position.map(|p| p.to_string()).unwrap_or_default(),
                driver,
                constructor,
                detail: r.duration.as_f64().map(fmt_laptime).unwrap_or_default(),
            }
        })
        .collect()
}

/// Practice classifications (FP1/FP2/FP3) for the Grand Prix on `race_date`, as
/// [`F1Result`] sections in session order. Empty if the weekend can't be matched
/// or OpenF1 has no practice data for it (anything pre-2023, or upcoming).
pub async fn fetch_practice(
    client: &reqwest::Client,
    season: i64,
    race_date: NaiveDate,
) -> Vec<F1Result> {
    let Some(meeting_key) = meeting_for_date(client, season, race_date).await else {
        return Vec::new();
    };
    // Fetch the practice sessions, every session result for the weekend (one
    // meeting-wide call, not one per session), and the driver map — sequentially,
    // so OpenF1's burst throttle stays happy.
    let sessions_url = format!("{BASE}/sessions?meeting_key={meeting_key}&session_type=Practice");
    let results_url = format!("{BASE}/session_result?meeting_key={meeting_key}");
    let mut sessions: Vec<Session> = get_json(client, &sessions_url).await.unwrap_or_default();
    let results: Vec<SessionResult> = get_json(client, &results_url).await.unwrap_or_default();
    let drivers = driver_map(client, meeting_key).await;
    // FP1, FP2, FP3 in order.
    sessions.sort_by(|a, b| a.date_start.cmp(&b.date_start));
    let mut out = Vec::new();
    for s in sessions {
        let rows = session_rows(&results, s.session_key, &drivers);
        if !rows.is_empty() {
            out.push(F1Result { session: s.session_name, rows });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_lap_times() {
        assert_eq!(fmt_laptime(76.363), "1:16.363");
        assert_eq!(fmt_laptime(90.5), "1:30.500");
        assert_eq!(fmt_laptime(0.0), "");
    }

    #[test]
    fn title_cases_allcaps_surnames() {
        assert_eq!(title_case("Oliver BEARMAN"), "Oliver Bearman");
        assert_eq!(title_case("Kimi ANTONELLI"), "Kimi Antonelli");
    }

    #[test]
    fn parses_meeting_date() {
        assert_eq!(
            date_of("2026-06-12T11:30:00+00:00"),
            Some(NaiveDate::from_ymd_opt(2026, 6, 12).unwrap())
        );
    }
}
