//! Standings tables, the Swiss buchholz grid, and the single/double-elimination
//! bracket — plus the shared, persisted progressive-reveal machinery they all
//! use. Split out of the page components in `super` (which render these via the
//! `pub(crate)` entry points `StandingsTable`, `SwissBracket`, `Bracket`).

use super::*;
use crate::types::{BracketMatch, BracketRound, Sport, StandingRow, SwissMatch, SwissRound};
use std::collections::HashSet;

/// Reveal state for a persisted section key (standings / one bracket round),
/// OR'd with the global "show scores". Returns `(revealed, toggle, toggle-hidden)`
/// where the toggle is shared across every page showing this same section.
pub(crate) fn section_reveal(key: String) -> (Memo<bool>, impl Fn(leptos::ev::MouseEvent) + Copy) {
    let global = use_context::<ShowScores>().map(|s| s.0);
    let flash = use_context::<FlashScores>().map(|f| f.0);
    let sections = use_context::<RevealedSections>().map(|s| s.0);
    let key = StoredValue::new(key);
    let revealed = Memo::new(move |_| {
        global.is_some_and(|g| g.get())
            || sections.is_some_and(|s| s.with(|set| set.contains(&key.get_value())))
    });
    // The end (for pruning) to record with this section's reveal — the page's
    // RevealEnd (the event's last match / GP weekend / latest race).
    let end = current_reveal_end();
    #[cfg(not(feature = "hydrate"))]
    let _ = end;
    // Viewing counts as interaction: refresh this section's record on mount if it's
    // a stored reveal, so anything you still look at stays revealed.
    #[cfg(feature = "hydrate")]
    Effect::new(move |_| {
        let k = key.get_value();
        if sections.is_some_and(|s| s.with_untracked(|set| set.contains(&k))) {
            touch_reveals(keys::SECTIONS, &[(k, end)], now_ms());
        }
    });
    let toggle = move |_: leptos::ev::MouseEvent| {
        // While the global reveal is on, a hide can't take effect — flash the
        // "scores" button to point there instead of silently doing nothing.
        if hide_deflected_to_global(global, flash) {
            return;
        }
        if let Some(s) = sections {
            let k = key.get_value();
            let was = s.with_untracked(|set| set.contains(&k));
            s.update(|set| {
                if was {
                    set.remove(&k);
                } else {
                    set.insert(k.clone());
                }
            });
            #[cfg(feature = "hydrate")]
            if was {
                remove_reveal(keys::SECTIONS, &k);
            } else {
                touch_reveals(keys::SECTIONS, &[(k, end)], now_ms());
            }
        }
    };
    (revealed, toggle)
}

/// Sync the localStorage section records after a bracket op mutated the in-memory
/// set: record (with `end`, fresh touch) the keys it revealed, and drop the records
/// for the keys it hid. Only the changed keys are touched, so an unrelated reveal
/// elsewhere isn't refreshed.
#[cfg(feature = "hydrate")]
fn sync_section_diff(before: &HashSet<String>, after: &HashSet<String>, end: i64) {
    let now = now_ms();
    let added: Vec<(String, i64)> = after.difference(before).map(|k| (k.clone(), end)).collect();
    touch_reveals(keys::SECTIONS, &added, now);
    for k in before.difference(after) {
        remove_reveal(keys::SECTIONS, k);
    }
}

/// Viewing a bracket counts as interaction: on mount, refresh every revealed
/// section key belonging to this bracket (its `np`/`sp` name/score prefixes) so it
/// stays revealed while you still look at it.
#[cfg(feature = "hydrate")]
fn touch_bracket_on_view(
    sections: Option<RwSignal<HashSet<String>>>,
    np: String,
    sp: String,
    end: i64,
) {
    Effect::new(move |_| {
        if let Some(sec) = sections {
            let items: Vec<(String, i64)> = sec.with_untracked(|set| {
                set.iter()
                    .filter(|k| k.starts_with(&np) || k.starts_with(&sp))
                    .map(|k| (k.clone(), end))
                    .collect()
            });
            touch_reveals(keys::SECTIONS, &items, now_ms());
        }
    });
}

/// Reveal stage of a bracket series from its precomputed name/score keys:
/// 0 hidden, 1 team names, 2 scores. (`bs` set ⇒ scores; `bn` ⇒ names.)
pub(crate) fn stage_of(set: &HashSet<String>, bn: &str, bs: &str) -> u8 {
    if set.contains(bs) {
        2
    } else if set.contains(bn) {
        1
    } else {
        0
    }
}

/// Move one series to a reveal stage (0 hidden / 1 names / 2 scores).
pub(crate) fn apply_stage(set: &mut HashSet<String>, bn: &str, bs: &str, stage: u8) {
    match stage {
        0 => {
            set.remove(bn);
            set.remove(bs);
        }
        1 => {
            set.insert(bn.to_string());
            set.remove(bs);
        }
        _ => {
            set.insert(bn.to_string());
            set.insert(bs.to_string());
        }
    }
}

/// Whether a bracket match has actually been played: it has both scores and
/// either a decided winner or a non-zero score. A 0–0 with no winner is an
/// unplayed/upcoming match, not a real result, so it never gets a score stage.
fn bm_played(m: &BracketMatch) -> bool {
    m.score_a.is_some()
        && m.score_b.is_some()
        && (!m.winner.is_empty() || m.score_a != Some(0) || m.score_b != Some(0))
}

/// The furthest a series can be revealed: 0 if its teams are undecided (TBD —
/// stays hidden so we don't expose an unannounced match), 2 if it's been played
/// (has a score to show), else 1 (decided teams, not yet played → names only).
pub(crate) fn bm_max_stage(m: &BracketMatch) -> u8 {
    let tbd = |t: &str| t.is_empty() || t == "TBD";
    if tbd(&m.team_a) || tbd(&m.team_b) {
        0
    } else if bm_played(m) {
        2
    } else {
        1
    }
}

/// Swiss-match equivalents of [`bm_played`] / [`bm_max_stage`], so a Swiss stage
/// reveals through the same staged feeder-gated machinery as the playoff.
fn sw_played(m: &SwissMatch) -> bool {
    !m.winner.is_empty()
        || (m.score_a.is_some()
            && m.score_b.is_some()
            && (m.score_a != Some(0) || m.score_b != Some(0)))
}

/// A finishing record like "3-0"/"0-3": advanced if it has more wins than
/// losses, eliminated otherwise.
fn record_is_pass(rec: &str) -> bool {
    rec.split_once('-')
        .and_then(|(w, l)| Some(w.trim().parse::<i32>().ok()? > l.trim().parse::<i32>().ok()?))
        .unwrap_or(false)
}

fn sw_max_stage(m: &SwissMatch) -> u8 {
    let tbd = |t: &str| t.is_empty() || t == "TBD";
    if tbd(&m.team_a) || tbd(&m.team_b) {
        0
    } else if sw_played(m) {
        2
    } else {
        1
    }
}

