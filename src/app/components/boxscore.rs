//! The shared, sport-agnostic game-stats renderer. Every traditional sport (and,
//! later, esports) normalizes to `BoxScore` (`crate::types`), and this module is
//! the single place it's drawn — so all sports look identical. Spoiler-gated: the
//! whole section stays a "show" button until the viewer opens it.
use crate::app::*;
use crate::types::{BoxScore, LeaderCard, LineRow, LineScore, PlayerTable, ScoreEvent, StatPair};

/// The game-stats section: one spoiler reveal wrapping the shared sub-views.
/// `key` scopes the reveal (the match id), tied to `RevealEnd` for pruning.
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
    let (revealed, toggle) = section_reveal(format!("boxscore:{key}"));
    let content = StoredValue::new(box_score);
    view! {
        <section class="detail-section boxscore">
            <button class="f1-session-head" on:click=toggle>
                <span class="f1-session-name">"Game stats"</span>
                <span class="f1-session-toggle">
                    {move || if revealed.get() { "hide" } else { "show" }}
                </span>
            </button>
            {move || {
                revealed
                    .get()
                    .then(|| {
                        let BoxScore { line, leaders, team_stats, player_tables, timeline, .. } =
                            content.get_value();
                        view! {
                            <div class="boxscore-body">
                                {line.map(|l| view! { <LineScoreGrid line=l /> })}
                                {(!leaders.is_empty()).then(|| view! { <Leaders leaders=leaders /> })}
                                {(!team_stats.is_empty())
                                    .then(|| view! { <StatComparison stats=team_stats /> })}
                                {(!timeline.is_empty())
                                    .then(|| view! { <ScoringTimeline events=timeline /> })}
                                {(!player_tables.is_empty())
                                    .then(|| view! { <PlayerStatTables tables=player_tables /> })}
                            </div>
                        }
                    })
            }}
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
            .map(|v| view! { <td>{v}</td> })
            .collect_view();
        let tot = totals
            .iter()
            .map(|t| {
                let v = if home { t.home.clone() } else { t.away.clone() };
                view! { <td class="ls-total">{v}</td> }
            })
            .collect_view();
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
            view! {
                <div class="cmp-row">
                    <span class="cmp-val cmp-away">{s.away}</span>
                    <span class="cmp-mid">
                        <span class="cmp-label">{s.label}</span>
                        <span class="cmp-bars">
                            <span class="cmp-bar cmp-bar-away" style=format!("width:{away_share}%")></span>
                            <span class="cmp-bar cmp-bar-home" style=format!("width:{home_share}%")></span>
                        </span>
                    </span>
                    <span class="cmp-val cmp-home">{s.home}</span>
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
            view! {
                <div class="leader-card">
                    <span class="leader-cat">{l.category}</span>
                    <span class="leader-name">{l.name} " " <span class="leader-team">{l.team}</span></span>
                    <span class="leader-line">{l.line}</span>
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
            let when = [e.segment, e.clock]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            view! {
                <li class=format!("tl-row tl-{}", e.kind)>
                    <span class="tl-when">{when}</span>
                    <span class="tl-team">{e.team}</span>
                    <span class="tl-desc">{e.description}</span>
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
                        .map(|v| view! { <td>{v}</td> })
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
