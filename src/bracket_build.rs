//! Assemble a traditional-sport postseason into the renderer's [`BracketRound`]
//! shape, shared by every sport's fetcher (soccer/NFL/NBA/NHL/MLB).
//!
//! Each fetcher produces a flat list of [`RawSeries`] — one per playoff series (a
//! best-of-N in the series sports, a single game in soccer/NFL) — tagged with a
//! `section` (which half of the bracket: a conference/league, `""` for a single
//! elimination tree, `"final"` for the championship, `"third"` for a third-place
//! match), a per-section `round` index (0 = earliest), and a `slot` (its top-to-
//! bottom position within that round). [`build`] orders those into the flat
//! column list the [`crate::bracket`] layout expects and derives each match's
//! `feeders` — the `(round, index)` coordinates of the earlier matches feeding it.
//!
//! Feeders per side are resolved in priority order:
//!   1. an explicit `feed` the fetcher supplied (e.g. parsed from ESPN's
//!      "Quarterfinal 1 Winner" placeholder, or NHL's series-letter tree);
//!   2. **participant-matching** — the earlier series whose *winner* is this
//!      side's team. This is reseed-proof (NFL) and needs no positional template,
//!      and it also converges the `"final"` onto whichever conference finals the
//!      two finalists actually came through;
//!   3. **structural** — for still-undecided future matches with no team yet: the
//!      standard halving tree within a section (round `r` slot `2k`/`2k+1` feed
//!      round `r+1` slot `k`), and the `"final"` fed by the two conference
//!      sections' last rounds — so a future round still draws its connectors.
//!
//! The module is pure (no I/O) and unit-tested with synthetic series.

use crate::types::{BracketMatch, BracketRound};
use std::collections::{HashMap, HashSet};

/// A feeder reference by `(section, round, slot)` — the coordinates a fetcher
/// uses to point at another series before global positions are assigned.
pub type FeederRef = (String, u32, u32);

/// One playoff series, as produced by a sport fetcher.
#[derive(Clone, Debug, Default)]
pub struct RawSeries {
    /// Bracket half: a conference/league key (e.g. `"east"`, `"afc"`, `"al"`),
    /// `""` for a single elimination tree (the World Cup), `"final"` for the
    /// championship, or `"third"` for a third-place match.
    pub section: String,
    /// 0-based round within the section (0 = earliest knockout round).
    pub round: u32,
    /// 0-based top-to-bottom position within `(section, round)`.
    pub slot: u32,
    /// Column heading, e.g. "Round of 16", "Divisional", "Conference Finals".
    pub round_title: String,
    pub team_a: String,
    pub team_b: String,
    /// Games won in the series (series sports) or goals/points (single game).
    pub score_a: Option<i64>,
    pub score_b: Option<i64>,
    /// "a" / "b" once decided, else "".
    pub winner: String,
    /// A representative game id for the box's match link (0 = unlinked).
    pub match_id: i64,
    /// Optional explicit feeder per side `[a, b]`; `None` → derive it.
    pub feed: [Option<FeederRef>; 2],
}

impl RawSeries {
    /// The advancing team's name, or empty while undecided.
    fn winner_team(&self) -> &str {
        match self.winner.as_str() {
            "a" => &self.team_a,
            "b" => &self.team_b,
            _ => "",
        }
    }
}

/// A team slot is "real" (a decided participant we can match on) when it names an
/// actual team rather than a placeholder.
fn is_real_team(t: &str) -> bool {
    !t.is_empty() && !t.eq_ignore_ascii_case("tbd")
}

/// Section display order: normal sections in first-appearance order, then the
/// `"final"`, then `"third"` last — so conference brackets stack, the
/// championship sits to their right, and a third-place match trails.
fn section_rank(section: &str, first_seen: &HashMap<String, usize>) -> (u8, usize) {
    match section {
        "final" => (1, 0),
        "third" => (2, 0),
        s => (0, first_seen.get(s).copied().unwrap_or(usize::MAX)),
    }
}