/// Per-series reveal metadata used to compute effective stages and gate clicks.
#[derive(Clone)]
pub(crate) struct BkCell {
    pub(crate) bn: String,
    pub(crate) bs: String,
    /// Furthest this series can be revealed (see [`bm_max_stage`]).
    pub(crate) max: u8,
    /// Minimum stage always shown (1 = team names) — used so a bracket-only
    /// event's first-round matchups stay visible (only scores are a spoiler).
    pub(crate) floor: u8,
    /// `(round, index)` of the matches whose scores must be shown before this
    /// series' lineup is known (empty for a first-round match).
    pub(crate) feeders: Vec<(usize, usize)>,
}

/// A series' lineup is auto-revealed (stage 1) once all its feeders' scores are
/// shown — works for single- and double-elimination. First-round matches
/// (no feeders) never auto-reveal; they're revealed manually.
fn auto_stage(eff: &[Vec<u8>], feeders: &[(usize, usize)]) -> u8 {
    if feeders.is_empty() {
        return 0;
    }
    let all_scored = feeders
        .iter()
        .all(|&(r, i)| eff.get(r).and_then(|row| row.get(i)).copied().unwrap_or(0) >= 2);
    u8::from(all_scored)
}

/// Effective reveal stage per series: the manual reveal (from the shared set)
/// unioned with the auto-revealed lineup, capped at each series' max. Computed
/// round by round so feeders (always in earlier rounds) are ready. `global`
/// reveals everything (still capped, so TBD matches stay hidden).
pub(crate) fn compute_effective(
    grid: &[Vec<BkCell>],
    set: &HashSet<String>,
    global: bool,
) -> Vec<Vec<u8>> {
    let mut eff: Vec<Vec<u8>> = Vec::with_capacity(grid.len());
    for row in grid {
        let stages: Vec<u8> = row
            .iter()
            .map(|c| {
                if global {
                    return c.max;
                }
                let raw = stage_of(set, &c.bn, &c.bs)
                    .max(auto_stage(&eff, &c.feeders))
                    .max(c.floor)
                    .min(c.max);
                // Cascade hiding: a series can't show its score until its own
                // teams are known (every feeder's score is shown). So hiding an
                // early round pulls its lineup, which pulls every later round it
                // feeds — not just the immediately next one.
                let teams_known = c.feeders.iter().all(|&(r, i)| {
                    eff.get(r).and_then(|row| row.get(i)).copied().unwrap_or(0) >= 2
                });
                if teams_known {
                    raw
                } else {
                    raw.min(1)
                }
            })
            .collect();
        eff.push(stages);
    }
    eff
}

/// Whether a series can still be advanced: decided (max > 0), not already maxed,
/// and — for later rounds — its feeders' scores revealed (lineup known).
fn bk_advanceable(grid: &[Vec<BkCell>], eff: &[Vec<u8>], r: usize, i: usize) -> bool {
    let c = &grid[r][i];
    c.max > 0 && eff[r][i] < c.max && (c.feeders.is_empty() || auto_stage(eff, &c.feeders) == 1)
}

/// A bracket reveal action, applied against the shared section set.
#[derive(Clone, Copy)]
pub(crate) enum BkOp {
    /// Cycle one series (the granular click).
    Series(usize, usize),
    /// Advance a whole round one stage, or reset it once fully revealed.
    Round(usize),
    /// Advance the earliest revealable round; reset all once everything is shown.
    Cascade,
}

/// Reveal a series' contributing matches (recursively) up to their scores, so the
/// series' own lineup becomes known. This is the "team reveal" step for a series
/// whose participants aren't decided yet.
fn reveal_feeders(set: &mut HashSet<String>, grid: &[Vec<BkCell>], r: usize, i: usize) {
    for &(fr, fi) in &grid[r][i].feeders {
        reveal_feeders(set, grid, fr, fi);
        let fc = &grid[fr][fi];
        apply_stage(set, &fc.bn, &fc.bs, fc.max);
    }
}

/// Whether a series' teams are known: it's a first-round match, or all its
/// feeders' scores are revealed.
fn bk_teams_known(grid: &[Vec<BkCell>], eff: &[Vec<u8>], r: usize, i: usize) -> bool {
    let c = &grid[r][i];
    c.feeders.is_empty() || auto_stage(eff, &c.feeders) == 1
}

/// Reveal a whole round to one uniform stage — the least-revealed decided match's
/// stage plus one. Bringing every series to the same stage pulls any matches that
/// were individually revealed earlier into the even progression, so the outer
/// "round"/"Bracket" controls don't preserve those granular reveals out of order
/// ("revealing is granular, hiding is bigger-picture").
fn reveal_round_uniform(
    set: &mut HashSet<String>,
    grid: &[Vec<BkCell>],
    eff: &[Vec<u8>],
    r: usize,
) {
    let min_stage = (0..grid[r].len())
        .filter(|&i| grid[r][i].max > 0)
        .map(|i| eff[r][i])
        .min()
        .unwrap_or(0);
    let target = (min_stage + 1).min(2);
    for i in 0..grid[r].len() {
        let c = &grid[r][i];
        if c.max == 0 {
            continue;
        }
        // A score can't show before the lineup is known.
        let cap = if bk_teams_known(grid, eff, r, i) {
            c.max
        } else {
            c.max.min(1)
        };
        let t = target.min(cap);
        // If the auto-revealed lineup already provides this stage, clear the manual
        // entry and let auto provide it (and so hiding a feeder still cascades).
        let auto = auto_stage(eff, &c.feeders);
        apply_stage(set, &c.bn, &c.bs, if t <= auto { 0 } else { t });
    }
}

/// Drop every manual reveal in the rounds after `r`, so an outer control (a round
/// toggle or the cascade) is authoritative over everything past it — cycling it
/// never leaves a later round revealed from earlier clicks. Auto-revealed lineups
/// aren't in the set, so they're untouched.
fn hide_rounds_after(set: &mut HashSet<String>, grid: &[Vec<BkCell>], r: usize) {
    for later in &grid[r + 1..] {
        for c in later {
            apply_stage(set, &c.bn, &c.bs, 0);
        }
    }
}

