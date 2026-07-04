//! Deterministic elimination-bracket layout.
//!
//! Computes a position for every match from the `feeders` graph already carried
//! by [`BracketRound`] (each feeder is a `(round, match)` grid coord in a
//! strictly-earlier column). The renderer turns these positions into absolutely
//! placed boxes and an SVG overlay of connector lines, so connectors stay
//! attached at any shape or zoom — unlike the old `nth-child(odd/even)` CSS that
//! assumed a perfect power-of-2 halving tree.
//!
//! The function is pure and runs identically on the server (SSR) and the client
//! (hydrate) from the same serialized data, so the rendered geometry byte-matches.
//!
//! Units are "slots": one slot is one box pitch. The renderer multiplies a slot
//! `y` by [`ROW_EM`] and a column by [`COL_EM`]; these em values MUST match the
//! `--bk-*` custom properties in `style/main.scss` (cross-referenced there).

use crate::types::BracketRound;
use std::collections::HashSet;

/// Slot-to-slot (box pitch) distance, in `em` (box height + a connector gutter).
pub const ROW_EM: f64 = 5.0;
/// Box height, in `em` (two rows, one per team; fixed so a slot maps to a known
/// centre). Must match `--bk-box-h` in `style/main.scss`.
pub const BOX_H_EM: f64 = 3.8;

/// Gap (em) between a box's right edge and the next column, for the connector.
const COL_GAP_EM: f64 = 1.9;
/// Per-character width estimate (em) used to size a bracket's boxes to its names.
const NAME_CHAR_EM: f64 = 0.62;
/// Room in a box beside the name: its padding, the gap, and the score column.
const NAME_PAD_EM: f64 = 3.2;
/// Box-width bounds: a floor for short (esports) names, and a ceiling past which a
/// long name ellipsises rather than widening the whole bracket.
const MIN_BOX_W_EM: f64 = 6.0;
const MAX_BOX_W_EM: f64 = 13.0;

/// The box width (em) that fits this bracket's longest team name, clamped to
/// `[MIN_BOX_W_EM, MAX_BOX_W_EM]`. Every box uses it, so the bracket stays aligned
/// — sized to the names of *this* bracket, not fixed site-wide; a name past the
/// ceiling is ellipsised by the CSS.
#[must_use]
pub fn box_width_em(rounds: &[BracketRound]) -> f64 {
    let longest = rounds
        .iter()
        .flat_map(|r| &r.matches)
        .flat_map(|m| [m.team_a.chars().count(), m.team_b.chars().count()])
        .max()
        .unwrap_or(0);
    (longest as f64 * NAME_CHAR_EM + NAME_PAD_EM).clamp(MIN_BOX_W_EM, MAX_BOX_W_EM)
}

/// Column-to-column distance (em) for a bracket whose boxes are `box_w` wide.
#[must_use]
pub fn col_em(box_w: f64) -> f64 {
    box_w + COL_GAP_EM
}

/// Blank slots inserted between the stacked sections of a double-elim bracket —
/// room for the lower bracket's banner + the grand final, which sits in the gap.
const SECTION_GAP: f64 = 2.5;

/// Slots the third-place match sits below the lowest box of the final + semifinal
/// columns — clear of (not row-aligned with) the semifinals, with room for its own
/// title and a little breathing space.
const THIRD_GAP: f64 = 1.8;

/// A match's grid column and vertical centre (in slot units).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pos {
    pub col: usize,
    pub y: f64,
}

/// A connector to draw: from a feeder match's right edge to the fed match's left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    pub from: (usize, usize),
    pub to: (usize, usize),
}

#[derive(Clone, Debug, Default)]
pub struct BracketLayout {
    /// Parallel to `rounds`: `positions[r][i]` is match `i` of round `r`.
    pub positions: Vec<Vec<Pos>>,
    /// Connectors to draw (within-section tree edges + grand-final edges; the
    /// winner's-bracket-loser → loser's-bracket drop-downs are intentionally not
    /// drawn, matching how printed double-elim brackets look).
    pub edges: Vec<Edge>,
    /// Total columns (canvas width = `cols * COL_EM`).
    pub cols: usize,
    /// Columns occupied by the stacked winner's/loser's brackets (i.e. excluding
    /// the grand final to their right) — so a section banner can stop before the
    /// grand-final rail rather than crossing it. Equals `cols` for single-elim.
    pub bracket_cols: usize,
    /// Total height in slots (canvas height = `height * ROW_EM`).
    pub height: f64,
    /// `(section, y_min, y_max)` per consecutive section run, for the labels.
    pub group_rows: Vec<(String, f64, f64)>,
}

