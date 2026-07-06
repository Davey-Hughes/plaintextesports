//! PandaScore standings + bracket/Swiss reconstruction (server-only).
//!
//! The per-tournament endpoints (`/tournaments/{id}` detail, `/standings`,
//! `/brackets`) plus the logic that turns PandaScore's flat bracket-match list
//! into a renderable shape: an elimination tree ([`BracketRound`]s) for a playoff,
//! or a Swiss buchholz grid ([`SwissRound`]s) for a group stage. Split out of the
//! match-fetching client in [`super`] since it's a distinct concern, but it shares
//! that module's `Raw*` team/result types and JSON helpers via `super::`.

use super::{BASE_URL, RawOpponent, RawResult, RawTeam, get_text, score_for, team_label};
use crate::http::DynError;
use crate::types::{BracketMatch, BracketRound, StandingRow, SwissBucket, SwissMatch, SwissRound};
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
struct RawTournamentDetail {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    has_bracket: Option<bool>,
}

/// A tournament's stage name (e.g. "Stage 1", "Playoffs") and whether it's an
/// elimination bracket (vs a Swiss/group stage represented by standings).
pub async fn fetch_tournament_meta(
    client: &reqwest::Client,
    token: &str,
    tournament_id: i64,
) -> Result<(String, bool), DynError> {
    let url = format!("{BASE_URL}/tournaments/{tournament_id}");
    let query: [(&str, &str); 0] = [];
    let (body, _) = get_text(client, token, &url, &query).await?;
    let d: RawTournamentDetail = serde_json::from_str(&body)?;
    Ok((d.name.unwrap_or_default(), d.has_bracket.unwrap_or(false)))
}

#[derive(Debug, Deserialize)]
struct RawStanding {
    #[serde(default)]
    rank: Option<i32>,
    #[serde(default)]
    team: Option<RawTeam>,
    #[serde(default)]
    wins: Option<i32>,
    #[serde(default)]
    losses: Option<i32>,
    #[serde(default)]
    ties: Option<i32>,
    #[serde(default)]
    game_wins: Option<i32>,
    #[serde(default)]
    game_losses: Option<i32>,
}

/// Fetch a tournament's standings (group/Swiss table). Bracket-only tournaments
/// return rows without W-L; those are dropped (the bracket carries that info).
pub async fn fetch_standings(
    client: &reqwest::Client,
    token: &str,
    tournament_id: i64,
) -> Result<Vec<StandingRow>, DynError> {
    let url = format!("{BASE_URL}/tournaments/{tournament_id}/standings");
    let body = client
        .get(&url)
        .bearer_auth(token)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let raw: Vec<RawStanding> = serde_json::from_str(&body)?;
    Ok(raw
        .iter()
        .filter(|r| r.team.is_some() && (r.wins.is_some() || r.losses.is_some()))
        .map(|r| StandingRow {
            rank: r.rank.unwrap_or(0),
            team: team_label(r.team.as_ref()).0,
            wins: r.wins.unwrap_or(0),
            losses: r.losses.unwrap_or(0),
            ties: r.ties.unwrap_or(0),
            game_wins: r.game_wins.unwrap_or(0),
            game_losses: r.game_losses.unwrap_or(0),
            ..Default::default()
        })
        .collect())
}

#[derive(Debug, Deserialize)]
struct RawBracketMatch {
    id: i64,
    /// Match name, e.g. "Lower Bracket Final" — used to split a double-elim
    /// bracket into upper/lower/grand-final sections.
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    opponents: Vec<RawOpponent>,
    #[serde(default)]
    results: Vec<RawResult>,
    #[serde(default)]
    winner_id: Option<i64>,
    #[serde(default)]
    previous_matches: Vec<RawPrevMatch>,
}

#[derive(Debug, Deserialize)]
struct RawPrevMatch {
    #[serde(default)]
    match_id: Option<i64>,
}

/// Fetch a tournament's bracket and reconstruct it: an elimination tree
/// (`rounds`) for a playoff, or a Swiss buchholz grid (`swiss`) for a "Round N"
/// group stage. At most one is non-empty.
pub async fn fetch_bracket(
    client: &reqwest::Client,
    token: &str,
    tournament_id: i64,
) -> Result<(Vec<BracketRound>, Vec<SwissRound>), DynError> {
    let url = format!("{BASE_URL}/tournaments/{tournament_id}/brackets");
    let body = client
        .get(&url)
        .bearer_auth(token)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let raw: Vec<RawBracketMatch> = serde_json::from_str(&body)?;
    Ok((build_rounds(&raw), build_swiss(&raw)))
}