/// Apply a bracket reveal action to the shared set, respecting feeder gating so a
/// later round's lineup can't be revealed before its feeders' scores are shown.
pub(crate) fn apply_bracket_op(set: &mut HashSet<String>, grid: &[Vec<BkCell>], op: BkOp) {
    let eff = compute_effective(grid, set, false);
    let advance = |set: &mut HashSet<String>, r: usize, i: usize| {
        let c = &grid[r][i];
        let auto = auto_stage(&eff, &c.feeders);
        let next = (eff[r][i] + 1).min(c.max);
        // Falling back to the auto stage means "let auto provide it" → clear manual.
        let target = if next <= auto { 0 } else { next };
        apply_stage(set, &c.bn, &c.bs, target);
    };
    match op {
        BkOp::Series(r, i) => {
            let c = &grid[r][i];
            if c.max == 0 {
                return; // TBD: never revealable
            }
            if !bk_teams_known(grid, &eff, r, i) {
                // First step: reveal the contributing matches' scores; the lineup
                // then appears on its own (and you can't peek ahead unfairly).
                reveal_feeders(set, grid, r, i);
            } else if eff[r][i] >= c.max {
                // Fully shown → step back (auto keeps the names for a fed series).
                let auto = auto_stage(&eff, &c.feeders);
                apply_stage(set, &c.bn, &c.bs, auto);
            } else {
                advance(set, r, i);
            }
        }
        BkOp::Round(r) => {
            let n = grid[r].len();
            // If any series' teams aren't decided yet, the round's first step is to
            // reveal the contributing matches that feed it.
            let need_feeders: Vec<usize> = (0..n)
                .filter(|&i| grid[r][i].max > 0 && !bk_teams_known(grid, &eff, r, i))
                .collect();
            if !need_feeders.is_empty() {
                for i in need_feeders {
                    reveal_feeders(set, grid, r, i);
                }
            } else if (0..n).any(|i| bk_advanceable(grid, &eff, r, i)) {
                reveal_round_uniform(set, grid, &eff, r);
            } else {
                // Fully revealed → hide the whole round.
                for c in &grid[r] {
                    apply_stage(set, &c.bn, &c.bs, 0);
                }
            }
            // The round toggle owns everything after it: cycling it shouldn't keep
            // later rounds revealed from earlier granular clicks.
            hide_rounds_after(set, grid, r);
        }
        BkOp::Cascade => {
            let frontier = (0..grid.len())
                .find(|&r| (0..grid[r].len()).any(|i| bk_advanceable(grid, &eff, r, i)));
            match frontier {
                Some(r) => {
                    reveal_round_uniform(set, grid, &eff, r);
                    // Reveal strictly front-to-back: forget any granular reveals
                    // further along ("hiding is bigger-picture").
                    hide_rounds_after(set, grid, r);
                }
                None => {
                    for row in grid {
                        for c in row {
                            apply_stage(set, &c.bn, &c.bs, 0);
                        }
                    }
                }
            }
        }
    }
}

#[component]
pub(crate) fn StandingsTable(
    rows: Vec<StandingRow>,
    sport: Sport,
    /// Reveal key for a self-managed table (the Swiss "list" view). Unused when
    /// `shared` is set.
    #[prop(optional)]
    tournament_id: i64,
    /// When set, follow this shared reveal — the grouped-standings block owns the
    /// one "Standings" toggle and supplies each table's group/division heading, so
    /// the table renders headerless.
    #[prop(optional)]
    shared: Option<Memo<bool>>,
) -> impl IntoView {
    if rows.is_empty() {
        return ().into_any();
    }
    // The last column: CS2 counts maps, LoL games, the team sports a single
    // ranking value (games-back / points / win%).
    let record_label = sport.standings_last_label();
    let show_last = sport.standings_single_value();
    // Follow the block's shared reveal when grouped; else self-manage an own
    // "Standings" toggle (click the title to reveal/hide the table).
    let (revealed, header) = match shared {
        Some(m) => (m, None),
        None => {
            let (r, toggle) = section_reveal(format!("st:{tournament_id}"));
            let head = view! {
                <div class="section-head">
                    <button
                        class="section-title section-toggle"
                        class:on=move || r.get()
                        title=move || if r.get() { "Hide the standings" } else { "Show the standings" }
                        aria-expanded=move || if r.get() { "true" } else { "false" }
                        on:click=toggle
                    >
                        "Standings"
                    </button>
                    {move || (!r.get()).then(|| view! { <span class="section-hint">"hidden"</span> })}
                </div>
            };
            (r, Some(head))
        }
    };
    // Record alignment: if any team has ties/OT losses, every row shows the third
    // field (blank where a team has none) so the W, L and T columns line up; each
    // number is padded to the table's widest so double digits align too. Rendered
    // in the monospace body font with `white-space: pre`, so the padding holds.
    let show_ties = rows.iter().any(|r| r.ties > 0);
    let num_w = rows
        .iter()
        .flat_map(|r| [r.wins, r.losses, r.ties])
        .map(|n| n.to_string().len())
        .max()
        .unwrap_or(1)
        .max(1);
    // Always render every row so the table reserves its height — when hidden, the
    // team/record cells show blank placeholders so revealing doesn't shift the page.
    let body = move || {
        let show = revealed.get();
        rows.clone()
            .into_iter()
            .map(|r| {
                let (team, wl, maps) = if show {
                    let last = if show_last {
                        r.gb
                    } else {
                        format!("{}-{}", r.game_wins, r.game_losses)
                    };
                    // Zero-pad each present number to the table's widest so the W/L
                    // (and OTL/tie) columns line up even with double digits — a
                    // leading zero, never a space, so there's no gap after a dash
                    // ("43-33-06", not "43-33- 6").
                    let zpad = |v: String| {
                        let gap = num_w.saturating_sub(v.chars().count());
                        format!("{}{v}", "0".repeat(gap))
                    };
                    let w = zpad(r.wins.to_string());
                    let l = zpad(r.losses.to_string());
                    // NHL carries OT losses as the third record number (W-L-OTL);
                    // soccer, draws. A team with none gets the third field blanked
                    // (no dash, no digits) to keep the columns aligned — with
                    // non-breaking spaces, since a plain space would be
                    // collapsed/trimmed in the DOM and desync hydration on reveal.
                    let wl = if show_ties {
                        let t = if r.ties > 0 {
                            format!("-{}", zpad(r.ties.to_string()))
                        } else {
                            "\u{00A0}".repeat(num_w + 1)
                        };
                        format!("{w}-{l}{t}")
                    } else {
                        format!("{w}-{l}")
                    };
                    (r.team, wl, last)
                } else {
                    ("—".to_string(), "—".to_string(), "—".to_string())
                };
                // The full name rides along in `data-team` for the hover popover.
                let team_full = team.clone();
                view! {
                    <tr>
                        <td class="st-rank">{r.rank}</td>
                        <td class="st-team" data-team=team_full>
                            <span class="st-team-name">{team}</span>
                        </td>
                        <td class="st-wl">{wl}</td>
                        <td class="st-diff">{maps}</td>
                    </tr>
                }
            })
            .collect_view()
    };
    view! {
        <section class="detail-section">
            {header}
            <table class="standings" class:placeholder=move || !revealed.get()>
                <thead>
                    <tr>
                        <th></th>
                        <th class="st-team">"Team"</th>
                        <th class="st-wl">"W-L"</th>
                        <th class="st-diff">{record_label}</th>
                    </tr>
                </thead>
                <tbody>{body}</tbody>
            </table>
        </section>
    }
    .into_any()
}

