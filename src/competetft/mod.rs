//! First-party TFT source: Riot's competetft.com (RSC) + the official published
//! Google Sheet (CSV). Produces the same output types as `crate::tft`
//! (Liquipedia), plus streamer/broadcast/lobby extras. Server-only.

pub mod rsc;
pub mod sheet;

use crate::tft::ParsedSession;
use crate::types::{TftBroadcast, TftDayPanel, TftLobbyRound, TftPlacement, TftStreamer};

const HOST: &str = "https://competetft.com";

/// The competetft overview URL for a tournament (used as the schedule rows'
/// source link).
fn tournament_url(id: &str) -> String {
    format!("{HOST}/en-US/tournament/{id}/overview")
}

/// Everything one tournament contributes to the caches: schedule sessions plus
/// the standings/placements/streamers/broadcasts/lobbies keyed under its event.
pub struct CompeteTournamentData {
    pub tournament: String,
    pub sessions: Vec<ParsedSession>,
    pub placements: Vec<TftPlacement>,
    pub standings: Vec<TftDayPanel>,
    pub streamers: Vec<TftStreamer>,
    pub broadcasts: Vec<TftBroadcast>,
    pub lobbies: Vec<TftLobbyRound>,
}

/// Assemble a tournament's data from its parsed overview, the schedule events,
/// and its sheet tabs (`(gid, csv_text)`). Pure — no network — so it is unit
/// tested directly against fixtures. Each tab is routed to its parser by header
/// classification; each broadcast day becomes a `ParsedSession` whose `begin_at`
/// is the feed's precise time (joined by `stage_id`).
#[must_use]
pub fn assemble(
    t: &rsc::CompeteTournament,
    events: &[rsc::CompeteEvent],
    tabs: &[(Option<String>, String)],
) -> CompeteTournamentData {
    use sheet::{
        TabKind, classify_tab, parse_broadcasts, parse_leaderboard, parse_lobbies, parse_streamers,
        read_csv,
    };

    let mut placements = Vec::new();
    let mut standings = Vec::new();
    let mut streamers = Vec::new();
    let mut broadcasts = Vec::new();
    let mut lobbies = Vec::new();

    for (_gid, csv) in tabs {
        let rows = read_csv(csv);
        match classify_tab(&rows) {
            TabKind::Leaderboard { finals } => {
                let lb = parse_leaderboard(&rows, finals);
                if !lb.standings.rows.is_empty() {
                    standings.push(TftDayPanel {
                        label: if finals {
                            "Finals".to_string()
                        } else {
                            "Day 1 & 2".to_string()
                        },
                        standings: lb.standings,
                    });
                }
                placements.extend(lb.placements);
            }
            TabKind::Streams => streamers = parse_streamers(&rows),
            TabKind::Lobbies { day3 } => lobbies.extend(parse_lobbies(&rows, day3)),
            TabKind::Info => broadcasts = parse_broadcasts(&rows),
            TabKind::Unknown => {}
        }
    }
    // Day 1 & 2 panel before Finals.
    standings.sort_by_key(|p| p.label == "Finals");

    // One session per broadcast day, joined to its stage title by stage_id.
    let sessions = events
        .iter()
        .filter(|e| e.tournament_id == t.id)
        .map(|e| {
            let label = t
                .stages
                .iter()
                .find(|s| s.stage_id == e.stage_id)
                .map(|s| s.title.clone())
                .unwrap_or_else(|| e.event_type.replace('_', " "));
            ParsedSession {
                tournament: t.name.clone(),
                session_label: label,
                begin_at: e.date,
                tournament_url: tournament_url(&t.id),
                streams: Vec::new(),
            }
        })
        .collect();

    CompeteTournamentData {
        tournament: t.name.clone(),
        sessions,
        placements,
        standings,
        streamers,
        broadcasts,
        lobbies,
    }
}

/// GET a URL as text with a browser-ish UA and a request timeout. Returns None
/// on any transport/status error (callers treat that as "no data this cycle").
#[cfg(feature = "ssr")]
async fn get(client: &reqwest::Client, url: &str) -> Option<String> {
    client
        .get(url)
        .header(
            reqwest::header::USER_AGENT,
            "Mozilla/5.0 (compatible; plaintextesports/1.0)",
        )
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .text()
        .await
        .ok()
}

/// Fetch competetft's schedule feed and parse it into TFT broadcast events.
#[cfg(feature = "ssr")]
pub async fn fetch_schedule(client: &reqwest::Client) -> Vec<rsc::CompeteEvent> {
    get(client, &format!("{HOST}/en-US/schedule"))
        .await
        .map(|h| rsc::parse_schedule(&h))
        .unwrap_or_default()
}

