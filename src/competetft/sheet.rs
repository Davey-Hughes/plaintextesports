use crate::types::{TftPlacement, TftStandingRow, TftStandings, TftStreamer};
use csv::ReaderBuilder;

/// Parse published-sheet CSV into rows of string cells. Tolerant: ragged rows
/// are kept as-is (Google pads short rows inconsistently).
#[must_use]
pub fn read_csv(text: &str) -> Vec<Vec<String>> {
    ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_reader(text.as_bytes())
        .records()
        .filter_map(Result::ok)
        .map(|r| r.iter().map(|c| c.trim().to_string()).collect())
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabKind {
    Leaderboard { finals: bool },
    Streams,
    Lobbies { day3: bool },
    Info,
    Unknown,
}

/// Classify a sheet tab by its header content (never by gid — gids are bespoke
/// per tournament). Scans the first ~6 rows joined uppercase.
///
/// `DAY 1`/`DAY 3` markers are matched as whole cells, not substrings: the
/// leaderboard tab's data rows use the phrase "Qualify to Day 3" (a
/// qualification note in the prize column), which would otherwise
/// false-positive a plain substring search for "DAY 3" against the joined
/// header text.
#[must_use]
pub fn classify_tab(rows: &[Vec<String>]) -> TabKind {
    let cells = rows
        .iter()
        .take(6)
        .flatten()
        .map(|c| c.to_ascii_uppercase())
        .collect::<Vec<_>>();
    let head = cells.join("|");
    let has = |s: &str| head.contains(s);
    let has_cell = |s: &str| cells.iter().any(|c| c == s);
    if has("BROADCAST NAME") && has("STREAM LINK") {
        TabKind::Streams
    } else if has("POINT SYSTEM") || has("REGIONAL BROADCAST") {
        TabKind::Info
    } else if has("ROUND 1") && has("LOBBY 1") {
        TabKind::Lobbies {
            day3: has_cell("DAY 3") && !has_cell("DAY 1"),
        }
    } else if has("POSITION") && has("NAME") && has("POINTS") {
        TabKind::Leaderboard {
            finals: has_cell("DAY 3") || has("EXCL R13"),
        }
    } else {
        TabKind::Unknown
    }
}

/// A tab's leaderboard: the ranked standings plus any prize placements carried
/// in the same table (an eliminated row's Prize/Tiebreakers cell holds a payout
/// instead of a qualify note).
pub struct Leaderboard {
    pub standings: TftStandings,
    pub placements: Vec<TftPlacement>,
}

/// The header row: the first row whose cells contain both `POSITION` and
/// `POINTS` (case-insensitive, whole-cell match).
fn header_idx(rows: &[Vec<String>]) -> Option<usize> {
    rows.iter().position(|r| {
        let up: Vec<String> = r.iter().map(|c| c.to_ascii_uppercase()).collect();
        up.iter().any(|c| c == "POSITION") && up.iter().any(|c| c == "POINTS")
    })
}

/// Normalize a prize/qualify cell into `(prize, status)`: a `$`-prize passes
/// through as-is; a qualify marker like `Qualify to Day 3` becomes the status
/// `"→ Day 3"`; anything else (including empty) yields both empty.
fn prize_or_status(cell: &str) -> (String, String) {
    let t = cell.trim();
    if t.starts_with('$') {
        (t.to_string(), String::new())
    } else if t.to_ascii_uppercase().contains("QUALIFY") || t.starts_with('→') {
        // "Qualify to Day 3" → "→ Day 3"
        let tail = t
            .split_whitespace()
            .skip_while(|w| !w.eq_ignore_ascii_case("to"))
            .skip(1)
            .collect::<Vec<_>>()
            .join(" ");
        (
            String::new(),
            if tail.is_empty() {
                "→ advance".into()
            } else {
                format!("→ {tail}")
            },
        )
    } else {
        (String::new(), String::new())
    }
}

/// Whether a cell is a row's prize/qualify marker (a `$`-prize or a
/// "Qualify..." note) rather than a round score.
fn is_prize_cell(cell: &str) -> bool {
    cell.starts_with('$') || cell.to_ascii_uppercase().contains("QUALIFY")
}

/// Parse a leaderboard tab (`Position, Name, [REGION], Points, Round 1..Round
/// K, Prize ($), [Tiebreakers]`) into ranked standings plus any prize
/// placements.
///
/// The published sheet is ragged: a row can carry one fewer round value than
/// the header's `Round N` columns (Google Sheets drops the cell rather than
/// leaving it blank), which shifts that row's trailing cells left by one. That
/// means the Prize/qualify cell can't be found at a fixed header offset — it's
/// located by content (starts with `$`, or mentions "qualify") scanning
/// forward from just past the Points column, and everything before it up to
/// that column is the row's round values, padded (or truncated) to the
/// header's round-column count so every row's `games` is the same length.
#[must_use]
pub fn parse_leaderboard(rows: &[Vec<String>], _finals: bool) -> Leaderboard {
    let Some(h) = header_idx(rows) else {
        return Leaderboard {
            standings: TftStandings {
                game_count: 0,
                rows: vec![],
            },
            placements: vec![],
        };
    };
    let header: Vec<String> = rows[h].iter().map(|c| c.to_ascii_uppercase()).collect();
    let col = |name: &str| header.iter().position(|c| c == name);
    let pos_i = col("POSITION").unwrap_or(0);
    let name_i = col("NAME").unwrap_or(1);
    let pts_i = col("POINTS").unwrap_or(3);
    let game_count = header.iter().filter(|c| c.starts_with("ROUND")).count();

    let mut out_rows = Vec::new();
    let mut placements = Vec::new();
    for r in &rows[h + 1..] {
        let get = |i: usize| {
            r.get(i)
                .map(String::as_str)
                .unwrap_or("")
                .trim()
                .to_string()
        };
        let rank = get(pos_i);
        let name = get(name_i);
        if rank.is_empty() || name.is_empty() {
            continue;
        }
        // Round values run from just after Points up to the first prize/qualify
        // cell (found by content, since a dropped round shifts a fixed header
        // offset off by one for that row).
        let start = pts_i + 1;
        let prize_idx = (start..r.len()).find(|&i| is_prize_cell(&get(i)));
        let end = prize_idx.unwrap_or(r.len());
        let mut games: Vec<String> = (start..end)
            .map(get)
            .map(|v| if v == "-" { String::new() } else { v })
            .collect();
        games.resize(game_count, String::new());
        let (prize, status) = prize_idx
            .map(|i| prize_or_status(&get(i)))
            .unwrap_or_default();
        out_rows.push(TftStandingRow {
            rank: rank.clone(),
            participant: name.clone(),
            total: get(pts_i),
            games,
            status,
        });
        if !prize.is_empty() {
            placements.push(TftPlacement {
                place: rank,
                participant: name,
                prize,
            });
        }
    }
    Leaderboard {
        standings: TftStandings {
            game_count,
            rows: out_rows,
        },
        placements,
    }
}

/// Normalize a raw stream cell into `(platform, absolute_url)`. Accepts bare
/// hosts (`twitch.tv/x`), full URLs, and youtube/tiktok links; returns None for
/// an empty cell.
#[must_use]
pub fn normalize_stream_url(raw: &str) -> Option<(String, String)> {
    let t = raw.trim();
    if t.is_empty() {
        return None;
    }
    let url = if t.starts_with("http") {
        t.to_string()
    } else {
        format!("https://{t}")
    };
    let low = url.to_ascii_lowercase();
    let platform = if low.contains("twitch.tv") {
        "twitch"
    } else if low.contains("youtube.com") || low.contains("youtu.be") {
        "youtube"
    } else if low.contains("tiktok.com") {
        "tiktok"
    } else {
        "other"
    };
    Some((platform.to_string(), url))
}

/// Parse the player co-streamer directory tab (`… BROADCAST NAME, REGION,
/// STREAM LINK, …`) into per-player stream entries. Rows without a name or a
/// usable link are skipped.
#[must_use]
pub fn parse_streamers(rows: &[Vec<String>]) -> Vec<TftStreamer> {
    let Some(h) = rows.iter().position(|r| {
        let up = r
            .iter()
            .map(|c| c.to_ascii_uppercase())
            .collect::<Vec<_>>()
            .join("|");
        up.contains("BROADCAST NAME") && up.contains("STREAM LINK")
    }) else {
        return vec![];
    };
    let header: Vec<String> = rows[h].iter().map(|c| c.to_ascii_uppercase()).collect();
    let name_i = header
        .iter()
        .position(|c| c == "BROADCAST NAME")
        .unwrap_or(2);
    let link_i = header.iter().position(|c| c == "STREAM LINK").unwrap_or(4);
    let mut out = Vec::new();
    for r in &rows[h + 1..] {
        let name = r.get(name_i).map(|s| s.trim()).unwrap_or("");
        let link = r.get(link_i).map(|s| s.trim()).unwrap_or("");
        if name.is_empty() {
            continue;
        }
        if let Some((platform, url)) = normalize_stream_url(link) {
            out.push(TftStreamer {
                player: name.to_string(),
                url,
                platform,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_stream_urls() {
        assert_eq!(
            normalize_stream_url("twitch.tv/guillosko"),
            Some(("twitch".into(), "https://twitch.tv/guillosko".into()))
        );
        assert_eq!(
            normalize_stream_url("https://www.twitch.tv/horox335")
                .unwrap()
                .0,
            "twitch"
        );
        assert_eq!(
            normalize_stream_url("youtube.com/@steppytft").unwrap().0,
            "youtube"
        );
        assert_eq!(normalize_stream_url(""), None);
    }

    #[test]
    fn parses_streamers() {
        let rows = read_csv(include_str!("../fixtures/competetft_streams.csv"));
        let s = parse_streamers(&rows);
        assert!(s.iter().any(|x| x.player == "Guillosko"
            && x.url.ends_with("/guillosko")
            && x.platform == "twitch"));
        assert!(s.len() >= 15);
        assert!(
            s.iter()
                .all(|x| !x.player.is_empty() && x.url.starts_with("http"))
        );
    }

    #[test]
    fn classifies_each_tab() {
        assert!(matches!(
            classify_tab(&read_csv(include_str!(
                "../fixtures/competetft_leaderboard.csv"
            ))),
            TabKind::Leaderboard { finals: false }
        ));
        assert!(matches!(
            classify_tab(&read_csv(include_str!("../fixtures/competetft_finals.csv"))),
            TabKind::Leaderboard { finals: true }
        ));
        assert_eq!(
            classify_tab(&read_csv(include_str!(
                "../fixtures/competetft_streams.csv"
            ))),
            TabKind::Streams
        );
        assert!(matches!(
            classify_tab(&read_csv(include_str!(
                "../fixtures/competetft_lobbies.csv"
            ))),
            TabKind::Lobbies { day3: false }
        ));
        assert_eq!(
            classify_tab(&read_csv(include_str!("../fixtures/competetft_info.csv"))),
            TabKind::Info
        );
    }

    #[test]
    fn read_csv_handles_quoted_multiline() {
        let rows = read_csv("a,b\n\"x\ny\",z\n");
        assert_eq!(rows[1][0], "x\ny");
        assert_eq!(rows[1][1], "z");
    }

    #[test]
    fn parses_leaderboard() {
        let rows = read_csv(include_str!("../fixtures/competetft_leaderboard.csv"));
        let lb = parse_leaderboard(&rows, false);
        // rank 1 known row
        let first = &lb.standings.rows[0];
        assert_eq!(first.rank, "1");
        assert_eq!(first.participant, "AUR Huanmie");
        assert_eq!(first.total, "75");
        assert_eq!(first.games.len(), lb.standings.game_count);
        assert_eq!(first.status, "→ Day 3"); // "Qualify to Day 3" normalized
        // an eliminated row carries a prize placement, no qualify status
        let paid = lb
            .placements
            .iter()
            .find(|p| p.participant == "bruhbruhbruhwho")
            .unwrap();
        assert_eq!(paid.prize, "$11,000");
    }
}