/// Render data for one bracket series (parallel to its [`BkCell`] in the grid).
struct BkRender {
    i: usize,
    ta: String,
    tb: String,
    sa: Option<i64>,
    sb: Option<i64>,
    winner: String,
    max: u8,
    mid: i64,
}

/// The day number from a `YYYY-MM-DD` key (leading zero stripped by parsing).
fn day_num(key: &str) -> u32 {
    key.rsplit('-')
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// Format a `YYYY-MM-DD` day key as a short date, e.g. "Jun 18".
fn fmt_day_short(key: &str) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let month: usize = key
        .split('-')
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let Some(mon) = MONTHS.get(month.wrapping_sub(1)) else {
        return String::new();
    };
    format!("{mon} {}", day_num(key))
}

/// Format a round's date span from its sorted-unique day keys: a single day
/// ("Jun 18"), a same-month range ("Jun 18–19"), or a cross-month range
/// ("Jun 30 – Jul 1"). Empty when no dates are known.
fn fmt_date_range(keys: &[String]) -> String {
    let (Some(first), Some(last)) = (keys.first(), keys.last()) else {
        return String::new();
    };
    if first == last {
        return fmt_day_short(first);
    }
    if first.get(..7).is_some() && first.get(..7) == last.get(..7) {
        format!("{}–{}", fmt_day_short(first), day_num(last))
    } else {
        format!("{} – {}", fmt_day_short(first), fmt_day_short(last))
    }
}

/// One Swiss match's render data, paired with its `(round, position)` grid slot
/// and the resolved grid coordinates of its two feeders (each side's prior
/// match), so it reveals through the same staged, feeder-gated machinery as the
/// playoff bracket.
struct SwRender {
    r: usize,
    i: usize,
    team_a: String,
    team_b: String,
    sa: Option<i64>,
    sb: Option<i64>,
    winner: String,
    mid: i64,
    a_record: String,
    b_record: String,
    f0: Option<(usize, usize)>,
    f1: Option<(usize, usize)>,
    max: u8,
}

/// One Swiss record bucket for render: its entering record label and the matches
/// in it. A round is a `Vec` of these; the whole grid a `Vec` of rounds.
type SwBucketRender = (String, Vec<SwRender>);
/// One Swiss round for render: its heading and its record buckets.
type SwRoundRender = (String, Vec<SwBucketRender>);
/// A reachable outcome from a bucket: its record, whether it advances, and the
/// `(round, match)` positions that produce it (for spoiler-gating).
type SwOutcome = (String, bool, Vec<(usize, usize)>);

