//! First-party TFT source: Riot's competetft.com (RSC) + the official published
//! Google Sheet (CSV). Produces the same output types as `crate::tft`
//! (Liquipedia), plus streamer/broadcast/lobby extras. Server-only.

pub mod rsc;
pub mod sheet;

use crate::tft::ParsedSession;
use crate::types::{TftBroadcast, TftDayPanel, TftLobbyRound, TftPlacement, TftStreamer};
use chrono::{DateTime, Datelike, Utc};

const HOST: &str = "https://competetft.com";

/// The competetft overview URL for a tournament (used as the schedule rows'
/// source link).
fn tournament_url(id: &str) -> String {
    format!("{HOST}/en-US/tournament/{id}/overview")
}

/// Infer the calendar year for a month/day that the overview omits: choose the
/// occurrence nearest to `now` among {year-1, year, year+1}, biased by status —
/// UPCOMING should not resolve to the past, and a non-upcoming (completed/live)
/// event should not resolve to the far future.
#[must_use]
pub fn infer_year(month: u32, day: u32, status: &str, now: DateTime<Utc>) -> i32 {
    let upcoming = status.eq_ignore_ascii_case("upcoming");
    let mut best = now.year();
    let mut best_score = i64::MAX;
    for y in [now.year() - 1, now.year(), now.year() + 1] {
        let Some(d) = chrono::NaiveDate::from_ymd_opt(y, month, day) else {
            continue;
        };
        let dt = d.and_hms_opt(0, 0, 0).unwrap().and_utc();
        let delta = (dt - now).num_days();
        let mut score = delta.abs();
        if upcoming && delta < 0 {
            score += 400; // an upcoming event in the past is wrong
        }
        if !upcoming && delta > 200 {
            score += 400; // a completed/live event far in the future is wrong
        }
        if score < best_score {
            best_score = score;
            best = y;
        }
    }
    best
}

/// The tournament's event name, year-qualified. CompeteTFT names tournaments bare
/// ("Tactician's Crown") and every set has one, so without the year they collide
/// across seasons in event URLs, session ids, and enrichment keys. Reads like the
/// other esports events ("Mid-Season Invitational 2026"). The year comes from the
/// scraped date range (inferred), else the tournament's first broadcast event;
/// with neither, the bare name is used.
#[must_use]
pub fn tournament_event_name(
    t: &rsc::CompeteTournament,
    events: &[rsc::CompeteEvent],
    now: DateTime<Utc>,
) -> String {
    let year = t
        .date_range
        .map(|dr| infer_year(dr.start.0, dr.start.1, &t.status, now))
        .or_else(|| {
            events
                .iter()
                .find(|e| e.tournament_id == t.id)
                .map(|e| e.date.year())
        });
    match year {
        Some(y) => format!("{} {y}", t.name),
        None => t.name.clone(),
    }
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
    /// The tournament's published Google Sheet (`pubhtml`), refreshed with the rest
    /// each poll. Surfaced as the "see all" link behind the capped stream list —
    /// the sheet carries every co-streamer, we only show a handful.
    pub sheet_url: String,
}

/// The published sheet's public URL for a tournament, or empty when it publishes
/// none. Same `pubhtml` address `fetch_sheet_tabs` reads the tabs from.
#[must_use]
pub fn sheet_url(t: &rsc::CompeteTournament) -> String {
    t.stages
        .iter()
        .find(|s| !s.sheet_key.is_empty())
        .map(|s| {
            format!(
                "https://docs.google.com/spreadsheets/d/e/{}/pubhtml",
                s.sheet_key
            )
        })
        .unwrap_or_default()
}

