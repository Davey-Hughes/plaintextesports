//! The shared, sport-agnostic game-stats renderer. Every traditional sport (and,
//! later, esports) normalizes to `BoxScore` (`crate::types`), and this module is
//! the single place it's drawn — so all sports look identical. The section's
//! structure (headings, tables, rows, labels) always renders so its space is
//! reserved from first paint; the spoiler VALUES (scores, stat numbers, leader
//! lines) blank until the match's scores are revealed, so revealing fills them in
//! place without shifting the layout.
use crate::app::*;
use crate::types::{BoxScore, LeaderCard, LineRow, LineScore, PlayerTable, ScoreEvent, StatPair};

/// The game-stats section, rendered below the standings. `reveal` is the match's
/// spoiler state (global scores toggle or this match individually revealed); when
/// false the structure still renders but the values are blank. Renders nothing
/// when the box score is empty.
#[component]
pub(crate) fn BoxScoreView(box_score: BoxScore, reveal: Memo<bool>) -> impl IntoView {
    // Nothing to show at all → render nothing (the caller also guards, but be safe).
    let empty = box_score.line.is_none()
        && box_score.leaders.is_empty()
        && box_score.team_stats.is_empty()
        && box_score.player_tables.is_empty()
        && box_score.timeline.is_empty();
    if empty {
        return ().into_any();
    }
    // Store the data so the body can re-render (values ↔ blanks) when `reveal`
    // flips, without changing the row/column structure — so the space never shifts.
    let content = StoredValue::new(box_score);
    view! {
        <section class="detail-section boxscore">
            <h2 class="section-title">"Game stats"</h2>
            {move || {
                let show = reveal.get();
                let BoxScore { line, leaders, team_stats, player_tables, timeline, .. } =
                    content.get_value();
                view! {
                    <div class="boxscore-body">
                        {line.map(|l| view! { <LineScoreGrid line=l show=show /> })}
                        {(!leaders.is_empty()).then(|| view! { <Leaders leaders=leaders show=show /> })}
                        {(!team_stats.is_empty())
                            .then(|| view! { <StatComparison stats=team_stats show=show /> })}
                        {(!timeline.is_empty())
                            .then(|| view! { <ScoringTimeline events=timeline show=show /> })}
                        {(!player_tables.is_empty())
                            .then(|| view! { <PlayerStatTables tables=player_tables show=show /> })}
                    </div>
                }
            }}
        </section>
    }
    .into_any()
}

/// Blank a spoiler value when scores are hidden (keeps the cell, drops the text).
fn hide(show: bool, v: String) -> String {
    if show {
        v
    } else {
        String::new()
    }
}

#[component]
fn LineScoreGrid(line: LineScore, show: bool) -> impl IntoView {
    let LineScore {
        segments,
        away,
        home,
        totals,
    } = line;
    // Segment labels (inning/period numbers) and the R/H/E column labels aren't
    // spoilers — always shown so the grid keeps its shape.
    let head = segments
        .iter()
        .map(|s| view! { <th>{s.clone()}</th> })
        .collect_view();
    let total_head = totals
        .iter()
        .map(|t| view! { <th class="ls-total">{t.label.clone()}</th> })
        .collect_view();
    let row = move |r: LineRow, totals: &[StatPair], home: bool| {
        let cells = r
            .segment_values
            .into_iter()
            .map(|v| view! { <td>{hide(show, v)}</td> })
            .collect_view();
        let tot = totals
            .iter()
            .map(|t| {
                let v = if home { t.home.clone() } else { t.away.clone() };
                view! { <td class="ls-total">{hide(show, v)}</td> }
            })
            .collect_view();
        // The team abbreviation isn't a spoiler — always shown.
        view! {
            <tr>
                <th class="ls-team">{r.abbrev}</th>
                {cells}
                {tot}
            </tr>
        }
    };
    view! {
        <div class="ls-wrap">
            <table class="linescore">
                <thead>
                    <tr>
                        <th></th>
                        {head}
                        {total_head}
                    </tr>
                </thead>
                <tbody>
                    {row(away, &totals, false)}
                    {row(home, &totals, true)}
                </tbody>
            </table>
        </div>
    }
}