/// A Swiss / "first to N" stage drawn as a buchholz grid: a column per round,
/// each round's matches grouped into record buckets (2-0, 1-1, 0-2, …). Styled
/// and revealed exactly like the playoff bracket — winner bold, loser muted,
/// finishers badged with their final record (3-0 … 0-3). Progressive reveal is
/// feeder-gated: a side's name only shows once its previous-round match's score
/// is revealed, so you reveal round by round and can't peek ahead. A hidden but
/// played match shows a dot (there's a result to reveal); an unplayed one a
/// dash. Clicking a name scrolls to that match's row in the schedule.
#[component]
pub(crate) fn SwissBracket(
    rounds: Vec<SwissRound>,
    tournament_id: i64,
    sport: Sport,
) -> impl IntoView {
    if rounds.is_empty() {
        return ().into_any();
    }
    let global = use_context::<ShowScores>().map(|s| s.0);
    let flash = use_context::<FlashScores>().map(|f| f.0);
    let sections = use_context::<RevealedSections>().map(|s| s.0);
    let tid = tournament_id;
    // The team whose path is being traced (hovered), keyed by name.
    let hovered = RwSignal::new(String::new());

    // Map every match to its (round, position) slot so feeders resolve to grid
    // coordinates (position runs across a round's buckets, in display order).
    let mut pos: std::collections::HashMap<i64, (usize, usize)> = std::collections::HashMap::new();
    for (r, round) in rounds.iter().enumerate() {
        let mut i = 0usize;
        for bucket in &round.buckets {
            for m in &bucket.matches {
                if m.match_id > 0 {
                    pos.insert(m.match_id, (r, i));
                }
                i += 1;
            }
        }
    }

    // Build the reveal grid (one row per round, matches in column order) and the
    // parallel render tree (kept grouped into record buckets for display).
    let mut grid: Vec<Vec<BkCell>> = Vec::with_capacity(rounds.len());
    let mut render: Vec<SwRoundRender> = Vec::with_capacity(rounds.len());
    for (r, round) in rounds.into_iter().enumerate() {
        let head = format!("Round {}", round.number);
        let mut cells: Vec<BkCell> = Vec::new();
        let mut buckets_r: Vec<(String, Vec<SwRender>)> = Vec::new();
        let mut i = 0usize;
        for bucket in round.buckets {
            let mut ms: Vec<SwRender> = Vec::new();
            for m in bucket.matches {
                let max = sw_max_stage(&m);
                let f0 = m.a_feeder.and_then(|f| pos.get(&f).copied());
                let f1 = m.b_feeder.and_then(|f| pos.get(&f).copied());
                // Both feeders or neither (round 1) — keep slot order [a, b].
                let feeders = match (f0, f1) {
                    (Some(a), Some(b)) => vec![a, b],
                    _ => Vec::new(),
                };
                cells.push(BkCell {
                    bn: format!("swn:{tid}:{r}:{i}"),
                    bs: format!("sws:{tid}:{r}:{i}"),
                    max,
                    floor: 0,
                    feeders,
                });
                ms.push(SwRender {
                    r,
                    i,
                    team_a: m.team_a,
                    team_b: m.team_b,
                    sa: m.score_a,
                    sb: m.score_b,
                    winner: m.winner,
                    mid: m.match_id,
                    a_record: m.a_record,
                    b_record: m.b_record,
                    f0,
                    f1,
                    max,
                });
                i += 1;
            }
            buckets_r.push((bucket.record, ms));
        }
        grid.push(cells);
        render.push((head, buckets_r));
    }

    // Effective stages for the whole stage, recomputed when the shared reveal set
    // or the global toggle changes (same machinery as the playoff bracket).
    let grid_sv = StoredValue::new(grid.clone());
    let eff = Memo::new(move |_| {
        let g = global.is_some_and(|s| s.get());
        let set = sections.map(|s| s.get()).unwrap_or_default();
        compute_effective(&grid, &set, g)
    });
    // The reveal "end" for this bracket's keys (the event's last match).
    let end = current_reveal_end();
    #[cfg(not(feature = "hydrate"))]
    let _ = end;
    #[cfg(feature = "hydrate")]
    touch_bracket_on_view(sections, format!("swn:{tid}:"), format!("sws:{tid}:"), end);
    let do_op = move |op: BkOp| {
        // A hide can't take effect while the global reveal is on — flash the
        // "scores" button instead of silently doing nothing.
        if hide_deflected_to_global(global, flash) {
            return;
        }
        if let Some(sec) = sections {
            #[cfg(feature = "hydrate")]
            let before = sec.get_untracked();
            sec.update(|set| apply_bracket_op(set, &grid_sv.get_value(), op));
            #[cfg(feature = "hydrate")]
            sync_section_diff(&before, &sec.get_untracked(), end);
        }
    };

    // Render one match box: feeder-gated per-slot reveal, dot/dash for a hidden
    // result, hover-trace, and a name that scrolls to the schedule row.
    let mk_match = move |m: SwRender| -> leptos::prelude::AnyView {
        let SwRender {
            r,
            i,
            team_a,
            team_b,
            sa,
            sb,
            winner,
            mid,
            a_record,
            b_record,
            f0,
            f1,
            max,
        } = m;
        let stage = move || eff.with(|e| e.get(r).and_then(|row| row.get(i)).copied().unwrap_or(0));
        let fed_shown = move |f: Option<(usize, usize)>| {
            f.is_some_and(|(fr, fi)| {
                eff.with(|e| e.get(fr).and_then(|row| row.get(fi)).copied().unwrap_or(0) >= 2)
            })
        };
        let team_link = move |name: String| -> leptos::prelude::AnyView {
            if mid <= 0 {
                return view! { <span class="sw-team">{name}</span> }.into_any();
            }
            let muid = crate::types::match_uid(sport, mid);
            view! {
                <a
                    class="sw-team sw-link"
                    href=crate::types::match_path(sport, mid)
                    title="Find this match in the schedule"
                    on:click=move |e: leptos::ev::MouseEvent| {
                        e.stop_propagation();
                        #[cfg(feature = "hydrate")]
                        if flash_match_row(&muid) {
                            e.prevent_default();
                        }
                        #[cfg(not(feature = "hydrate"))]
                        let _ = &muid;
                    }
                >
                    {name}
                </a>
            }
            .into_any()
        };
        let body = move || {
            let st = stage();
            // A fed slot is known once its feeder's score is shown; a round-1 slot
            // once this match itself is revealed. Scores need both sides known.
            let known_a = if f0.is_some() { fed_shown(f0) } else { st >= 1 };
            let known_b = if f1.is_some() { fed_shown(f1) } else { st >= 1 };
            let scores = st >= 2 && known_a && known_b;
            let score_view = move |s: Option<i64>| -> leptos::prelude::AnyView {
                if scores {
                    view! {
                        <span class="sw-score">{s.map(|v| v.to_string()).unwrap_or_default()}</span>
                    }
                    .into_any()
                } else if max >= 2 {
                    view! {
                        <span class="sw-score sw-pending" title="Result hidden — click to reveal">
                            "●"
                        </span>
                    }
                    .into_any()
                } else {
                    view! {
                        <span class="sw-score sw-unplayed" title="Not played yet">"-"</span>
                    }
                    .into_any()
                }
            };
            let row = move |known: bool,
                            win: bool,
                            lose: bool,
                            name: String,
                            score: Option<i64>,
                            rec: String|
                  -> leptos::prelude::AnyView {
                if !known {
                    return view! {
                        <div class="sw-row sw-hidden">
                            <span class="sw-team">"—"</span>
                        </div>
                    }
                    .into_any();
                }
                // A team that finished the stage (has a record) colours its name
                // green if it advanced (more wins) or red if it was knocked out;
                // the exact record sits above the bucket. Otherwise the usual
                // bold-winner / muted-loser styling.
                let cls = if scores && !rec.is_empty() {
                    if record_is_pass(&rec) {
                        "sw-row pass"
                    } else {
                        "sw-row exit"
                    }
                } else if win {
                    "sw-row win"
                } else if lose {
                    "sw-row lose"
                } else {
                    "sw-row"
                };
                // Hovering a team lights its row in every round (and dims the
                // rest), so you can trace its run. `mouseenter` is per-row but
                // `mouseleave` is on the box (below), so moving between the two
                // rows never clears the trace — no flash across the row gap.
                let (n_class, n_enter) = (name.clone(), name.clone());
                view! {
                    <div
                        class=cls
                        class:sw-path=move || hovered.with(|h| !h.is_empty() && *h == n_class)
                        on:mouseenter=move |_| hovered.set(n_enter.clone())
                    >
                        {team_link(name)}
                        {score_view(score)}
                    </div>
                }
                .into_any()
            };
            view! {
                {row(known_a, scores && winner == "a", scores && winner == "b", team_a.clone(), sa, a_record.clone())}
                {row(known_b, scores && winner == "b", scores && winner == "a", team_b.clone(), sb, b_record.clone())}
            }
            .into_any()
        };
        let title = if max == 0 {
            "Not decided yet"
        } else {
            "Click to reveal this match"
        };
        view! {
            <div
                class="sw-match"
                class:sw-locked=max == 0
                role="button"
                tabindex=if max == 0 { "-1" } else { "0" }
                title=title
                aria-label=title
                on:click=move |_| do_op(BkOp::Series(r, i))
                on:keydown=move |e: leptos::ev::KeyboardEvent| {
                    #[cfg(feature = "hydrate")]
                    if matches!(e.key().as_str(), "Enter" | " ") {
                        e.prevent_default();
                        do_op(BkOp::Series(r, i));
                    }
                    #[cfg(not(feature = "hydrate"))]
                    let _ = &e;
                }
                on:mouseleave=move |_| hovered.set(String::new())
            >
                {body}
            </div>
        }
        .into_any()
    };

    let cols = render
        .into_iter()
        .enumerate()
        .map(move |(r, (head, buckets_r))| {
            let buckets = buckets_r
                .into_iter()
                .map(move |(record, ms)| {
                    // The records teams reach from this bucket (3-0 advance, 0-3
                    // out, …), deduped, each with the match positions that produce
                    // it — shown above the group, gated on those matches being
                    // revealed so it isn't a spoiler.
                    let mut outcomes: Vec<SwOutcome> = Vec::new();
                    for m in &ms {
                        for rec in [&m.a_record, &m.b_record] {
                            if rec.is_empty() {
                                continue;
                            }
                            match outcomes.iter_mut().find(|(r, _, _)| r == rec) {
                                Some(e) => e.2.push((m.r, m.i)),
                                None => outcomes.push((
                                    rec.clone(),
                                    record_is_pass(rec),
                                    vec![(m.r, m.i)],
                                )),
                            }
                        }
                    }
                    let outcome_view = (!outcomes.is_empty()).then_some(move || {
                        outcomes
                            .iter()
                            .filter(|(_, _, pos)| {
                                pos.iter().any(|&(rr, ii)| {
                                    eff.with(|e| {
                                        e.get(rr).and_then(|row| row.get(ii)).copied().unwrap_or(0)
                                            >= 2
                                    })
                                })
                            })
                            .map(|(rec, pass, _)| {
                                let cls = if *pass {
                                    "sw-out sw-pass"
                                } else {
                                    "sw-out sw-exit"
                                };
                                view! { <span class=cls>{rec.clone()}</span> }
                            })
                            .collect_view()
                    });
                    let matches = ms.into_iter().map(mk_match).collect_view();
                    view! {
                        <div class="sw-bucket">
                            <div class="sw-record">
                                <span class="sw-record-in">{record}</span>
                                <span class="sw-outs">{outcome_view}</span>
                            </div>
                            {matches}
                        </div>
                    }
                })
                .collect_view();
            // The round header reveals/hides this whole round (names → scores),
            // like the playoff bracket's round titles.
            let round_on =
                move || eff.with(|e| e.get(r).is_some_and(|row| row.iter().any(|&s| s >= 1)));
            view! {
                <div class="sw-col">
                    <button
                        class="sw-col-head sw-round-toggle"
                        class:on=round_on
                        title="Reveal this round"
                        aria-expanded=move || if round_on() { "true" } else { "false" }
                        on:click=move |_| do_op(BkOp::Round(r))
                    >
                        {head}
                    </button>
                    {buckets}
                </div>
            }
        })
        .collect_view();

    // The "Bracket" title cascades the whole stage: each click reveals the
    // earliest revealable round's scores, unlocking the next round's lineup.
    let bracket_on = move || eff.with(|e| e.iter().flatten().any(|&s| s >= 1));
    view! {
        <section class="detail-section swiss-section">
            <div class="section-head">
                <button
                    class="section-title section-toggle"
                    class:on=bracket_on
                    title="Reveal the bracket, round by round"
                    aria-expanded=move || if bracket_on() { "true" } else { "false" }
                    on:click=move |_| do_op(BkOp::Cascade)
                >
                    "Bracket"
                </button>
                {move || (!bracket_on()).then(|| view! { <span class="section-hint">"hidden"</span> })}
            </div>
            <div class="swiss-scroll">
                <div class="swiss" class:sw-tracing=move || hovered.with(|h| !h.is_empty())>
                    {cols}
                </div>
            </div>
        </section>
    }
    .into_any()
}