/// Human date-range label for a tier-3 row, e.g. "May 2 – 10" or "May 23 – Jun 7".
fn range_display(dr: rsc::DateRange) -> String {
    const MON: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (sm, sd) = dr.start;
    let (em, ed) = dr.end;
    if sm == em && sd == ed {
        format!("{} {sd}", MON[(sm - 1) as usize])
    } else if sm == em {
        format!("{} {sd} \u{2013} {ed}", MON[(sm - 1) as usize])
    } else {
        format!(
            "{} {sd} \u{2013} {} {ed}",
            MON[(sm - 1) as usize],
            MON[(em - 1) as usize]
        )
    }
}

/// Map a scraped CompeteTFT status badge to a match status, or `None` (unknown ⇒
/// fall back to the clock). Used for tier-3 rows whose `begin_at` is a range start.
fn tft_status_override(status: &str) -> Option<crate::types::MatchStatus> {
    use crate::types::MatchStatus;
    let s = status.to_ascii_uppercase();
    if s.contains("COMPLETE") {
        Some(MatchStatus::Finished)
    } else if s.contains("UPCOMING") {
        Some(MatchStatus::Upcoming)
    } else if s.contains("LIVE") || s.contains("PROGRESS") || s.contains("ONGOING") {
        Some(MatchStatus::Live)
    } else {
        None
    }
}

/// Per-day round counts from the lobby breakdown's labels ("Day 1 · Round 3"),
/// ascending by day — e.g. `[(1, 6), (2, 7), (3, 8)]`. This is the only place the
/// sheet says which rounds belong to which day, so it's what lets the combined
/// "Day 1 & 2" leaderboard be split. Empty when no label parses (⇒ don't split).
#[must_use]
pub fn day_round_counts(lobbies: &[crate::types::TftLobbyRound]) -> Vec<(u32, usize)> {
    let mut counts: std::collections::BTreeMap<u32, usize> = std::collections::BTreeMap::new();
    for l in lobbies {
        // "Day 1 · Round 3" → day 1. Take the token after "Day ", before the
        // separator; anything else is not a per-day round label.
        let Some(rest) = l.label.strip_prefix("Day ") else {
            continue;
        };
        let Some(day) = rest
            .split_whitespace()
            .next()
            .and_then(|d| d.parse::<u32>().ok())
        else {
            continue;
        };
        if !rest.contains("Round") {
            continue;
        }
        *counts.entry(day).or_insert(0) += 1;
    }
    counts.into_iter().collect()
}

/// Sum a row's game scores, treating blank/"-" as 0.
fn games_total(games: &[String]) -> i64 {
    games
        .iter()
        .filter_map(|g| g.trim().parse::<i64>().ok())
        .sum()
}

/// A participant's prize from the placement ladder, or empty.
fn prize_of(placements: &[crate::types::TftPlacement], who: &str) -> String {
    placements
        .iter()
        .find(|p| p.participant == who)
        .map(|p| p.prize.clone())
        .unwrap_or_default()
}

