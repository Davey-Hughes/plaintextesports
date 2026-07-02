//! The shared, sport-agnostic game-stats renderer. Every traditional sport (and,
//! later, esports) normalizes to `BoxScore` (`crate::types`), and this module is
//! the single place it's drawn — so all sports look identical. The structure and
//! the values both always render, so column/card sizes are fixed from first paint;
//! the spoiler values are hidden via CSS `visibility` until scores are revealed, so
//! revealing never shifts the layout. The "Game stats" title is a toggle button
//! (same pattern as the standings) that reveals it; the global scores toggle
//! reveals it too.
use crate::app::*;
use crate::types::{BoxScore, LeaderCard, LineRow, LineScore, PlayerTable, ScoreEvent, StatPair};

/// The game-stats section, rendered below the standings. The "Game stats" title
/// toggles a `key`-scoped, persisted section reveal (and the global scores toggle
/// reveals it too) — exactly like the standings section. Renders nothing when the
/// box score is empty.
#[component]
pub(crate) fn BoxScoreView(box_score: BoxScore, key: String) -> impl IntoView {
    // Nothing to show at all → render nothing (the caller also guards, but be safe).
    let empty = box_score.line.is_none()
        && box_score.leaders.is_empty()
        && box_score.team_stats.is_empty()
        && box_score.player_tables.is_empty()
        && box_score.timeline.is_empty();
    if empty {
        return ().into_any();
    }
    // Click the "Game stats" title to reveal/hide — same section-reveal machinery
    // as the standings (also revealed by the global scores toggle).
    let (revealed, toggle) = section_reveal(format!("boxscore:{key}"));
    let BoxScore {
        line,
        leaders,
        team_stats,
        player_tables,
        timeline,
        ..
    } = box_score;
    view! {
        <section class="detail-section boxscore">
            <div class="section-head">
                <button
                    class="section-title section-toggle"
                    class:on=move || revealed.get()
                    title=move || if revealed.get() { "Hide the game stats" } else { "Show the game stats" }
                    aria-expanded=move || if revealed.get() { "true" } else { "false" }
                    on:click=toggle
                >
                    "Game stats"
                </button>
                {move || (!revealed.get()).then(|| view! { <span class="section-hint">"hidden"</span> })}
            </div>
            // Structure AND values always render (so widths/heights are fixed); the
            // `.revealed` class flips the values' CSS visibility, so revealing fills
            // them in with zero layout shift.
            <div class="boxscore-body" class:revealed=move || revealed.get()>
                {line.map(|l| view! { <LineScoreGrid line=l /> })}
                {(!leaders.is_empty()).then(|| view! { <Leaders leaders=leaders /> })}
                {(!team_stats.is_empty()).then(|| view! { <StatComparison stats=team_stats /> })}
                {(!timeline.is_empty()).then(|| view! { <ScoringTimeline events=timeline /> })}
                {(!player_tables.is_empty()).then(|| view! { <PlayerStatTables tables=player_tables /> })}
            </div>
        </section>
    }
    .into_any()
}

#[component]
fn LineScoreGrid(line: LineScore) -> impl IntoView {
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
    let row = |r: LineRow, totals: &[StatPair], home: bool| {
        let cells = r
            .segment_values
            .into_iter()
            .map(|v| view! { <td><span class="bx-spoiler">{v}</span></td> })
            .collect_view();
        let tot = totals
            .iter()
            .map(|t| {
                let v = if home { t.home.clone() } else { t.away.clone() };
                view! { <td class="ls-total"><span class="bx-spoiler">{v}</span></td> }
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
fn StatComparison(stats: Vec<StatPair>) -> impl IntoView {
    let rows = stats
        .into_iter()
        .map(|s| {
            let away_share = s.away_share.unwrap_or(50);
            let home_share = 100u8.saturating_sub(away_share);
            // Values + bars are spoilers (the bar reveals who leads); the label is
            // not. `.bx-spoiler` / `.cmp-bar` hide via CSS until revealed.
            view! {
                <div class="cmp-row">
                    <span class="cmp-val cmp-away bx-spoiler">{s.away}</span>
                    <span class="cmp-mid">
                        <span class="cmp-label">{s.label}</span>
                        <span class="cmp-bars">
                            <span class="cmp-bar cmp-bar-away" style=format!("width:{away_share}%")></span>
                            <span class="cmp-bar cmp-bar-home" style=format!("width:{home_share}%")></span>
                        </span>
                    </span>
                    <span class="cmp-val cmp-home bx-spoiler">{s.home}</span>
                </div>
            }
        })
        .collect_view();
    view! { <div class="stat-compare">{rows}</div> }
}

#[component]
fn Leaders(leaders: Vec<LeaderCard>) -> impl IntoView {
    let cards = leaders
        .into_iter()
        .map(|l| {
            // The category reserves the card; name + line are spoilers. Rendering
            // them always (hidden via CSS) keeps each card its final size, so it
            // doesn't grow when revealed.
            view! {
                <div class="leader-card">
                    <span class="leader-cat">{l.category}</span>
                    <span class="leader-name bx-spoiler">
                        {l.name} " " <span class="leader-team">{l.team}</span>
                    </span>
                    <span class="leader-line bx-spoiler">{l.line}</span>
                </div>
            }
        })
        .collect_view();
    view! { <div class="leaders">{cards}</div> }
}

#[component]
fn ScoringTimeline(events: Vec<ScoreEvent>) -> impl IntoView {
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
                    <span class="tl-team bx-spoiler">{e.team}</span>
                    <span class="tl-desc bx-spoiler">{e.description}</span>
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
fn PlayerStatTables(tables: Vec<PlayerTable>) -> impl IntoView {
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
                        .map(|v| view! { <td><span class="bx-spoiler">{v}</span></td> })
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