#[component]
pub(crate) fn Bracket(
    rounds: Vec<BracketRound>,
    tournament_id: i64,
    /// The event's sport, so a match link can build the right `/match/{uid}`.
    sport: Sport,
    bracket_only: bool,
    /// `match_id -> (day_key, clock_label)` for the event's matches, so the
    /// bracket can date each round and show a per-match time on hover. Empty
    /// where the schedule isn't on the page (then no times are shown).
    #[prop(optional)]
    times: std::collections::HashMap<i64, (String, String)>,
) -> impl IntoView {
    if rounds.is_empty() {
        return ().into_any();
    }
    let times = StoredValue::new(times);
    let global = use_context::<ShowScores>().map(|s| s.0);
    let flash = use_context::<FlashScores>().map(|f| f.0);
    let sections = use_context::<RevealedSections>().map(|s| s.0);
    let tid = tournament_id;

    // Position every match from the feeder graph; the render places boxes
    // absolutely at these slots and draws an SVG connector per edge. Slot units →
    // em: a round title (name over date) sits above each section's first box,
    // with a section banner above that for double-elim. `head_em` is the box
    // centre offset that reserves room for all of it above slot 0.
    let layout = bracket::layout(&rounds);
    // Size the boxes (and so the column pitch) to this bracket's longest team name,
    // uniform across the bracket. `box_w_em` also feeds the `--bk-box-w` custom
    // property so the CSS box matches the geometry the connectors are drawn to.
    let box_w_em = bracket::box_width_em(&rounds, layout.cols);
    let col_em = bracket::col_em(box_w_em);
    const TITLE_EM: f64 = 2.8; // round name + date (two lines)
    const TITLE_GAP: f64 = 0.5; // below the date, above the box
    const LABEL_EM: f64 = 1.5; // section banner
    const LABEL_GAP: f64 = 1.2; // below the banner, above the title
    const TOP_PAD: f64 = 0.3; // above the topmost banner/title
    let half_h = bracket::BOX_H_EM / 2.0;
    let multi = layout.group_rows.len() > 1
        || layout
            .group_rows
            .first()
            .is_some_and(|(s, _, _)| !s.is_empty());
    let head_em =
        TOP_PAD + half_h + TITLE_GAP + TITLE_EM + if multi { LABEL_GAP + LABEL_EM } else { 0.0 };
    let center_em = move |y: f64| y * bracket::ROW_EM + head_em;

    // Build the reveal grid (keys/max/feeders, for stage computation + clicks)
    // and the parallel render data, once. `winners` holds each match's advancing
    // team so a fed match can show a single feeder's winner before its other
    // feeder is revealed (and before PandaScore has seeded that slot).
    let mut grid: Vec<Vec<BkCell>> = Vec::with_capacity(rounds.len());
    let mut render: Vec<(String, String, Vec<BkRender>)> = Vec::with_capacity(rounds.len());
    let mut winners: Vec<Vec<String>> = Vec::with_capacity(rounds.len());
    for (r, round) in rounds.into_iter().enumerate() {
        let mut cells = Vec::with_capacity(round.matches.len());
        let mut sres = Vec::with_capacity(round.matches.len());
        let mut wins = Vec::with_capacity(round.matches.len());
        for (i, m) in round.matches.into_iter().enumerate() {
            let max = bm_max_stage(&m);
            // For a bracket-only event, keep first-round matchups (no feeders)
            // visible by default — only their scores are a spoiler.
            let floor = u8::from(bracket_only && m.feeders.is_empty() && max >= 1);
            wins.push(match m.winner.as_str() {
                "a" => m.team_a.clone(),
                "b" => m.team_b.clone(),
                _ => String::new(),
            });
            cells.push(BkCell {
                bn: format!("bn:{tid}:{r}:{i}"),
                bs: format!("bs:{tid}:{r}:{i}"),
                max,
                floor,
                feeders: m.feeders,
            });
            sres.push(BkRender {
                i,
                ta: m.team_a,
                tb: m.team_b,
                sa: m.score_a,
                sb: m.score_b,
                winner: m.winner,
                max,
                mid: m.match_id,
            });
        }
        grid.push(cells);
        render.push((round.section, round.title, sres));
        winners.push(wins);
    }
    let winners_sv = StoredValue::new(winners);

    // Effective stages for the whole bracket, recomputed when the shared reveal
    // set or the global toggle changes.
    let grid_sv = StoredValue::new(grid.clone());
    let eff = Memo::new(move |_| {
        let g = global.is_some_and(|s| s.get());
        let set = sections.map(|s| s.get()).unwrap_or_default();
        compute_effective(&grid, &set, g)
    });
    // The reveal "end" for this bracket's keys (the event's last match).
    let end = current_reveal_end();
    #[cfg(not(feature = "hydrate"))]
    let _ = end;
    #[cfg(feature = "hydrate")]
    touch_bracket_on_view(sections, format!("bn:{tid}:"), format!("bs:{tid}:"), end);
    // A single Copy handle for every reveal click (series / round / cascade).
    let do_op = move |op: BkOp| {
        // A hide can't take effect while the global reveal is on — flash the
        // "scores" button instead of silently doing nothing.
        if hide_deflected_to_global(global, flash) {
            return;
        }
        if let Some(sec) = sections {
            #[cfg(feature = "hydrate")]
            let before = sec.get_untracked();
            sec.update(|set| apply_bracket_op(set, &grid_sv.get_value(), op));
            #[cfg(feature = "hydrate")]
            sync_section_diff(&before, &sec.get_untracked(), end);
        }
    };

    let positions = &layout.positions;
    let group_rows = &layout.group_rows;
    // Each round (column) contributes a positioned title header plus its boxes,
    // all absolutely placed in one canvas (no flex columns).
    let nodes = render
        .into_iter()
        .enumerate()
        .map(|(r, (section, title, sres))| {
            let x_em = positions[r].first().map_or(0, |p| p.col) as f64 * col_em;
            let sec_ymin = group_rows
                .iter()
                .find(|(s, _, _)| s == &section)
                .map_or(0.0, |(_, lo, _)| *lo);
            let title_top = center_em(sec_ymin) - half_h - TITLE_GAP - TITLE_EM;
            let round_on = move || eff.with(|e| e.get(r).is_some_and(|row| row.iter().any(|&s| s >= 1)));
            // The round's date span, from its matches' schedule days.
            let round_date = {
                let mut keys: Vec<String> = sres
                    .iter()
                    .filter_map(|s| times.with_value(|t| t.get(&s.mid).map(|(d, _)| d.clone())))
                    .collect();
                keys.sort();
                keys.dedup();
                fmt_date_range(&keys)
            };
            let header = view! {
                <div class="bk-round-title" style=format!("left:{x_em}em;top:{title_top}em")>
                    <button
                        class="bk-round-toggle"
                        class:on=round_on
                        title="Reveal this round"
                        aria-expanded=move || if round_on() { "true" } else { "false" }
                        on:click=move |_| do_op(BkOp::Round(r))
                    >
                        {title}
                    </button>
                    {(!round_date.is_empty())
                        .then(|| view! { <span class="bk-round-date">{round_date}</span> })}
                </div>
            };
            let ms = sres
                .into_iter()
                .map(|s| {
                    let (i, max) = (s.i, s.max);
                    let (ta, tb) = (s.ta, s.tb);
                    let (sa, sb) = (s.sa, s.sb);
                    let winner = s.winner;
                    let mid = s.mid;
                    let top_em = center_em(positions[r][i].y) - half_h;
                    let match_title = if max == 0 { "Not decided yet" } else { "Reveal this match" };
                    // Box tooltip shows the match time when known (the schedule is
                    // on the page), else the reveal hint.
                    let box_title = times
                        .with_value(|t| {
                            t.get(&mid).map(|(d, c)| format!("{} · {}", fmt_day_short(d), c))
                        })
                        .unwrap_or_else(|| match_title.to_string());
                    let stage = move || eff.with(|e| e.get(r).and_then(|row| row.get(i)).copied().unwrap_or(0));
                    // A revealed team name links to its match page; the rest of
                    // the box still reveals on click (the link stops that
                    // propagation). TBD/demo matches (no id) stay plain text.
                    // Both names share one title and highlight together (via
                    // `:has` in CSS) so they read as a single link to the match.
                    // A click scrolls to this match's row in the schedule above
                    // (so you see it in context, then open it from there); where
                    // that row isn't on the page, the href opens the match page.
                    let link_title = format!("Find {ta} vs {tb} in the schedule");
                    let muid = crate::types::match_uid(sport, mid);
                    let team_cell = move |name: String| {
                        if mid > 0 {
                            let muid = muid.clone();
                            view! {
                                <a
                                    class="bk-team bk-team-link"
                                    href=crate::types::match_path(sport, mid)
                                    title=link_title.clone()
                                    on:click=move |e: leptos::ev::MouseEvent| {
                                        e.stop_propagation();
                                        #[cfg(feature = "hydrate")]
                                        if flash_match_row(&muid) {
                                            e.prevent_default();
                                        }
                                        #[cfg(not(feature = "hydrate"))]
                                        let _ = &muid;
                                    }
                                >
                                    {name}
                                </a>
                            }
                            .into_any()
                        } else {
                            view! { <span class="bk-team">{name}</span> }.into_any()
                        }
                    };
                    // Per-slot reveal: each side of a fed match shows its feeder's
                    // winner as soon as that feeder's score is revealed, independent
                    // of the other feeder. So one finished feeder reveals one team
                    // here, and hiding a feeder hides only its slot. `feeders[k]`
                    // maps to side k (verified against the PandaScore bracket).
                    let feeders = grid_sv.with_value(|g| g[r][i].feeders.clone());
                    let (f0, f1) = (feeders.first().copied(), feeders.get(1).copied());
                    let is_real = |s: &str| !s.is_empty() && s != "TBD";
                    // A slot's label is the match's own opponent, falling back to
                    // the feeder's winner when PandaScore hasn't seeded it yet.
                    let label_for = move |own: &str, f: Option<(usize, usize)>| {
                        if is_real(own) {
                            own.to_string()
                        } else {
                            f.map(|(fr, fi)| winners_sv.with_value(|w| w[fr][fi].clone()))
                                .unwrap_or_default()
                        }
                    };
                    let label_a = label_for(&ta, f0);
                    let label_b = label_for(&tb, f1);
                    let fed_shown = move |f: Option<(usize, usize)>| {
                        f.is_some_and(|(fr, fi)| eff.with(|e| e[fr][fi] >= 2))
                    };
                    view! {
                        // The box is absolutely placed at its computed slot; only
                        // the visible box is the click target (the SVG connectors
                        // behind it are `pointer-events:none`).
                        <div
                            class="bk-match"
                            class:bk-locked=max == 0
                            style=format!("left:{x_em}em;top:{top_em}em")
                        >
                            <div
                                class="bk-box"
                                role="button"
                                tabindex=if max == 0 { "-1" } else { "0" }
                                title=box_title.clone()
                                aria-label=box_title
                                on:click=move |_| do_op(BkOp::Series(r, i))
                                on:keydown=move |e: leptos::ev::KeyboardEvent| {
                                    #[cfg(feature = "hydrate")]
                                    if matches!(e.key().as_str(), "Enter" | " ") {
                                        e.prevent_default();
                                        do_op(BkOp::Series(r, i));
                                    }
                                    #[cfg(not(feature = "hydrate"))]
                                    let _ = &e;
                                }
                            >
                                {move || {
                                    let st = stage();
                                    // A fed slot is known only while its feeder's
                                    // score is shown (the team comes from that
                                    // result); a first-round slot is known once the
                                    // match itself is revealed. So hiding a feeder's
                                    // score makes the fed team unknown again, and a
                                    // played match can't show its score until both
                                    // of its teams are known. This cascades up the
                                    // bracket: hide a semifinal score and the final's
                                    // fed side goes blank.
                                    let known_a = if f0.is_some() { fed_shown(f0) } else { st >= 1 };
                                    let known_b = if f1.is_some() { fed_shown(f1) } else { st >= 1 };
                                    let scores = st >= 2 && known_a && known_b;
                                    let show_a = is_real(&label_a) && known_a;
                                    let show_b = is_real(&label_b) && known_b;
                                    let cls_a = if scores && winner == "a" {
                                        "bk-row win"
                                    } else if scores && winner == "b" {
                                        "bk-row lose"
                                    } else {
                                        "bk-row"
                                    };
                                    let cls_b = if scores && winner == "b" {
                                        "bk-row win"
                                    } else if scores && winner == "a" {
                                        "bk-row lose"
                                    } else {
                                        "bk-row"
                                    };
                                    // When a score is hidden, show whether there's
                                    // a result waiting — a played match gets a
                                    // filled dot you can reveal; one not yet played
                                    // gets a faint dash (nothing to reveal).
                                    let score_view = move |s: Option<i64>| {
                                        if scores {
                                            view! {
                                                <span class="bk-score">
                                                    {s.map(|v| v.to_string()).unwrap_or_default()}
                                                </span>
                                            }
                                            .into_any()
                                        } else if max >= 2 {
                                            view! {
                                                <span
                                                    class="bk-score bk-pending"
                                                    title="Result hidden — click to reveal"
                                                >
                                                    "●"
                                                </span>
                                            }
                                            .into_any()
                                        } else {
                                            view! {
                                                <span
                                                    class="bk-score bk-unplayed"
                                                    title="Not played yet"
                                                >
                                                    "-"
                                                </span>
                                            }
                                            .into_any()
                                        }
                                    };
                                    let row_a = if show_a {
                                        view! {
                                            <div class=cls_a>
                                                {team_cell(label_a.clone())}
                                                {score_view(sa)}
                                            </div>
                                        }
                                        .into_any()
                                    } else {
                                        view! {
                                            <div class="bk-row bk-hidden">
                                                <span class="bk-team">"—"</span>
                                            </div>
                                        }
                                        .into_any()
                                    };
                                    let row_b = if show_b {
                                        view! {
                                            <div class=cls_b>
                                                {team_cell(label_b.clone())}
                                                {score_view(sb)}
                                            </div>
                                        }
                                        .into_any()
                                    } else {
                                        view! {
                                            <div class="bk-row bk-hidden">
                                                <span class="bk-team">"—"</span>
                                            </div>
                                        }
                                        .into_any()
                                    };
                                    view! {
                                        {row_a}
                                        {row_b}
                                    }
                                }}
                            </div>
                        </div>
                    }
                })
                .collect_view();
            view! {
                {header}
                {ms}
            }
            .into_any()
        })
        .collect::<Vec<AnyView>>();

    // Connector overlay: one elbow path per feeder edge, drawn behind the boxes in
    // the same em coordinate space, so it stays attached at any shape or zoom.
    let w_em = layout.cols as f64 * col_em;
    let h_em = positions
        .iter()
        .flatten()
        .map(|p| center_em(p.y) + half_h)
        .fold(0.0_f64, f64::max)
        + 0.4;
    let paths = layout
        .edges
        .iter()
        .map(|e| {
            let (fr, fi) = e.from;
            let (tr, ti) = e.to;
            let x1 = positions[fr][fi].col as f64 * col_em + box_w_em;
            let y1 = center_em(positions[fr][fi].y);
            let x2 = positions[tr][ti].col as f64 * col_em;
            let y2 = center_em(positions[tr][ti].y);
            let xm = (x1 + x2) / 2.0;
            view! { <path d=format!("M {x1:.2} {y1:.2} H {xm:.2} V {y2:.2} H {x2:.2}") /> }
        })
        .collect_view();

    // Section banners: double-elim's Winner's/Loser's brackets, or a traditional
    // playoff's conference/league halves and third-place match. Each stops at the
    // bracket edge so its underline never reaches the final rail to its right.
    let banner_w = bracket::banner_width_em(layout.bracket_cols, col_em, box_w_em);
    let labels = multi.then(|| {
        group_rows
            .iter()
            .filter_map(|(sec, lo, _)| {
                // The grand final / championship labels itself via its round title;
                // a banner over it would cut across the brackets it sits between.
                let label = match sec.as_str() {
                    "upper" => "Winner's Bracket",
                    "lower" => "Loser's Bracket",
                    "afc" => "AFC",
                    "nfc" => "NFC",
                    "east" => "Eastern Conference",
                    "west" => "Western Conference",
                    "al" => "American League",
                    "nl" => "National League",
                    // "third" (soccer's third-place match) is a lone column — it
                    // labels itself via its round title, no spanning banner.
                    _ => return None,
                };
                let top = center_em(*lo) - half_h - TITLE_GAP - TITLE_EM - LABEL_GAP - LABEL_EM;
                Some(view! {
                    <div class="bk-group-label" style=format!("top:{top}em;width:{banner_w:.2}em")>
                        {label}
                    </div>
                })
            })
            .collect_view()
    });

    let body = view! {
        <div class="bracket-scroll">
            <div
                class="bracket"
                style=format!("--bk-box-w:{box_w_em:.2}em;width:{w_em:.2}em;height:{h_em:.2}em")
            >
                <svg class="bk-connectors" viewBox=format!("0 0 {w_em:.2} {h_em:.2}")>
                    {paths}
                </svg>
                {labels}
                {nodes}
            </div>
        </div>
    }
    .into_any();

    // The "Bracket" title cascades the whole bracket: each click advances the
    // earliest revealable round (names → scores), unlocking the next round's
    // lineup as it goes; once everything is shown, the next click hides all.
    let bracket_on = move || eff.with(|e| e.iter().flatten().any(|&s| s >= 1));
    view! {
        <section class="detail-section">
            <div class="section-head">
                <button
                    class="section-title section-toggle"
                    class:on=bracket_on
                    title="Reveal the bracket, round by round"
                    aria-expanded=move || if bracket_on() { "true" } else { "false" }
                    on:click=move |_| do_op(BkOp::Cascade)
                >
                    "Bracket"
                </button>
                {move || (!bracket_on()).then(|| view! { <span class="section-hint">"hidden"</span> })}
            </div>
            {body}
        </section>
    }
    .into_any()
}