#[component]
fn StatComparison(stats: Vec<StatPair>, show: bool) -> impl IntoView {
    let rows = stats
        .into_iter()
        .map(|s| {
            // When hidden, the bars sit neutral (50/50) so they don't reveal who
            // leads, and the values blank — the row keeps its fixed-width columns.
            let away_share = if show { s.away_share.unwrap_or(50) } else { 50 };
            let home_share = 100u8.saturating_sub(away_share);
            view! {
                <div class="cmp-row">
                    <span class="cmp-val cmp-away">{hide(show, s.away)}</span>
                    <span class="cmp-mid">
                        <span class="cmp-label">{s.label}</span>
                        <span class="cmp-bars">
                            <span class="cmp-bar cmp-bar-away" style=format!("width:{away_share}%")></span>
                            <span class="cmp-bar cmp-bar-home" style=format!("width:{home_share}%")></span>
                        </span>
                    </span>
                    <span class="cmp-val cmp-home">{hide(show, s.home)}</span>
                </div>
            }
        })
        .collect_view();
    view! { <div class="stat-compare">{rows}</div> }
}

#[component]
fn Leaders(leaders: Vec<LeaderCard>, show: bool) -> impl IntoView {
    let cards = leaders
        .into_iter()
        .map(|l| {
            // The category label reserves the card; name + line are spoilers.
            view! {
                <div class="leader-card">
                    <span class="leader-cat">{l.category}</span>
                    <span class="leader-name">
                        {hide(show, l.name)} " "
                        <span class="leader-team">{hide(show, l.team)}</span>
                    </span>
                    <span class="leader-line">{hide(show, l.line)}</span>
                </div>
            }
        })
        .collect_view();
    view! { <div class="leaders">{cards}</div> }
}

#[component]
fn ScoringTimeline(events: Vec<ScoreEvent>, show: bool) -> impl IntoView {
    let rows = events
        .into_iter()
        .map(|e| {
            // The segment/clock (when) isn't a spoiler; the team + description are.
            let when = [e.segment, e.clock]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            view! {
                <li class=format!("tl-row tl-{}", e.kind)>
                    <span class="tl-when">{when}</span>
                    <span class="tl-team">{hide(show, e.team)}</span>
                    <span class="tl-desc">{hide(show, e.description)}</span>
                </li>
            }
        })
        .collect_view();
    view! {
        <div class="timeline">
            <h3 class="boxscore-subhead">"Scoring"</h3>
            <ol class="tl-list">{rows}</ol>
        </div>
    }
}

#[component]
fn PlayerStatTables(tables: Vec<PlayerTable>, show: bool) -> impl IntoView {
    let out = tables
        .into_iter()
        .map(|t| {
            // Column headers + player names/positions reserve the table shape; the
            // stat values are the spoilers.
            let head = t
                .columns
                .iter()
                .map(|c| view! { <th>{c.clone()}</th> })
                .collect_view();
            let body = t
                .rows
                .into_iter()
                .map(|r| {
                    let vals = r
                        .values
                        .into_iter()
                        .map(|v| view! { <td>{hide(show, v)}</td> })
                        .collect_view();
                    view! {
                        <tr>
                            <th class="ps-name">{r.name} <span class="ps-note">{r.note}</span></th>
                            {vals}
                        </tr>
                    }
                })
                .collect_view();
            view! {
                <div class="player-table">
                    <h3 class="boxscore-subhead">{t.title}</h3>
                    <div class="ps-wrap">
                        <table class="player-stats">
                            <thead>
                                <tr>
                                    <th class="ps-name"></th>
                                    {head}
                                </tr>
                            </thead>
                            <tbody>{body}</tbody>
                        </table>
                    </div>
                </div>
            }
        })
        .collect_view();
    view! { <div class="player-tables">{out}</div> }
}