/// Fetch and parse a tournament's overview page (name, Set, stage→sheet map).
#[cfg(feature = "ssr")]
pub async fn fetch_tournament(
    client: &reqwest::Client,
    id: &str,
) -> Option<rsc::CompeteTournament> {
    let html = get(client, &tournament_url(id)).await?;
    Some(rsc::parse_tournament(id, &html))
}

/// Enumerate every tab gid from a published sheet's `pubhtml` and fetch each as
/// CSV. Returns `(gid, csv_text)` pairs.
#[cfg(feature = "ssr")]
pub async fn fetch_sheet_tabs(
    client: &reqwest::Client,
    key: &str,
) -> Vec<(Option<String>, String)> {
    let pub_url = format!("https://docs.google.com/spreadsheets/d/e/{key}/pubhtml");
    let Some(html) = get(client, &pub_url).await else {
        return vec![];
    };
    let mut gids: Vec<String> = Vec::new();
    for c in html.split("gid=").skip(1) {
        let g: String = c.chars().take_while(char::is_ascii_digit).collect();
        if !g.is_empty() && !gids.contains(&g) {
            gids.push(g);
        }
    }
    let mut out = Vec::new();
    for g in gids {
        let url = format!(
            "https://docs.google.com/spreadsheets/d/e/{key}/pub?gid={g}&single=true&output=csv"
        );
        if let Some(csv) = get(client, &url).await {
            out.push((Some(g), csv));
        }
    }
    out
}

/// Fetch and assemble one tournament end to end (overview → sheet tabs →
/// assemble). Returns None if the overview or every sheet tab fails to load, so
/// the caller keeps any cached rows untouched.
#[cfg(feature = "ssr")]
pub async fn refresh_competetft_tournament(
    client: &reqwest::Client,
    id: &str,
    events: &[rsc::CompeteEvent],
) -> Option<CompeteTournamentData> {
    let t = fetch_tournament(client, id).await?;
    let key = t
        .stages
        .iter()
        .find_map(|s| (!s.sheet_key.is_empty()).then(|| s.sheet_key.clone()))?;
    let tabs = fetch_sheet_tabs(client, &key).await;
    if tabs.is_empty() {
        return None;
    }
    Some(assemble(&t, events, &tabs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembles_tournament_from_fixtures() {
        let t = rsc::parse_tournament(
            "116323184504995859",
            include_str!("../fixtures/competetft_tournament.html"),
        );
        let events = rsc::parse_schedule(include_str!("../fixtures/competetft_schedule.html"));
        let tabs = vec![
            (
                Some("532077851".to_string()),
                include_str!("../fixtures/competetft_leaderboard.csv").to_string(),
            ),
            (
                Some("1506128251".to_string()),
                include_str!("../fixtures/competetft_finals.csv").to_string(),
            ),
            (
                Some("1551656749".to_string()),
                include_str!("../fixtures/competetft_streams.csv").to_string(),
            ),
            (
                Some("791702275".to_string()),
                include_str!("../fixtures/competetft_lobbies.csv").to_string(),
            ),
            (
                Some("893325208".to_string()),
                include_str!("../fixtures/competetft_info.csv").to_string(),
            ),
        ];
        let d = assemble(&t, &events, &tabs);
        assert!(!d.standings.is_empty());
        assert_eq!(d.standings[0].label, "Day 1 & 2");
        assert!(!d.placements.is_empty());
        assert!(d.streamers.len() >= 15);
        assert!(!d.lobbies.is_empty());
        assert!(d.broadcasts.iter().any(|b| b.label == "KR"));
        // three broadcast days, with feed-sourced labels
        assert_eq!(d.sessions.len(), 3);
        assert!(d.sessions.iter().any(|s| s.session_label.contains("Day 2")));
        assert_eq!(d.tournament, "Tactician's Crown");
    }

    /// Live smoke test. Ignored by default; run with
    /// `cargo test --features ssr live_smoke -- --ignored --nocapture`.
    #[test]
    #[ignore = "hits live competetft + sheets"]
    fn live_smoke() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let client = reqwest::Client::new();
        let evs = rt.block_on(fetch_schedule(&client));
        assert!(
            evs.iter()
                .all(|e| e.event_type == "TACTICIANS_CROWN" || e.event_type == "REGIONAL_FINALS")
        );
    }
}