/// Reconstruct a Swiss stage from its flat match list. Each match name carries
/// its round ("Round N: A vs B"); replaying the rounds in order recovers every
/// team's running record, which buckets the matches (the 2-0 teams play each
/// other, etc.) and marks who advanced (hit the win target) or was eliminated
/// (hit the loss limit). Returns empty when the matches aren't "Round N"-named
/// (e.g. a playoff), so a non-Swiss stage falls through to the tree/standings.
fn build_swiss(raw: &[RawBracketMatch]) -> Vec<SwissRound> {
    // Parse the round number out of each name; bail unless every match has one.
    let mut by_round: std::collections::BTreeMap<u32, Vec<&RawBracketMatch>> =
        std::collections::BTreeMap::new();
    for m in raw {
        let Some(round) = m.name.as_deref().and_then(parse_round) else {
            return Vec::new();
        };
        by_round.entry(round).or_default().push(m);
    }
    if by_round.is_empty() {
        return Vec::new();
    }
    // The win target / loss limit: a Swiss stage runs until a record reaches N
    // (3 for a first-to-3). Infer it from the deepest record seen + 1 isn't
    // reliable, so derive it as ceil(rounds / 2)+? — simplest: it's the max
    // wins or losses any team finishes with. Compute after replay; meanwhile use
    // a generous cap and mark outcomes by "no further matches" too.
    let mut records: HashMap<i64, (i32, i32)> = HashMap::new();
    let mut labels: HashMap<i64, String> = HashMap::new();
    // Each team's previous-round match id, so a match can name its feeders.
    let mut last_match: HashMap<i64, i64> = HashMap::new();
    let target = win_target(by_round.len());
    let mut out = Vec::with_capacity(by_round.len());
    for (number, matches) in by_round {
        // Bucket the round's matches by the (shared) entering record, computing
        // each result and the advance/eliminate outcome from the pre-round
        // record (so later matches in the round still see the pre-round state).
        let mut buckets: std::collections::BTreeMap<(i32, i32), Vec<SwissMatch>> =
            std::collections::BTreeMap::new();
        let mut updates: Vec<(i64, bool)> = Vec::new();
        for m in matches {
            let (label_a, _, id_a) =
                team_label(m.opponents.first().and_then(|o| o.opponent.as_ref()));
            let (label_b, _, id_b) =
                team_label(m.opponents.get(1).and_then(|o| o.opponent.as_ref()));
            if let (Some(a), Some(b)) = (id_a, id_b) {
                labels.entry(a).or_insert_with(|| label_a.clone());
                labels.entry(b).or_insert_with(|| label_b.clone());
            }
            let (pw, pl) = id_a
                .and_then(|a| records.get(&a).copied())
                .unwrap_or((0, 0));
            let winner = match m.winner_id {
                w if w.is_some() && w == id_a => "a",
                w if w.is_some() && w == id_b => "b",
                _ => "",
            };
            // Both sides enter with (pw, pl); the winner becomes (pw+1, pl) and
            // the loser (pw, pl+1). A side that reaches the target wins/losses
            // finishes here, and we record its final record (e.g. "3-0", "0-3").
            let win_rec = format!("{}-{}", pw + 1, pl);
            let loss_rec = format!("{}-{}", pw, pl + 1);
            let (mut a_rec, mut b_rec) = (String::new(), String::new());
            match winner {
                "a" => {
                    if pw + 1 >= target {
                        a_rec = win_rec;
                    }
                    if pl + 1 >= target {
                        b_rec = loss_rec;
                    }
                    updates.push((id_a.unwrap_or(0), true));
                    updates.push((id_b.unwrap_or(0), false));
                }
                "b" => {
                    if pw + 1 >= target {
                        b_rec = win_rec;
                    }
                    if pl + 1 >= target {
                        a_rec = loss_rec;
                    }
                    updates.push((id_b.unwrap_or(0), true));
                    updates.push((id_a.unwrap_or(0), false));
                }
                _ => {}
            }
            // Each side's feeder is its match in the previous round (None in
            // round 1, where the matchup is the seeding).
            let a_feeder = id_a.and_then(|a| last_match.get(&a).copied());
            let b_feeder = id_b.and_then(|b| last_match.get(&b).copied());
            if let Some(a) = id_a {
                last_match.insert(a, m.id);
            }
            if let Some(b) = id_b {
                last_match.insert(b, m.id);
            }
            buckets.entry((pw, pl)).or_default().push(SwissMatch {
                team_a: label_a,
                team_b: label_b,
                score_a: score_for(&m.results, id_a),
                score_b: score_for(&m.results, id_b),
                winner: winner.to_string(),
                match_id: m.id,
                a_record: a_rec,
                b_record: b_rec,
                a_feeder,
                b_feeder,
            });
        }
        for (id, won) in updates {
            let e = records.entry(id).or_insert((0, 0));
            if won {
                e.0 += 1;
            } else {
                e.1 += 1;
            }
        }
        // Buckets ordered most-wins-first (then fewest-losses), like the grid.
        let mut bucket_vec: Vec<((i32, i32), Vec<SwissMatch>)> = buckets.into_iter().collect();
        bucket_vec.sort_by_key(|&((w, l), _)| (-w, l));
        out.push(SwissRound {
            number,
            buckets: bucket_vec
                .into_iter()
                .map(|((w, l), matches)| SwissBucket {
                    record: format!("{w}-{l}"),
                    matches,
                })
                .collect(),
        });
    }
    out
}

