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

#[cfg(test)]
mod tests {
    use super::*;

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
}