/// Rework the CompeteTFT leaderboard panels for display: split the sheet's
/// combined "Day 1 & 2" panel into per-day panels using the lobby round→day map,
/// and expand "Finals" to the whole field with eliminated players carrying their
/// placement prize and last cumulative score. Returns `panels` untouched when
/// there's no lobby data to split on (never guesses a boundary).
#[must_use]
pub fn split_day_panels(
    panels: Vec<crate::types::TftDayPanel>,
    lobbies: &[crate::types::TftLobbyRound],
    placements: &[crate::types::TftPlacement],
) -> Vec<crate::types::TftDayPanel> {
    use crate::types::{TftDayPanel, TftStandings};

    let counts = day_round_counts(lobbies);
    // The first entry must actually BE day 1 (counts is ascending by day): if a
    // sheet only labelled later days, its round count would silently bound a
    // "Day 1" split that never existed — a guess, which this must never make.
    let Some(&(1, n1)) = counts.first() else {
        return panels; // no day-1 lobby data ⇒ no split
    };
    let Some(combined) = panels.iter().find(|p| p.label == "Day 1 & 2").cloned() else {
        return panels;
    };
    if n1 == 0 || n1 >= combined.standings.game_count {
        return panels; // boundary doesn't apply to this panel
    }

    // Day 1: re-total and re-rank on the Day-1 rounds only.
    let mut d1_rows: Vec<crate::types::TftStandingRow> = combined
        .standings
        .rows
        .iter()
        .map(|r| {
            let games: Vec<String> = r.games.iter().take(n1).cloned().collect();
            let mut row = r.clone();
            row.total = games_total(&games).to_string();
            row.games = games;
            row.status = String::new();
            row.prize = String::new();
            row.eliminated = false;
            row
        })
        .collect();
    d1_rows.sort_by_key(|r| std::cmp::Reverse(r.total.parse::<i64>().unwrap_or(0)));
    for (i, r) in d1_rows.iter_mut().enumerate() {
        r.rank = (i + 1).to_string();
    }
    let day1 = TftDayPanel {
        label: "Day 1".to_string(),
        standings: TftStandings {
            game_count: n1,
            rows: d1_rows,
        },
    };

    // Day 2: the cumulative panel, relabeled; non-advancers greyed with prize.
    let mut day2 = combined.clone();
    day2.label = "Day 2".to_string();
    for r in &mut day2.standings.rows {
        if r.status.trim().is_empty() {
            r.eliminated = true;
            r.prize = prize_of(placements, &r.participant);
        }
    }

    // Finals: the finalists, then everyone else greyed with prize + last total.
    let finals = panels.iter().find(|p| p.label == "Finals").cloned();
    let finals = finals.map(|mut f| {
        let in_finals: std::collections::HashSet<String> = f
            .standings
            .rows
            .iter()
            .map(|r| r.participant.clone())
            .collect();
        let mut extra: Vec<crate::types::TftStandingRow> = combined
            .standings
            .rows
            .iter()
            .filter(|r| !in_finals.contains(&r.participant))
            .map(|r| {
                let mut row = r.clone();
                row.games = Vec::new(); // no finals games for a knocked-out player
                row.status = String::new();
                row.eliminated = true;
                row.prize = prize_of(placements, &r.participant);
                row // `total` stays: the last cumulative score
            })
            .collect();
        extra.sort_by_key(|r| std::cmp::Reverse(r.total.parse::<i64>().unwrap_or(0)));
        f.standings.rows.append(&mut extra);
        f
    });

    let mut out = vec![day1, day2];
    if let Some(f) = finals {
        out.push(f);
    }
    out
}