/// Parse the round number from a Swiss match name like "Round 5: NRG vs BIG".
fn parse_round(name: &str) -> Option<u32> {
    name.strip_prefix("Round ")?
        .split([':', ' '])
        .next()?
        .parse()
        .ok()
}

/// The win/loss target for a Swiss stage with `rounds` rounds: a first-to-N
/// Swiss runs 2N-1 rounds, so N = (rounds + 1) / 2 (3 for the usual 5 rounds).
fn win_target(rounds: usize) -> i32 {
    ((rounds as i32) + 1) / 2
}

/// Depth of a bracket match = rounds of feeders beneath it (0 for first-round
/// matches). Memoized, with a cycle guard.
fn bracket_depth(
    id: i64,
    by_id: &HashMap<i64, &RawBracketMatch>,
    cache: &mut HashMap<i64, i32>,
    visiting: &mut std::collections::HashSet<i64>,
) -> i32 {
    if let Some(&d) = cache.get(&id) {
        return d;
    }
    if !visiting.insert(id) {
        return 0; // cycle / dangling — treat as a leaf
    }
    let depth = by_id.get(&id).map_or(0, |m| {
        m.previous_matches
            .iter()
            .filter_map(|p| p.match_id)
            .filter(|pid| by_id.contains_key(pid))
            .map(|pid| 1 + bracket_depth(pid, by_id, cache, visiting))
            .max()
            .unwrap_or(0)
    });
    visiting.remove(&id);
    cache.insert(id, depth);
    depth
}

/// Name a round by how many matches it has (1 = Final, 2 = Semifinals, …). Used
/// only as a fallback when PandaScore doesn't name the matches.
fn round_title(n_matches: usize) -> String {
    match n_matches {
        1 => "Final".to_string(),
        2 => "Semifinals".to_string(),
        4 => "Quarterfinals".to_string(),
        8 => "Round of 16".to_string(),
        16 => "Round of 32".to_string(),
        n => format!("Round of {}", n * 2),
    }
}