/// Lay a bracket out from its rounds. See the module docs.
#[must_use]
pub fn layout(rounds: &[BracketRound]) -> BracketLayout {
    let n = rounds.len();
    if n == 0 {
        return BracketLayout::default();
    }
    let section: Vec<&str> = rounds.iter().map(|r| r.section.as_str()).collect();

    // Consecutive section runs (single-elim is one "" run).
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    for (r, s) in section.iter().enumerate() {
        match groups.last_mut() {
            Some((gs, idxs)) if gs == s => idxs.push(r),
            _ => groups.push(((*s).to_string(), vec![r])),
        }
    }

    // Columns. Winner's/loser's brackets are right-aligned so both finals land in
    // the same (rightmost) column next to the grand final, whatever each bracket's
    // length; the grand final's columns follow.
    let non_final_cols = groups
        .iter()
        .filter(|(s, _)| s != "final")
        .map(|(_, idxs)| idxs.len())
        .max()
        .unwrap_or(0);
    let mut col_of = vec![0usize; n];
    for (s, idxs) in &groups {
        if s == "final" {
            for (local, &r) in idxs.iter().enumerate() {
                col_of[r] = non_final_cols + local;
            }
        } else {
            let offset = non_final_cols - idxs.len();
            for (local, &r) in idxs.iter().enumerate() {
                col_of[r] = offset + local;
            }
        }
    }

    let mut positions: Vec<Vec<Pos>> = rounds
        .iter()
        .enumerate()
        .map(|(r, round)| {
            round
                .matches
                .iter()
                .map(|_| Pos {
                    col: col_of[r],
                    y: 0.0,
                })
                .collect()
        })
        .collect();

    // Vertical layout. Each section is laid out top-to-bottom; a `cursor` hands
    // out leaf slots and is bumped past each finished section (+ a gap) so the
    // next section stacks below. A match's centre is the mean of its in-section
    // feeders (the grand final means across both bracket finals), so it sits
    // between its parents; a per-column min-gap pass keeps boxes from overlapping.
    let mut group_rows: Vec<(String, f64, f64)> = Vec::new();
    let mut cursor = 0.0_f64;
    for (s, idxs) in &groups {
        let is_final = s == "final";
        // A third-place match sits in the lower-right, beneath the final/semifinal
        // area, rather than stacked at the very bottom of the whole tree. It shares
        // the final's (rightmost) column via the column pass above, so only its row
        // needs placing: anchor it below the lowest box of the final and semifinal
        // columns so it clears the semifinals instead of lining up with one.
        if s == "third" {
            let placed = || {
                positions
                    .iter()
                    .enumerate()
                    .filter(|(rr, _)| section[*rr] != "third")
                    .flat_map(|(_, row)| row.iter())
            };
            let final_col = placed().map(|p| p.col).max().unwrap_or(0);
            // The lowest box in the final column and the one to its left (the
            // semifinals) — everything further left is a taller earlier round we
            // don't want to drop below.
            let anchor = placed()
                .filter(|p| p.col + 1 >= final_col)
                .map(|p| p.y)
                .fold(f64::NEG_INFINITY, f64::max);
            let base = if anchor.is_finite() { anchor } else { cursor };
            let mut k = 1.0;
            for &r in idxs {
                for i in 0..rounds[r].matches.len() {
                    positions[r][i].y = base + THIRD_GAP * k;
                    k += 1.0;
                }
            }
            group_rows.push((s.clone(), base + THIRD_GAP, base + THIRD_GAP * (k - 1.0)));
            continue;
        }
        for &r in idxs {
            let mlen = rounds[r].matches.len();
            for i in 0..mlen {
                let parents: Vec<(usize, usize)> = rounds[r].matches[i]
                    .feeders
                    .iter()
                    .copied()
                    .filter(|&(fr, _)| is_final || section[fr] == s.as_str())
                    .collect();
                positions[r][i].y = if parents.is_empty() {
                    let slot = cursor;
                    cursor += 1.0;
                    slot
                } else {
                    parents
                        .iter()
                        .map(|&(fr, fi)| positions[fr][fi].y)
                        .sum::<f64>()
                        / parents.len() as f64
                };
            }
            // Resolve overlaps within the column (centres at least one slot apart).
            let mut order: Vec<usize> = (0..mlen).collect();
            order.sort_by(|&a, &b| positions[r][a].y.total_cmp(&positions[r][b].y));
            for w in 1..order.len() {
                let need = positions[r][order[w - 1]].y + 1.0;
                if positions[r][order[w]].y < need {
                    positions[r][order[w]].y = need;
                }
            }
        }
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for &r in idxs {
            for p in &positions[r] {
                lo = lo.min(p.y);
                hi = hi.max(p.y);
            }
        }
        if lo.is_finite() {
            group_rows.push((s.clone(), lo, hi));
            cursor = hi + 1.0 + SECTION_GAP;
        }
    }

    // Connectors: every feeder within a section, plus the grand final's edges to
    // both bracket finals. Skip winner→loser drop-downs (cross-section into lower).
    let mut edges = Vec::new();
    for r in 0..n {
        for i in 0..rounds[r].matches.len() {
            for &(fr, fi) in &rounds[r].matches[i].feeders {
                if section[r] == "final" || section[fr] == section[r] {
                    edges.push(Edge {
                        from: (fr, fi),
                        to: (r, i),
                    });
                }
            }
        }
    }

    let cols = col_of.iter().copied().max().map_or(0, |c| c + 1);
    let bracket_cols = non_final_cols.max(1);
    let height = positions.iter().flatten().map(|p| p.y).fold(0.0, f64::max) + 1.0;
    BracketLayout {
        positions,
        edges,
        cols,
        bracket_cols,
        height,
        group_rows,
    }
}