/// Turn a tournament into schedule sessions via the 3-tier date precedence:
/// (1) broadcast feed (real times), (2) clean per-day map when stage count equals
/// the range's day span and stage titles are distinct, (3) one tournament-level
/// row spanning the range. Off-broadcast rows are anchored at 12:00 UTC (nominal;
/// date-granular). Empty when there's neither a feed nor a date range.
#[must_use]
pub fn derive_sessions(
    t: &rsc::CompeteTournament,
    events: &[rsc::CompeteEvent],
    now: DateTime<Utc>,
) -> Vec<crate::tft::ParsedSession> {
    let name = tournament_event_name(t, events, now);
    let url = tournament_url(&t.id);
    let mk = |label: String, begin_at: DateTime<Utc>| crate::tft::ParsedSession {
        tournament: name.clone(),
        session_label: label,
        begin_at,
        tournament_url: url.clone(),
        streams: Vec::new(),
        status: None,
    };

    // Tier 1: broadcast feed — real per-stage dates + times.
    let feed: Vec<&rsc::CompeteEvent> = events.iter().filter(|e| e.tournament_id == t.id).collect();
    if !feed.is_empty() {
        return feed
            .iter()
            .map(|e| {
                let label = t
                    .stages
                    .iter()
                    .find(|s| s.stage_id == e.stage_id)
                    .map(|s| s.title.clone())
                    .unwrap_or_else(|| e.event_type.replace('_', " "));
                mk(label, e.date)
            })
            .collect();
    }

    // Tiers 2/3 need a date range.
    let Some(dr) = t.date_range else {
        return Vec::new();
    };
    let year = infer_year(dr.start.0, dr.start.1, &t.status, now);
    let end_year = if dr.end.0 < dr.start.0 {
        year + 1
    } else {
        year
    };
    let (Some(start_d), Some(end_d)) = (
        chrono::NaiveDate::from_ymd_opt(year, dr.start.0, dr.start.1),
        chrono::NaiveDate::from_ymd_opt(end_year, dr.end.0, dr.end.1),
    ) else {
        return Vec::new();
    };
    // Nominal noon UTC keeps the calendar day stable for most viewer timezones.
    let start = start_d.and_hms_opt(12, 0, 0).unwrap().and_utc();
    let span = (end_d - start_d).num_days() + 1;

    let distinct = {
        let mut titles: Vec<&str> = t.stages.iter().map(|s| s.title.as_str()).collect();
        titles.sort_unstable();
        let n = titles.len();
        titles.dedup();
        titles.len() == n
    };

    // Tier 2: clean per-day map.
    if span >= 1 && t.stages.len() as i64 == span && distinct {
        return t
            .stages
            .iter()
            .enumerate()
            .map(|(i, s)| mk(s.title.clone(), start + chrono::Duration::days(i as i64)))
            .collect();
    }

    // Tier 3: one tournament-level row labeled by the range. `begin_at` is the
    // range start, so the clock's fixed live window can't be trusted — carry the
    // scraped tournament status instead.
    let mut row = mk(range_display(dr), start);
    row.status = tft_status_override(&t.status);
    vec![row]
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
    now: DateTime<Utc>,
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

    // Rework for display: per-day panels + a full-field Finals (see
    // `split_day_panels`). Falls back to the sheet's own panels without lobbies.
    let standings = split_day_panels(standings, &lobbies, &placements);

    let sessions = derive_sessions(t, events, now);

    CompeteTournamentData {
        tournament: tournament_event_name(t, events, now),
        sessions,
        placements,
        standings,
        streamers,
        broadcasts,
        lobbies,
        sheet_url: sheet_url(t),
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

/// The schedule page split into what discovery needs: every tournament id the
/// site lists (present year-round) plus the dated broadcast events (live/near
/// only, the sole source of session times).
#[derive(Default)]
pub struct Schedule {
    pub tournaments: Vec<String>,
    pub events: Vec<rsc::CompeteEvent>,
}

/// Fetch competetft's schedule page once and parse it into the season tournament
/// ids (for discovery) and the dated broadcast events (for session times).
#[cfg(feature = "ssr")]
pub async fn fetch_schedule(client: &reqwest::Client) -> Schedule {
    match get(client, &format!("{HOST}/en-US/schedule")).await {
        Some(h) => Schedule {
            tournaments: rsc::parse_tournament_ids(&h),
            events: rsc::parse_schedule(&h),
        },
        None => Schedule::default(),
    }
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

/// Fetch + assemble a tournament's sheet data from an already-parsed overview.
/// The sheet (pubhtml + a CSV per tab) is the expensive part; the overview is
/// fetched and cached separately by the poller. Returns None if the tournament
/// publishes no sheet or every tab fails, so the caller keeps cached rows.
#[cfg(feature = "ssr")]
pub async fn refresh_from_overview(
    client: &reqwest::Client,
    t: &rsc::CompeteTournament,
    events: &[rsc::CompeteEvent],
    now: DateTime<Utc>,
) -> Option<CompeteTournamentData> {
    let key = t
        .stages
        .iter()
        .find_map(|s| (!s.sheet_key.is_empty()).then(|| s.sheet_key.clone()))?;
    let tabs = fetch_sheet_tabs(client, &key).await;
    if tabs.is_empty() {
        return None;
    }
    Some(assemble(t, events, &tabs, now))
}

/// Fetch and assemble one tournament end to end (overview → sheet tabs →
/// assemble). Returns None if the overview or every sheet tab fails to load, so
/// the caller keeps any cached rows untouched.
#[cfg(feature = "ssr")]
pub async fn refresh_competetft_tournament(
    client: &reqwest::Client,
    id: &str,
    events: &[rsc::CompeteEvent],
    now: DateTime<Utc>,
) -> Option<CompeteTournamentData> {
    let t = fetch_tournament(client, id).await?;
    refresh_from_overview(client, &t, events, now).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lobby(label: &str) -> crate::types::TftLobbyRound {
        crate::types::TftLobbyRound {
            label: label.into(),
            lobbies: Vec::new(),
        }
    }

    fn row(
        rank: &str,
        who: &str,
        total: &str,
        games: &[&str],
        status: &str,
    ) -> crate::types::TftStandingRow {
        crate::types::TftStandingRow {
            rank: rank.into(),
            participant: who.into(),
            total: total.into(),
            games: games.iter().map(|g| (*g).to_string()).collect(),
            status: status.into(),
            prize: String::new(),
            eliminated: false,
        }
    }
    fn panel(
        label: &str,
        game_count: usize,
        rows: Vec<crate::types::TftStandingRow>,
    ) -> crate::types::TftDayPanel {
        crate::types::TftDayPanel {
            label: label.into(),
            standings: crate::types::TftStandings { game_count, rows },
        }
    }
    fn place(p: &str, who: &str, prize: &str) -> crate::types::TftPlacement {
        crate::types::TftPlacement {
            place: p.into(),
            participant: who.into(),
            prize: prize.into(),
        }
    }
    fn day12_lobbies() -> Vec<crate::types::TftLobbyRound> {
        let mut ls: Vec<crate::types::TftLobbyRound> = (1..=2)
            .map(|r| crate::types::TftLobbyRound {
                label: format!("Day 1 · Round {r}"),
                lobbies: Vec::new(),
            })
            .collect();
        ls.extend((3..=4).map(|r| crate::types::TftLobbyRound {
            label: format!("Day 2 · Round {r}"),
            lobbies: Vec::new(),
        }));
        ls
    }

    #[test]
    fn split_day_panels_splits_day1_by_lobby_rounds_and_retotals() {
        // 4 games; lobbies say Day 1 = first 2 rounds. "B" wins Day 1 alone (9)
        // even though "A" leads cumulatively (12) — so Day 1 must re-rank.
        let combined = panel(
            "Day 1 & 2",
            4,
            vec![
                row("1", "A", "12", &["3", "3", "3", "3"], "→ Day 3"),
                row("2", "B", "10", &["5", "4", "1", ""], ""),
            ],
        );
        let out = split_day_panels(vec![combined], &day12_lobbies(), &[place("2", "B", "$100")]);
        let d1 = out
            .iter()
            .find(|p| p.label == "Day 1")
            .expect("Day 1 panel");
        assert_eq!(d1.standings.game_count, 2);
        // Re-ranked by Day-1-only points: B (9) ahead of A (6).
        let order: Vec<&str> = d1
            .standings
            .rows
            .iter()
            .map(|r| r.participant.as_str())
            .collect();
        assert_eq!(order, ["B", "A"]);
        assert_eq!(d1.standings.rows[0].total, "9");
        assert_eq!(d1.standings.rows[0].games, vec!["5", "4"]);
        // Day 2 keeps the cumulative panel, relabeled.
        let d2 = out
            .iter()
            .find(|p| p.label == "Day 2")
            .expect("Day 2 panel");
        assert_eq!(d2.standings.rows[0].total, "12");
        // B didn't advance → greyed with prize on Day 2.
        let b = d2
            .standings
            .rows
            .iter()
            .find(|r| r.participant == "B")
            .unwrap();
        assert!(b.eliminated);
        assert_eq!(b.prize, "$100");
    }

    #[test]
    fn split_day_panels_keeps_combined_panel_without_lobby_data() {
        let combined = panel(
            "Day 1 & 2",
            4,
            vec![row("1", "A", "12", &["3", "3", "3", "3"], "")],
        );
        let out = split_day_panels(vec![combined.clone()], &[], &[]);
        assert_eq!(out, vec![combined], "no lobby data ⇒ no split, unchanged");
    }

    #[test]
    fn split_day_panels_expands_finals_to_the_full_field() {
        let combined = panel(
            "Day 1 & 2",
            4,
            vec![
                row("1", "A", "12", &["3", "3", "3", "3"], "→ Day 3"),
                row("2", "B", "10", &["5", "4", "1", ""], ""),
            ],
        );
        let finals = panel("Finals", 2, vec![row("1", "A", "9", &["4", "5"], "")]);
        let out = split_day_panels(
            vec![combined, finals],
            &day12_lobbies(),
            &[place("1", "A", "$500"), place("2", "B", "$100")],
        );
        let f = out
            .iter()
            .find(|p| p.label == "Finals")
            .expect("Finals panel");
        // Full field: the finalist first, then the eliminated player greyed.
        assert_eq!(f.standings.rows.len(), 2);
        assert_eq!(f.standings.rows[0].participant, "A");
        assert!(!f.standings.rows[0].eliminated);
        let b = &f.standings.rows[1];
        assert_eq!(b.participant, "B");
        assert!(b.eliminated);
        assert_eq!(b.prize, "$100");
        // Last cumulative score carried over from the combined panel.
        assert_eq!(b.total, "10");
    }

    #[test]
    fn day_round_counts_maps_lobby_labels_to_per_day_counts() {
        // Mirrors the real Tactician's Crown data: Day 1 rounds 1-6, Day 2 rounds
        // 7-13, Day 3 rounds 1-8 (the finals renumber from 1).
        let mut ls: Vec<crate::types::TftLobbyRound> = (1..=6)
            .map(|r| lobby(&format!("Day 1 · Round {r}")))
            .collect();
        ls.extend((7..=13).map(|r| lobby(&format!("Day 2 · Round {r}"))));
        ls.extend((1..=8).map(|r| lobby(&format!("Day 3 · Round {r}"))));
        assert_eq!(day_round_counts(&ls), vec![(1, 6), (2, 7), (3, 8)]);
    }

    #[test]
    fn day_round_counts_is_empty_without_parseable_labels() {
        assert!(day_round_counts(&[]).is_empty());
        assert!(day_round_counts(&[lobby("Lobby A"), lobby("nonsense")]).is_empty());
    }

    fn tourn(
        id: &str,
        name: &str,
        dr: Option<rsc::DateRange>,
        stages: &[&str],
    ) -> rsc::CompeteTournament {
        rsc::CompeteTournament {
            id: id.into(),
            name: name.into(),
            set_label: "Set 18".into(),
            date_range: dr,
            status: "COMPLETED".into(),
            stages: stages
                .iter()
                .enumerate()
                .map(|(i, s)| rsc::CompeteStage {
                    stage_id: format!("{id}-{i}"),
                    title: (*s).into(),
                    sheet_key: String::new(),
                    gid: None,
                })
                .collect(),
        }
    }

    #[test]
    fn derive_sessions_tier2_clean_cup_maps_stages_to_consecutive_days() {
        let now = "2026-07-13T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let t = tourn(
            "1",
            "Stargazer Cup AMER",
            Some(rsc::DateRange {
                start: (5, 8),
                end: (5, 10),
            }),
            &["Day 1", "Day 2", "Finals"],
        );
        let s = derive_sessions(&t, &[], now);
        assert_eq!(s.len(), 3);
        let labels: Vec<&str> = s.iter().map(|x| x.session_label.as_str()).collect();
        assert_eq!(labels, ["Day 1", "Day 2", "Finals"]);
        // Consecutive days May 8, 9, 10 (year inferred 2026).
        let days: Vec<u32> = s.iter().map(|x| x.begin_at.day()).collect();
        assert_eq!(days, [8, 9, 10]);
        assert!(
            s.iter()
                .all(|x| x.begin_at.month() == 5 && x.begin_at.year() == 2026)
        );
    }

    #[test]
    fn derive_sessions_tier3_falls_back_to_one_row_when_stages_ne_days() {
        let now = "2026-07-13T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        // Trials: 5 stages over a 9-day range → not a clean map.
        let t = tourn(
            "2",
            "Tactician's Trials 1 AMER",
            Some(rsc::DateRange {
                start: (5, 2),
                end: (5, 10),
            }),
            &["Day 1", "Day 2", "Day 3", "Day 4", "Finals"],
        );
        let s = derive_sessions(&t, &[], now);
        assert_eq!(s.len(), 1, "one tournament-level row");
        // en-dash (U+2013), matching range_display.
        assert_eq!(s[0].session_label, "May 2 \u{2013} 10");
        assert_eq!((s[0].begin_at.month(), s[0].begin_at.day()), (5, 2));
    }

    #[test]
    fn derive_sessions_tier3_status_reflects_scraped_status_not_clock() {
        use crate::types::MatchStatus;
        // A LIVE multi-day Trials, "now" several days after the range start — the
        // clock alone (status_at) would say Finished; the scraped status must win.
        let now = "2026-05-06T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let mut t = tourn(
            "9",
            "Tactician's Trials 1 AMER",
            Some(rsc::DateRange {
                start: (5, 2),
                end: (5, 10),
            }),
            &["Day 1", "Day 2", "Day 3", "Day 4", "Finals"],
        );
        t.status = "LIVE".into();
        let s = derive_sessions(&t, &[], now);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].status, Some(MatchStatus::Live));
    }

    #[test]
    fn derive_sessions_tier1_uses_broadcast_feed_times() {
        let now = "2026-07-13T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let t = tourn(
            "3",
            "Live Cup",
            Some(rsc::DateRange {
                start: (7, 10),
                end: (7, 12),
            }),
            &["Day 1"],
        );
        let events = vec![rsc::CompeteEvent {
            tournament_id: "3".into(),
            stage_id: "3-0".into(),
            date: "2026-07-10T18:30:00Z".parse::<DateTime<Utc>>().unwrap(),
            event_type: "TACTICIANS_CROWN".into(),
        }];
        let s = derive_sessions(&t, &events, now);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].session_label, "Day 1");
        // Real broadcast time preserved (not the noon nominal).
        assert_eq!(
            s[0].begin_at,
            "2026-07-10T18:30:00Z".parse::<DateTime<Utc>>().unwrap()
        );
    }

    #[test]
    fn infer_year_picks_nearest_occurrence_with_status_bias() {
        let now = "2026-07-13T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        // A completed July event is this year.
        assert_eq!(infer_year(7, 10, "COMPLETED", now), 2026);
        // A completed January event (already passed) is this year, not next.
        assert_eq!(infer_year(1, 15, "COMPLETED", now), 2026);
        // An upcoming January event rolls to next year rather than the past.
        assert_eq!(infer_year(1, 15, "UPCOMING", now), 2027);
        // An upcoming December event is this year (still ahead).
        assert_eq!(infer_year(12, 20, "UPCOMING", now), 2026);
    }

    #[test]
    fn tournament_event_name_appends_the_inferred_year() {
        let now = "2026-07-13T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let t = tourn(
            "1",
            "Tactician's Crown",
            Some(rsc::DateRange {
                start: (7, 10),
                end: (7, 12),
            }),
            &["Day 1", "Day 2", "Finals"],
        );
        assert_eq!(
            tournament_event_name(&t, &[], now),
            "Tactician's Crown 2026"
        );
    }

    #[test]
    fn derive_sessions_year_qualifies_the_tournament_name() {
        // Sessions must carry the year-qualified name so event URLs and session ids
        // don't collide with the next set's identically-named tournament.
        let now = "2026-07-13T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let t = tourn(
            "1",
            "Tactician's Crown",
            Some(rsc::DateRange {
                start: (7, 10),
                end: (7, 12),
            }),
            &["Day 1", "Day 2", "Finals"],
        );
        let s = derive_sessions(&t, &[], now);
        assert!(!s.is_empty());
        assert!(s.iter().all(|x| x.tournament == "Tactician's Crown 2026"));
    }

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
        let now = "2026-07-13T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let d = assemble(&t, &events, &tabs, now);
        assert!(!d.standings.is_empty());
        // The fixture carries lobby data, so the combined panel splits: Day 1
        // (re-totaled/re-ranked) sorts first, ahead of Day 2 and Finals.
        assert_eq!(d.standings[0].label, "Day 1");
        assert!(!d.placements.is_empty());
        assert!(d.streamers.len() >= 15);
        assert!(!d.lobbies.is_empty());
        assert!(d.broadcasts.iter().any(|b| b.label == "KR"));
        // three broadcast days, with feed-sourced labels
        assert_eq!(d.sessions.len(), 3);
        assert!(d.sessions.iter().any(|s| s.session_label.contains("Day 2")));
        assert_eq!(d.tournament, "Tactician's Crown 2026");
    }

    /// Live smoke test. Ignored by default; run with
    /// `cargo test --features ssr live_smoke -- --ignored --nocapture`.
    #[test]
    #[ignore = "hits live competetft + sheets"]
    fn live_smoke() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let client = reqwest::Client::new();
        let sched = rt.block_on(fetch_schedule(&client));
        // The tournament list is present year-round, independent of broadcasts.
        assert!(!sched.tournaments.is_empty());
        assert!(
            sched
                .events
                .iter()
                .all(|e| e.event_type == "TACTICIANS_CROWN" || e.event_type == "REGIONAL_FINALS")
        );
    }

    /// Full live pipeline: discover → refresh one tournament → assemble. Prints
    /// the section counts. Ignored by default; run with
    /// `cargo test --features ssr live_full_refresh -- --ignored --nocapture`.
    #[test]
    #[ignore = "hits live competetft + sheets"]
    fn live_full_refresh() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let client = reqwest::Client::new();
        let sched = rt.block_on(fetch_schedule(&client));
        println!(
            "live schedule: {} tournament(s), {} broadcast event(s)",
            sched.tournaments.len(),
            sched.events.len()
        );
        // Prefer a currently-broadcasting tournament (dated); else the first
        // discovered tournament id; else the known Tactician's Crown id (its
        // overview + sheet stay published, so the sheet pipeline is exercisable
        // off-broadcast).
        let id = sched
            .events
            .iter()
            .find(|e| e.event_type == "TACTICIANS_CROWN")
            .map(|e| e.tournament_id.clone())
            .or_else(|| sched.tournaments.first().cloned())
            .unwrap_or_else(|| "116323184504995859".to_string());
        let now = Utc::now();
        match rt.block_on(refresh_competetft_tournament(
            &client,
            &id,
            &sched.events,
            now,
        )) {
            Some(d) => {
                println!(
                    "tournament={:?} sessions={} standings={} placements={} streamers={} broadcasts={} lobbies={}",
                    d.tournament,
                    d.sessions.len(),
                    d.standings.len(),
                    d.placements.len(),
                    d.streamers.len(),
                    d.broadcasts.len(),
                    d.lobbies.len(),
                );
                // Sessions need the live feed (empty off-broadcast); the sheet
                // data is the pipeline proof.
                assert!(
                    !d.standings.is_empty() || !d.streamers.is_empty(),
                    "expected sheet-derived standings or streamers"
                );
            }
            None => println!("no data for {id} (its sheet may not be published yet)"),
        }
    }
}