/// Turn a PandaScore bracket-match name into a round title. The names look like
/// "Upper bracket semifinal 1: KC vs DCG" / "Lower bracket round 1 match 2: …" /
/// "Grand final: …", so: drop the matchup, drop the upper/lower prefix (the
/// bracket already labels those halves) keeping "grand final", drop the per-match
/// instance number ("semifinal 1" → "semifinal", but keep "round 1"), pluralise a
/// semi/quarterfinal, and tidy the casing. None for an unusable name.
fn clean_round_name(name: &str) -> Option<String> {
    let lower = name.split(':').next().unwrap_or("").trim().to_lowercase();
    let stripped = lower
        .strip_prefix("upper bracket ")
        .or_else(|| lower.strip_prefix("lower bracket "))
        .unwrap_or(lower.as_str());
    let mut words: Vec<&str> = stripped.split_whitespace().collect();
    while let Some(&last) = words.last() {
        let drop = last == "match"
            || (last.bytes().all(|b| b.is_ascii_digit())
                && words.len() >= 2
                && words[words.len() - 2] != "round");
        if drop {
            words.pop();
        } else {
            break;
        }
    }
    if words.is_empty() {
        return None;
    }
    let mut title = words.join(" ");
    if title.ends_with("final") && title != "final" && title != "grand final" {
        title.push('s'); // semifinal → semifinals, quarterfinal → quarterfinals
    }
    Some(
        title
            .split(' ')
            .map(|w| {
                let mut c = w.chars();
                c.next()
                    .map(|f| f.to_uppercase().chain(c).collect::<String>())
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// Which half of a double-elimination bracket a match belongs to, from its name.
fn bracket_section(m: &RawBracketMatch) -> &'static str {
    let n = m.name.as_deref().unwrap_or("").to_lowercase();
    if n.contains("grand") {
        "final"
    } else if n.contains("lower") || n.contains("loser") {
        "lower"
    } else {
        "upper"
    }
}

fn build_rounds(raw: &[RawBracketMatch]) -> Vec<BracketRound> {
    if raw.is_empty() {
        return Vec::new();
    }
    // A Swiss/group stage comes back from the brackets endpoint as a flat set of
    // matches with no feeder links (no tree). It isn't an elimination bracket —
    // its standings table represents it — so don't render it as one (which would
    // otherwise collapse into a single nonsensical "Round of N" column).
    let is_tree = raw
        .iter()
        .any(|m| m.previous_matches.iter().any(|p| p.match_id.is_some()));
    if raw.len() > 1 && !is_tree {
        return Vec::new();
    }
    let by_id: HashMap<i64, &RawBracketMatch> = raw.iter().map(|m| (m.id, m)).collect();
    let mut cache = HashMap::new();
    let mut visiting = std::collections::HashSet::new();

    // A bracket is double-elimination only if it has a lower bracket. When it
    // does, columns are grouped upper → lower → grand-final (each by depth);
    // otherwise it's one section ("") grouped purely by depth.
    let double_elim = raw.iter().any(|m| bracket_section(m) == "lower");

    let mut max_depth = 0;
    let tagged: Vec<(&RawBracketMatch, i32, &str)> = raw
        .iter()
        .map(|m| {
            let d = bracket_depth(m.id, &by_id, &mut cache, &mut visiting);
            max_depth = max_depth.max(d);
            let sec = if double_elim { bracket_section(m) } else { "" };
            (m, d, sec)
        })
        .collect();

    // Order columns by section, then depth (preserving input order within one).
    let sections: &[&str] = if double_elim {
        &["upper", "lower", "final"]
    } else {
        &[""]
    };
    let mut columns: Vec<(&str, Vec<&RawBracketMatch>)> = Vec::new();
    for &sec in sections {
        for d in 0..=max_depth {
            let ms: Vec<&RawBracketMatch> = tagged
                .iter()
                .filter(|(_, md, msec)| *md == d && *msec == sec)
                .map(|(m, _, _)| *m)
                .collect();
            if !ms.is_empty() {
                columns.push((sec, ms));
            }
        }
    }

    // Order matches within each column so the vertical layout lines up with the
    // connector lines: a later-round match sits centred between the two feeders
    // it joins, so position `i` in a column must be fed by positions `2i`/`2i+1`
    // of the previous one. PandaScore returns each round in an arbitrary order,
    // so within each section walk from the deepest column back, laying out every
    // column in the order its successor consumes its feeders.
    let section_cols: Vec<Vec<usize>> = sections
        .iter()
        .map(|&sec| {
            columns
                .iter()
                .enumerate()
                .filter(|(_, (s, _))| *s == sec)
                .map(|(i, _)| i)
                .collect()
        })
        .collect();
    for idxs in &section_cols {
        for w in (0..idxs.len().saturating_sub(1)).rev() {
            let (cur, next) = (idxs[w], idxs[w + 1]);
            let cur_ms: Vec<&RawBracketMatch> = columns[cur].1.clone();
            let next_ms: Vec<&RawBracketMatch> = columns[next].1.clone();
            let mut ordered: Vec<&RawBracketMatch> = Vec::with_capacity(cur_ms.len());
            let mut seen: std::collections::HashSet<i64> = std::collections::HashSet::new();
            for parent in &next_ms {
                for pid in parent.previous_matches.iter().filter_map(|p| p.match_id) {
                    if !seen.contains(&pid)
                        && let Some(m) = cur_ms.iter().find(|m| m.id == pid)
                    {
                        seen.insert(pid);
                        ordered.push(*m);
                    }
                }
            }
            // Any match no successor reached keeps its place, appended in order.
            for m in &cur_ms {
                if seen.insert(m.id) {
                    ordered.push(*m);
                }
            }
            columns[cur].1 = ordered;
        }
    }

    // Map each match id to its (column_index, position) so feeder edges can be
    // expressed as grid coordinates the UI can resolve without the raw ids. The
    // section ordering keeps every feeder in an earlier column.
    let mut pos: HashMap<i64, (usize, usize)> = HashMap::new();
    for (r, (_, ms)) in columns.iter().enumerate() {
        for (i, m) in ms.iter().enumerate() {
            pos.insert(m.id, (r, i));
        }
    }

    columns
        .into_iter()
        .map(|(sec, ms)| {
            // Prefer PandaScore's own round name; fall back to the match count.
            let title = ms
                .iter()
                .find_map(|m| m.name.as_deref().and_then(clean_round_name))
                .unwrap_or_else(|| round_title(ms.len()));
            let matches = ms
                .iter()
                .map(|m| to_bracket_match(m, &pos))
                .collect::<Vec<_>>();
            BracketRound {
                title,
                matches,
                section: sec.to_string(),
            }
        })
        .collect()
}

fn to_bracket_match(m: &RawBracketMatch, pos: &HashMap<i64, (usize, usize)>) -> BracketMatch {
    let (label_a, _, id_a) = team_label(m.opponents.first().and_then(|o| o.opponent.as_ref()));
    let (label_b, _, id_b) = team_label(m.opponents.get(1).and_then(|o| o.opponent.as_ref()));
    let winner = match m.winner_id {
        w if w.is_some() && w == id_a => "a",
        w if w.is_some() && w == id_b => "b",
        _ => "",
    };
    let feeders = m
        .previous_matches
        .iter()
        .filter_map(|p| p.match_id.and_then(|id| pos.get(&id).copied()))
        .collect();
    BracketMatch {
        team_a: label_a,
        team_b: label_b,
        score_a: score_for(&m.results, id_a),
        score_b: score_for(&m.results, id_b),
        winner: winner.to_string(),
        match_id: m.id,
        feeders,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_rounds_orders_columns_to_match_feeder_layout() {
        // Rounds arrive scrambled (final first, semis before quarters, both out
        // of tree order). The layout must still place each match's feeders at
        // positions 2i / 2i+1 of the previous column so the connectors line up.
        let json = r#"[
          {"id":20,"opponents":[],"previous_matches":[{"match_id":10},{"match_id":11}]},
          {"id":11,"opponents":[{"opponent":{"acronym":"A1"}},{"opponent":{"acronym":"B1"}}],
           "previous_matches":[{"match_id":1},{"match_id":2}]},
          {"id":10,"opponents":[{"opponent":{"acronym":"C1"}},{"opponent":{"acronym":"D1"}}],
           "previous_matches":[{"match_id":3},{"match_id":4}]},
          {"id":2,"opponents":[{"opponent":{"acronym":"B1"}},{"opponent":{"acronym":"B2"}}],"previous_matches":[]},
          {"id":1,"opponents":[{"opponent":{"acronym":"A1"}},{"opponent":{"acronym":"A2"}}],"previous_matches":[]},
          {"id":4,"opponents":[{"opponent":{"acronym":"D1"}},{"opponent":{"acronym":"D2"}}],"previous_matches":[]},
          {"id":3,"opponents":[{"opponent":{"acronym":"C1"}},{"opponent":{"acronym":"C2"}}],"previous_matches":[]}
        ]"#;
        let raw: Vec<RawBracketMatch> = serde_json::from_str(json).expect("valid json");
        let rounds = build_rounds(&raw);
        assert_eq!(rounds.len(), 3, "quarters / semis / final");
        // The final's feeders are the two semifinals in column order.
        assert_eq!(rounds[2].matches[0].feeders, vec![(1, 0), (1, 1)]);
        // Each semifinal's feeders sit at 2i / 2i+1 of the quarterfinal column.
        for (i, sf) in rounds[1].matches.iter().enumerate() {
            assert_eq!(
                sf.feeders,
                vec![(0, 2 * i), (0, 2 * i + 1)],
                "semifinal {i}"
            );
        }
        // Final.prev = [10, 11] orders the semis [C/D, A/B]; that propagates to
        // the quarterfinal column so each semifinal sits between its feeders.
        let qf_a: Vec<&str> = rounds[0]
            .matches
            .iter()
            .map(|m| m.team_a.as_str())
            .collect();
        assert_eq!(qf_a, vec!["C1", "D1", "A1", "B1"]);
    }

    #[test]
    fn build_swiss_buckets_and_outcomes() {
        // A 4-team, first-to-2 Swiss: rounds parse from names, records replay
        // into buckets, and 2-0 / 0-2 trigger advance / eliminate.
        let g =
            |id: i64, name: &str, a: (i64, &str), b: (i64, &str), wa: i64, wb: i64, win: i64| {
                format!(
                    r#"{{"id":{id},"name":"{name}",
                  "opponents":[{{"opponent":{{"id":{},"acronym":"{}"}}}},
                               {{"opponent":{{"id":{},"acronym":"{}"}}}}],
                  "results":[{{"team_id":{},"score":{wa}}},{{"team_id":{},"score":{wb}}}],
                  "winner_id":{win}}}"#,
                    a.0, a.1, b.0, b.1, a.0, b.0
                )
            };
        let json = format!(
            "[{}]",
            [
                g(1, "Round 1: A vs B", (1, "A"), (2, "B"), 1, 0, 1),
                g(2, "Round 1: C vs D", (3, "C"), (4, "D"), 1, 0, 3),
                g(3, "Round 2: A vs C", (1, "A"), (3, "C"), 1, 0, 1),
                g(4, "Round 2: B vs D", (2, "B"), (4, "D"), 1, 0, 2),
                g(5, "Round 3: C vs B", (3, "C"), (2, "B"), 1, 0, 3),
            ]
            .join(",")
        );
        let raw: Vec<RawBracketMatch> = serde_json::from_str(&json).expect("valid json");
        let swiss = build_swiss(&raw);
        assert_eq!(swiss.len(), 3, "three rounds");
        let recs = |r: &SwissRound| {
            r.buckets
                .iter()
                .map(|b| b.record.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(recs(&swiss[0]), vec!["0-0"]);
        assert_eq!(recs(&swiss[1]), vec!["1-0", "0-1"]); // most wins first
        assert_eq!(recs(&swiss[2]), vec!["1-1"]);
        // Round 2: A wins the 1-0 bucket → advances 2-0; D loses 0-1 → out 0-2.
        assert_eq!(swiss[1].buckets[0].matches[0].a_record, "2-0");
        assert_eq!(swiss[1].buckets[1].matches[0].b_record, "0-2");
        // Round 3 decides the last spots: C advances 2-1, B is eliminated 1-2.
        assert_eq!(swiss[2].buckets[0].matches[0].a_record, "2-1");
        assert_eq!(swiss[2].buckets[0].matches[0].b_record, "1-2");
        // Feeders: round 1 has none; later rounds point at each side's prior
        // match. R2 A-vs-C is fed by A's R1 (id 1) and C's R1 (id 2); R3 C-vs-B
        // by C's R2 (id 3) and B's R2 (id 4).
        assert_eq!(swiss[0].buckets[0].matches[0].a_feeder, None);
        assert_eq!(swiss[0].buckets[0].matches[0].b_feeder, None);
        assert_eq!(swiss[1].buckets[0].matches[0].a_feeder, Some(1));
        assert_eq!(swiss[1].buckets[0].matches[0].b_feeder, Some(2));
        assert_eq!(swiss[2].buckets[0].matches[0].a_feeder, Some(3));
        assert_eq!(swiss[2].buckets[0].matches[0].b_feeder, Some(4));
    }

    #[test]
    fn clean_round_name_strips_section_and_instance() {
        let c = |s: &str| clean_round_name(s);
        assert_eq!(
            c("Upper bracket semifinal 1: KC vs DCG").as_deref(),
            Some("Semifinals")
        );
        assert_eq!(
            c("Upper bracket quarterfinal 3: TBD vs TBD").as_deref(),
            Some("Quarterfinals")
        );
        assert_eq!(
            c("Upper bracket final: TBD vs TBD").as_deref(),
            Some("Final")
        );
        assert_eq!(
            c("Lower bracket round 1 match 2: TBD vs TBD").as_deref(),
            Some("Round 1")
        );
        assert_eq!(
            c("Lower bracket final: TBD vs TBD").as_deref(),
            Some("Final")
        );
        assert_eq!(c("Grand final: TBD vs TBD").as_deref(), Some("Grand Final"));
        assert_eq!(c("").as_deref(), None);
    }
}