/// Right edge (in em) of the last winner's/loser's-bracket column — where a
/// section banner should end so it never reaches the grand-final rail. Takes this
/// bracket's `col_em`/`box_w` (its boxes are sized to its names).
#[must_use]
pub fn banner_width_em(bracket_cols: usize, col: f64, box_w: f64) -> f64 {
    (bracket_cols.max(1) - 1) as f64 * col + box_w
}

/// The match's lineage — itself, all its ancestors (feeders, transitively) and
/// all its descendants (matches it feeds) — for highlighting a path on hover.
/// (Not wired into the UI yet; provided for a follow-up.)
#[must_use]
pub fn lineage(layout: &BracketLayout, start: (usize, usize)) -> HashSet<(usize, usize)> {
    let mut set = HashSet::new();
    set.insert(start);
    let mut frontier = vec![start];
    while let Some(node) = frontier.pop() {
        for e in &layout.edges {
            if e.from == node && set.insert(e.to) {
                frontier.push(e.to);
            }
        }
    }
    let mut frontier = vec![start];
    while let Some(node) = frontier.pop() {
        for e in &layout.edges {
            if e.to == node && set.insert(e.from) {
                frontier.push(e.from);
            }
        }
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{BracketMatch, BracketRound};

    fn m(feeders: &[(usize, usize)]) -> BracketMatch {
        BracketMatch {
            feeders: feeders.to_vec(),
            ..Default::default()
        }
    }
    fn round(section: &str, matches: Vec<BracketMatch>) -> BracketRound {
        BracketRound {
            title: String::new(),
            matches,
            section: section.to_string(),
        }
    }
    fn y(l: &BracketLayout, r: usize, i: usize) -> f64 {
        l.positions[r][i].y
    }
    fn has_edge(l: &BracketLayout, from: (usize, usize), to: (usize, usize)) -> bool {
        l.edges.contains(&Edge { from, to })
    }

    #[test]
    fn clean_single_elim_centres_each_round() {
        // 4 → 2 → 1.
        let rounds = vec![
            round("", vec![m(&[]), m(&[]), m(&[]), m(&[])]),
            round("", vec![m(&[(0, 0), (0, 1)]), m(&[(0, 2), (0, 3)])]),
            round("", vec![m(&[(1, 0), (1, 1)])]),
        ];
        let l = layout(&rounds);
        assert_eq!(
            [y(&l, 0, 0), y(&l, 0, 1), y(&l, 0, 2), y(&l, 0, 3)],
            [0.0, 1.0, 2.0, 3.0]
        );
        assert_eq!([y(&l, 1, 0), y(&l, 1, 1)], [0.5, 2.5]);
        assert_eq!(y(&l, 2, 0), 1.5);
        assert_eq!(l.edges.len(), 6);
        assert_eq!(l.cols, 3);
        // Columns are just the round index for single-elim.
        assert_eq!(l.positions[2][0].col, 2);
    }

    #[test]
    fn double_elim_stacks_and_centres_grand_final() {
        // upper: r0 (2) → r1 final; lower: r2 (LB-R1, fed by WB losers) → r3 final
        // (fed by LB-R1 + a WB-final loser); final: r4 grand (upper-final + lower-final).
        let rounds = vec![
            round("upper", vec![m(&[]), m(&[])]),
            round("upper", vec![m(&[(0, 0), (0, 1)])]),
            round("lower", vec![m(&[(0, 0), (0, 1)])]),
            round("lower", vec![m(&[(2, 0), (1, 0)])]),
            round("final", vec![m(&[(1, 0), (3, 0)])]),
        ];
        let l = layout(&rounds);
        // Upper region near the top.
        assert_eq!(y(&l, 1, 0), 0.5);
        // Lower region below the gap (its only leaf takes the next free slot).
        let lb = y(&l, 2, 0);
        assert!(lb > 1.5, "lower bracket should stack below upper, got {lb}");
        assert_eq!(
            y(&l, 3, 0),
            lb,
            "LB final sits on its single in-section feeder"
        );
        // Grand final centred between the two bracket finals.
        assert_eq!(y(&l, 4, 0), (0.5 + lb) / 2.0);
        // Both finals right-aligned to the same column; grand final follows.
        assert_eq!(l.positions[1][0].col, l.positions[3][0].col);
        assert_eq!(l.positions[4][0].col, l.positions[1][0].col + 1);
        // Drop-downs (WB → LB) are not drawn; grand-final edges are.
        assert!(!has_edge(&l, (0, 0), (2, 0)));
        assert!(!has_edge(&l, (1, 0), (3, 0)));
        assert!(has_edge(&l, (2, 0), (3, 0)));
        assert!(has_edge(&l, (1, 0), (4, 0)));
        assert!(has_edge(&l, (3, 0), (4, 0)));
    }

    #[test]
    fn lone_feeder_has_no_phantom_edge() {
        // 3 leaves; round 1 has a normal pair and a single-fed (bye) match.
        let rounds = vec![
            round("", vec![m(&[]), m(&[]), m(&[])]),
            round("", vec![m(&[(0, 0), (0, 1)]), m(&[(0, 2)])]),
        ];
        let l = layout(&rounds);
        assert_eq!(y(&l, 1, 0), 0.5);
        assert_eq!(y(&l, 1, 1), 2.0, "lone-fed match sits on its single feeder");
        assert_eq!(l.edges.iter().filter(|e| e.to == (1, 1)).count(), 1);
    }

    #[test]
    fn colliding_centres_are_pushed_apart() {
        // Two round-1 matches that resolve to the same centre must separate.
        let rounds = vec![
            round("", vec![m(&[]), m(&[]), m(&[]), m(&[])]),
            round("", vec![m(&[(0, 1), (0, 2)]), m(&[(0, 1), (0, 2)])]),
        ];
        let l = layout(&rounds);
        let (a, b) = (y(&l, 1, 0), y(&l, 1, 1));
        assert!(
            (a - b).abs() >= 1.0,
            "centres must be >= 1 slot apart, got {a} and {b}"
        );
    }

    #[test]
    fn third_place_sits_below_the_final_clear_of_the_semis() {
        // A single-elim tree (2 semifinals → final) plus a feeder-less third-place
        // match in its own "third" section. The third box shares the final's column
        // and sits below the lower semifinal — clear of the semifinal row, not
        // stacked far beneath the whole tree.
        let rounds = vec![
            round("", vec![m(&[]), m(&[])]),
            round("", vec![m(&[(0, 0), (0, 1)])]),
            round("third", vec![m(&[])]),
        ];
        let l = layout(&rounds);
        let final_y = y(&l, 1, 0);
        let lower_sf = y(&l, 0, 0).max(y(&l, 0, 1));
        let third_y = y(&l, 2, 0);
        assert!(third_y > final_y, "third is below the final");
        assert!(third_y > lower_sf, "third clears the lower semifinal");
        assert!(
            (third_y - lower_sf - THIRD_GAP).abs() < 0.01,
            "third sits one THIRD_GAP below the lower semifinal"
        );
        assert_eq!(
            l.positions[2][0].col, l.positions[1][0].col,
            "third shares the final's column"
        );
    }
}