/// Turn a flat list of playoff series into the flat column list
/// (`Vec<BracketRound>`, section-major then round-ascending) that the bracket
/// renderer lays out, with each match's `feeders` filled in.
#[must_use]
pub fn build(series: Vec<RawSeries>) -> Vec<BracketRound> {
    if series.is_empty() {
        return Vec::new();
    }

    // Section order (first-appearance for the conference halves, final, third).
    let mut first_seen: HashMap<String, usize> = HashMap::new();
    for (i, s) in series.iter().enumerate() {
        first_seen.entry(s.section.clone()).or_insert(i);
    }
    let mut sections: Vec<String> = first_seen.keys().cloned().collect();
    sections.sort_by_key(|s| section_rank(s, &first_seen));

    // The two conference/league sections (everything but final/third), in order —
    // used to feed an undecided championship structurally.
    let conf_sections: Vec<String> = sections
        .iter()
        .filter(|s| s.as_str() != "final" && s.as_str() != "third")
        .cloned()
        .collect();

    // Columns: one per (section, round), section-major then round-ascending. Each
    // column holds its series sorted by slot (top-to-bottom).
    let mut columns: Vec<(String, u32, Vec<usize>)> = Vec::new();
    for sec in &sections {
        let mut rounds: Vec<u32> = series
            .iter()
            .filter(|s| &s.section == sec)
            .map(|s| s.round)
            .collect();
        rounds.sort_unstable();
        rounds.dedup();
        for r in rounds {
            let mut idxs: Vec<usize> = (0..series.len())
                .filter(|&i| series[i].section == *sec && series[i].round == r)
                .collect();
            idxs.sort_by_key(|&i| series[i].slot);
            columns.push((sec.clone(), r, idxs));
        }
    }

    // Reorder each section's first-round matches into the bracket's planar order,
    // so two feeders always converge cleanly into one and the connectors don't
    // cross. The layout positions a match at the mean of its feeders and hands out
    // first-round slots in input order, so only the leaves' order matters. We
    // resolve feeders to series references (independent of the positions we're
    // about to assign) and walk the tree in order from each section's root; the
    // coordinate map below then remaps every feeder to the new positions for free.
    {
        let by_srs: HashMap<(String, u32, u32), usize> = series
            .iter()
            .enumerate()
            .map(|(i, s)| ((s.section.clone(), s.round, s.slot), i))
            .collect();
        // The in-section feeder series for one side, if any: an explicit ref, else
        // the earlier same-section match this side's team won, else the halving-tree
        // slot. A cross-section feeder (a championship's conference finals) is a
        // section boundary — not followed — so each section orders its own leaves.
        let feeder = |si: usize, side: usize| -> Option<usize> {
            let s = &series[si];
            let cand = if let Some((fsec, fr, fslot)) = &s.feed[side] {
                by_srs.get(&(fsec.clone(), *fr, *fslot)).copied()
            } else {
                let team = if side == 0 { &s.team_a } else { &s.team_b };
                if is_real_team(team) {
                    (0..series.len())
                        .filter(|&j| {
                            series[j].section == s.section
                                && series[j].round < s.round
                                && series[j].winner_team() == team
                        })
                        .max_by_key(|&j| series[j].round)
                } else if s.round > 0 {
                    by_srs
                        .get(&(s.section.clone(), s.round - 1, s.slot * 2 + side as u32))
                        .copied()
                } else {
                    None
                }
            };
            cand.filter(|&j| series[j].section == s.section)
        };
        for (sec, r, idxs) in columns.iter_mut() {
            if *r != 0 {
                continue; // only the leaves (first round) drive the ordering
            }
            let max_round = series
                .iter()
                .filter(|s| &s.section == sec)
                .map(|s| s.round)
                .max()
                .unwrap_or(0);
            let mut roots: Vec<usize> = (0..series.len())
                .filter(|&i| series[i].section == *sec && series[i].round == max_round)
                .collect();
            roots.sort_by_key(|&i| series[i].slot);
            // In-order DFS from the root(s), collecting childless (leaf) series
            // left-to-right — the planar leaf order.
            let mut order: Vec<usize> = Vec::new();
            let mut seen: HashSet<usize> = HashSet::new();
            let mut stack: Vec<usize> = roots.into_iter().rev().collect();
            while let Some(si) = stack.pop() {
                if !seen.insert(si) {
                    continue;
                }
                match (feeder(si, 0), feeder(si, 1)) {
                    (None, None) => order.push(si),
                    (f0, f1) => {
                        if let Some(c) = f1 {
                            stack.push(c);
                        }
                        if let Some(c) = f0 {
                            stack.push(c);
                        }
                    }
                }
            }
            // Apply only if it's a clean permutation of this column (safety net).
            let planar: Vec<usize> = order.iter().copied().filter(|i| idxs.contains(i)).collect();
            if planar.len() == idxs.len() {
                *idxs = planar;
            }
        }
    }

    // Coordinate maps: (section, round, slot) → (col, pos), and the last round
    // number present in each section (for the structural final edge).
    let mut coord: HashMap<(String, u32, u32), (usize, usize)> = HashMap::new();
    let mut last_round: HashMap<String, u32> = HashMap::new();
    for (col, (sec, r, idxs)) in columns.iter().enumerate() {
        last_round
            .entry(sec.clone())
            .and_modify(|lr| *lr = (*lr).max(*r))
            .or_insert(*r);
        for (pos, &i) in idxs.iter().enumerate() {
            coord.insert((sec.clone(), *r, series[i].slot), (col, pos));
        }
    }

    // The column each series ended up in (for winner-based lookups by team).
    let mut col_of_series: HashMap<usize, usize> = HashMap::new();
    for (col, (_, _, idxs)) in columns.iter().enumerate() {
        for &i in idxs {
            col_of_series.insert(i, col);
        }
    }

    // Resolve one side's feeder to a global (col, pos).
    let resolve_side = |si: usize, side: usize| -> Option<(usize, usize)> {
        let s = &series[si];
        // 1. Explicit feeder ref from the fetcher.
        if let Some((fsec, fr, fslot)) = &s.feed[side] {
            return coord.get(&(fsec.clone(), *fr, *fslot)).copied();
        }
        let this_col = col_of_series[&si];
        let team = if side == 0 { &s.team_a } else { &s.team_b };
        // 2. Participant match: the closest earlier column whose winner is us.
        if is_real_team(team) {
            let mut best: Option<(usize, usize)> = None;
            for (col, (_, _, idxs)) in columns.iter().enumerate() {
                if col >= this_col {
                    break;
                }
                for (pos, &j) in idxs.iter().enumerate() {
                    if series[j].winner_team() == team {
                        best = Some((col, pos)); // later earlier col wins (closest)
                    }
                }
            }
            if best.is_some() {
                return best;
            }
        }
        // 3. Structural fallback for undecided future matches.
        if s.section == "final" {
            // Championship side a ← first conference's final, b ← second's.
            let sec = conf_sections.get(side)?;
            let lr = *last_round.get(sec)?;
            return coord.get(&(sec.clone(), lr, 0)).copied();
        }
        if s.round > 0 {
            let fslot = s.slot * 2 + side as u32;
            return coord.get(&(s.section.clone(), s.round - 1, fslot)).copied();
        }
        None
    };

    columns
        .iter()
        .map(|(sec, _r, idxs)| {
            let title = idxs
                .iter()
                .find_map(|&i| {
                    (!series[i].round_title.is_empty()).then(|| series[i].round_title.clone())
                })
                .unwrap_or_default();
            let matches = idxs
                .iter()
                .map(|&i| {
                    let s = &series[i];
                    let mut feeders = Vec::new();
                    for side in 0..2 {
                        if let Some(c) = resolve_side(i, side) {
                            if !feeders.contains(&c) {
                                feeders.push(c);
                            }
                        }
                    }
                    BracketMatch {
                        team_a: s.team_a.clone(),
                        team_b: s.team_b.clone(),
                        score_a: s.score_a,
                        score_b: s.score_b,
                        winner: s.winner.clone(),
                        match_id: s.match_id,
                        feeders,
                    }
                })
                .collect();
            BracketRound {
                title: title.clone(),
                matches,
                section: sec.clone(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a decided single-game series with no explicit feeders.
    fn game(section: &str, round: u32, slot: u32, a: &str, b: &str, win: &str) -> RawSeries {
        RawSeries {
            section: section.to_string(),
            round,
            slot,
            round_title: String::new(),
            team_a: a.to_string(),
            team_b: b.to_string(),
            score_a: Some(if win == "a" { 1 } else { 0 }),
            score_b: Some(if win == "b" { 1 } else { 0 }),
            winner: win.to_string(),
            match_id: 0,
            feed: [None, None],
        }
    }

    fn feeders_of(
        rounds: &[BracketRound],
        sec: &str,
        round_pos: usize,
        i: usize,
    ) -> Vec<(usize, usize)> {
        let col = rounds
            .iter()
            .enumerate()
            .filter(|(_, r)| r.section == sec)
            .nth(round_pos)
            .expect("column")
            .0;
        rounds[col].matches[i].feeders.clone()
    }

    fn global_col(rounds: &[BracketRound], sec: &str, round_pos: usize) -> usize {
        rounds
            .iter()
            .enumerate()
            .filter(|(_, r)| r.section == sec)
            .nth(round_pos)
            .expect("column")
            .0
    }

    #[test]
    fn two_conference_bracket_converges_into_final() {
        // Each conference: 2 first-round series → 1 conference final. Then a final.
        let series = vec![
            game("east", 0, 0, "E1", "E4", "a"),
            game("east", 0, 1, "E2", "E3", "b"),
            game("east", 1, 0, "E1", "E3", "a"), // East final: E1 beats E3
            game("west", 0, 0, "W1", "W4", "a"),
            game("west", 0, 1, "W2", "W3", "a"),
            game("west", 1, 0, "W1", "W2", "b"), // West final: W2 beats W1
            game("final", 0, 0, "E1", "W2", "a"), // champion E1
        ];
        let rounds = build(series);
        // East final (round pos 1) fed by its two first-round games.
        let east_r0 = global_col(&rounds, "east", 0);
        let ef = feeders_of(&rounds, "east", 1, 0);
        assert_eq!(ef.len(), 2);
        assert!(ef.iter().all(|(c, _)| *c == east_r0));
        // The championship is fed by the two conference finals (participant-matched
        // to whichever conference each finalist came through).
        let final_feed = feeders_of(&rounds, "final", 0, 0);
        assert_eq!(final_feed.len(), 2);
        let east_final_col = global_col(&rounds, "east", 1);
        let west_final_col = global_col(&rounds, "west", 1);
        let cols: Vec<usize> = final_feed.iter().map(|(c, _)| *c).collect();
        assert!(cols.contains(&east_final_col) && cols.contains(&west_final_col));
    }

    #[test]
    fn reseeding_resolved_by_participant_match() {
        // A conference where structural 2k/2k+1 pairing would be WRONG: round-1
        // participants force the feeders. r0: three winners A, C, top-seed T (bye).
        // r1 s0: T vs C (T beat C in structural terms? no — participants say so).
        let series = vec![
            game("afc", 0, 0, "A", "B", "a"), // A advances
            game("afc", 0, 1, "C", "D", "a"), // C advances
            game("afc", 0, 2, "E", "F", "b"), // F advances
            // Divisional (reseeded): top seed T plays lowest — here A vs F, C vs G.
            RawSeries {
                winner: "a".into(),
                ..game("afc", 1, 0, "A", "F", "a")
            },
            RawSeries {
                winner: "a".into(),
                ..game("afc", 1, 1, "C", "G", "a")
            },
        ];
        let rounds = build(series);
        let r0 = global_col(&rounds, "afc", 0);
        // Divisional s0 is A vs F — participant-matched to the first-round games A
        // and F won (NOT a structural 2k/2k+1 pairing). The planar reorder then
        // makes those two leaves adjacent so the connectors don't cross.
        let mut f0 = feeders_of(&rounds, "afc", 1, 0);
        f0.sort();
        assert_eq!(f0, vec![(r0, 0), (r0, 1)], "A and F, made adjacent");
        // "G" never appears as a winner, so divisional s1 only matches C.
        let f1 = feeders_of(&rounds, "afc", 1, 1);
        assert_eq!(f1, vec![(r0, 2)], "C only (G never won)");
    }

    #[test]
    fn first_round_reorders_to_a_planar_tree() {
        // A 4-leaf single-elim tree whose middle round crosses over: r1 slot0 is fed
        // by r0 slots 0 & 2 (not 0 & 1). Given the leaves in slot order, build() must
        // reorder them so each middle match's two feeders end up adjacent (2i/2i+1)
        // — i.e. no crossing connectors.
        let leaf = |slot: u32| RawSeries {
            round: 0,
            slot,
            ..Default::default()
        };
        let mid = |slot: u32, f0: u32, f1: u32| RawSeries {
            round: 1,
            slot,
            feed: [Some((String::new(), 0, f0)), Some((String::new(), 0, f1))],
            ..Default::default()
        };
        let series = vec![
            leaf(0),
            leaf(1),
            leaf(2),
            leaf(3),
            mid(0, 0, 2), // crossover: fed by r0 slots 0 & 2
            mid(1, 1, 3),
            RawSeries {
                round: 2,
                slot: 0,
                feed: [Some((String::new(), 1, 0)), Some((String::new(), 1, 1))],
                ..Default::default()
            },
        ];
        let rounds = build(series);
        // rounds[0] = leaves (col 0), rounds[1] = middle (col 1).
        let mut m0 = rounds[1].matches[0].feeders.clone();
        m0.sort();
        let mut m1 = rounds[1].matches[1].feeders.clone();
        m1.sort();
        assert_eq!(
            m0,
            vec![(0, 0), (0, 1)],
            "mid 0 fed by an adjacent leaf pair"
        );
        assert_eq!(m1, vec![(0, 2), (0, 3)], "mid 1 fed by the next pair");
    }

    #[test]
    fn undecided_future_round_uses_structural_edges() {
        // First round decided, semifinal not yet (placeholder teams) → structural
        // 2k/2k+1 pairing still connects the future column.
        let series = vec![
            game("west", 0, 0, "W1", "W4", "a"),
            game("west", 0, 1, "W2", "W3", ""), // undecided
            RawSeries {
                team_a: "TBD".into(),
                team_b: "TBD".into(),
                winner: "".into(),
                score_a: None,
                score_b: None,
                ..game("west", 1, 0, "", "", "")
            },
        ];
        let rounds = build(series);
        let sf = feeders_of(&rounds, "west", 1, 0);
        let r0 = global_col(&rounds, "west", 0);
        assert_eq!(
            sf.len(),
            2,
            "structural pairing feeds both first-round slots"
        );
        assert!(sf.contains(&(r0, 0)) && sf.contains(&(r0, 1)));
    }

    #[test]
    fn single_tree_with_clean_third_place() {
        // A tiny World-Cup-style single-elim tree (SF → Final) plus a lone
        // third-place match in its own section, with no drawn feeders.
        let series = vec![
            game("", 0, 0, "A", "B", "a"),
            game("", 0, 1, "C", "D", "b"),
            game("", 1, 0, "A", "D", "a"),      // Final
            game("third", 0, 0, "B", "C", "a"), // 3rd place, feeder-less
        ];
        let rounds = build(series);
        // Final fed by both semifinals.
        let ff = feeders_of(&rounds, "", 1, 0);
        assert_eq!(ff.len(), 2);
        // Third-place match sits alone in its own section with no connectors.
        let tf = feeders_of(&rounds, "third", 0, 0);
        assert!(tf.is_empty(), "third-place match has no drawn feeders");
        // Sections ordered so "third" is the last column.
        assert_eq!(rounds.last().unwrap().section, "third");
    }

    #[test]
    fn explicit_feed_takes_precedence() {
        let mut s = game("", 1, 0, "A", "D", "a");
        s.feed = [Some(("".into(), 0, 1)), Some(("".into(), 0, 0))];
        let series = vec![
            game("", 0, 0, "A", "B", "a"),
            game("", 0, 1, "C", "D", "b"),
            s,
        ];
        let rounds = build(series);
        let ff = feeders_of(&rounds, "", 1, 0);
        let r0 = global_col(&rounds, "", 0);
        // Explicit refs used verbatim (slot1 for side a, slot0 for side b).
        assert!(ff.contains(&(r0, 1)) && ff.contains(&(r0, 0)));
    }
}
