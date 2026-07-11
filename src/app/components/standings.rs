//! Standings view components that aren't the playoff bracket: F1 / motorsport
//! championship tables and the traditional-sports standings wrapper.
use crate::app::*;
use crate::types::{TftPlacement, TftStandings};

/// The drivers' and constructors' championship standings as of a GP's round,
/// shown on the F1 event page. Spoiler-gated as one block (it encodes results).
#[component]
pub(crate) fn F1StandingsView(standings: F1Standings, season: i64, round: i64) -> impl IntoView {
    let (revealed, toggle) = section_reveal(format!("f1stand:{season}:{round}"));
    let asof = standings.round;
    let drivers = StoredValue::new(standings.drivers);
    let constructors = StoredValue::new(standings.constructors);
    // Always render both tables (blanked until revealed) so revealing doesn't
    // shift the page — the position column stays, names/points blank out.
    let body = move || {
        let show = revealed.get();
        let drv = drivers
            .get_value()
            .into_iter()
            .map(|r| {
                let (name, con, cabbr, pts, flag, clogo) = if show {
                    (r.name, r.detail, r.constructor_abbrev, r.points, r.flag, r.constructor_logo)
                } else {
                    (String::new(), String::new(), String::new(), String::new(), String::new(), String::new())
                };
                view! {
                    <li class="f1-standing-row">
                        <span class="f1-pos">{r.pos}</span>
                        <span class="f1-standing-name">{team_logo(&flag, "f1-flag")}{name}</span>
                        <span class="f1-standing-con">{clogo_icon(&clogo)}{team_cell(con, cabbr)}</span>
                        <span class="f1-standing-pts">{pts}</span>
                    </li>
                }
            })
            .collect_view();
        let con = constructors
            .get_value()
            .into_iter()
            .map(|r| {
                let (name, cabbr, pts, clogo) = if show {
                    (r.name, r.constructor_abbrev, r.points, r.constructor_logo)
                } else {
                    (String::new(), String::new(), String::new(), String::new())
                };
                view! {
                    <li class="f1-standing-row f1-standing-row-con">
                        <span class="f1-pos">{r.pos}</span>
                        <span class="f1-standing-name">{clogo_icon(&clogo)}{team_cell(name, cabbr)}</span>
                        <span class="f1-standing-pts">{pts}</span>
                    </li>
                }
            })
            .collect_view();
        view! {
            <div class="f1-standings-tables">
                <div class="f1-standings-table">
                    <h3 class="f1-standings-sub">"Drivers"</h3>
                    <ol class="f1-standings">{drv}</ol>
                </div>
                <div class="f1-standings-table">
                    <h3 class="f1-standings-sub">"Constructors"</h3>
                    <ol class="f1-standings f1-standings-con">{con}</ol>
                </div>
            </div>
        }
    };
    view! {
        <section class="detail-section" id="f1sec-standings">
            <h2 class="section-title f1-results-title">
                "Standings"
                <span class="f1-standings-asof">{format!(" · after Round {asof}")}</span>
            </h2>
            <button class="f1-session-head" on:click=toggle>
                <span class="f1-session-toggle">
                    {move || {
                        if revealed.get() { "hide standings".to_string() } else { "show standings".to_string() }
                    }}
                </span>
            </button>
            {body}
        </section>
    }
}

/// WRC/WEC championship standings — a set of titled tables (WRC: drivers /
/// co-drivers / manufacturers; WEC: per class × kind). Like the F1 table it's a
/// spoiler, so it's blanked behind a reveal toggle. `league` is the series name.
#[component]
pub(crate) fn MotorStandingsView(standings: MotorStandings, league: String) -> impl IntoView {
    let (revealed, toggle) = section_reveal(format!("motorstand:{league}"));
    let tables = StoredValue::new(standings.tables);
    // Keep the rows present (position column stays) but blank names/points until
    // revealed, so revealing doesn't shift the page.
    let body = move || {
        let show = revealed.get();
        // Group the tables by class (WEC: Hypercar / LMGT3; WRC: one empty group)
        // so each class is its own flex row — a class's tables wrap among
        // themselves, but a different class always starts on a new line.
        let mut groups: Vec<(String, Vec<MotorStandingTable>)> = Vec::new();
        for t in tables.get_value() {
            match groups.last_mut() {
                Some((g, v)) if *g == t.group => v.push(t),
                _ => groups.push((t.group.clone(), vec![t])),
            }
        }
        groups
            .into_iter()
            .map(|(group, group_tables)| {
                let class_tables = group_tables
                    .into_iter()
                    .map(|t| {
                        let rows = t
                            .rows
                            .into_iter()
                            .map(|r| {
                                let (name, pts, flag) = if show {
                                    (r.name, r.points, r.flag)
                                } else {
                                    (String::new(), String::new(), String::new())
                                };
                                view! {
                                    <li class="f1-standing-row f1-standing-row-con">
                                        <span class="f1-pos">{r.pos}</span>
                                        <span class="f1-standing-name">{team_logo(&flag, "f1-flag")}{name}</span>
                                        <span class="f1-standing-pts">{pts}</span>
                                    </li>
                                }
                            })
                            .collect_view();
                        view! {
                            <div class="f1-standings-table">
                                <h3 class="f1-standings-sub">{t.title}</h3>
                                <ol class="f1-standings f1-standings-con">{rows}</ol>
                            </div>
                        }
                    })
                    .collect_view();
                view! {
                    <div class="motor-standings-group">
                        {(!group.is_empty())
                            .then(|| view! { <h3 class="motor-standings-class">{group}</h3> })}
                        <div class="f1-standings-tables">{class_tables}</div>
                    </div>
                }
            })
            .collect_view()
    };
    view! {
        <section class="detail-section" id="motorsec-standings">
            <h2 class="section-title f1-results-title">"Standings"</h2>
            <button class="f1-session-head" on:click=toggle>
                <span class="f1-session-toggle">
                    {move || {
                        if revealed.get() { "hide standings".to_string() } else { "show standings".to_string() }
                    }}
                </span>
            </button>
            <div class="motor-standings">{body}</div>
        </section>
    }
}

/// A TFT tournament's final placements (place · participant · prize), shown on its
/// event page. Spoiler-gated as one block (it's a result); the place stays visible
/// to reserve height while the names/prizes hide. Nothing renders when empty
/// (undecided tournament / non-TFT event).
#[component]
pub(crate) fn TftPlacementsView(placements: Vec<TftPlacement>, event: String) -> impl IntoView {
    if placements.is_empty() {
        return ().into_any();
    }
    let (revealed, toggle) = section_reveal(format!("tftplace:{event}"));
    let rows = StoredValue::new(placements);
    let body = move || {
        let show = revealed.get();
        rows.get_value()
            .into_iter()
            .map(|p| {
                // Undecided places (an ongoing tournament's still-"TBD" spots) show
                // a dash for the name, so the table reads as complete.
                let decided = {
                    let n = p.participant.trim();
                    !n.is_empty() && !n.eq_ignore_ascii_case("TBD")
                };
                let (name, prize) = if show {
                    let name = if decided {
                        p.participant
                    } else {
                        "—".to_string()
                    };
                    (name, p.prize)
                } else {
                    (String::new(), String::new())
                };
                view! {
                    <li class="f1-standing-row">
                        <span class="f1-pos">{p.place}</span>
                        <span class="f1-standing-name">{name}</span>
                        <span class="f1-standing-pts">{prize}</span>
                    </li>
                }
            })
            .collect_view()
    };
    view! {
        <section class="detail-section" id="tftsec-placements">
            <h2 class="section-title f1-results-title">"Final Placements"</h2>
            <button class="f1-session-head" on:click=toggle>
                <span class="f1-session-toggle">
                    {move || {
                        if revealed.get() {
                            "hide placements".to_string()
                        } else {
                            "show placements".to_string()
                        }
                    }}
                </span>
            </button>
            // The 3-column (`-con`) grid: pos · participant · prize — matching the
            // three cells per row (the plain `.f1-standings` grid is 4-column).
            <ol class="f1-standings f1-standings-con">{body}</ol>
        </section>
    }
    .into_any()
}

/// A TFT lobby standings grid (rank · player · per-game points · total), shown on
/// its event page. Spoiler-gated as one block; the ranks stay visible while the
/// names/points hide. The grid is wide, so it scrolls horizontally within its own
/// container. Nothing renders when empty.
#[component]
pub(crate) fn TftStandingsView(standings: TftStandings, event: String) -> impl IntoView {
    if standings.rows.is_empty() {
        return ().into_any();
    }
    let (revealed, toggle) = section_reveal(format!("tftstand:{event}"));
    let game_count = standings.game_count;
    let data = standings.rows;
    // Precompute each column's width (in monospace `ch`) from the shown values, so
    // the grid is laid out identically whether the spoiler is hidden or shown —
    // revealing never reflows the columns/headers. Widths cover the header labels
    // too, and the name column is capped (longer names ellipsize).
    let chars = |s: &str| s.chars().count();
    let rank_w = data
        .iter()
        .map(|r| chars(&r.rank))
        .max()
        .unwrap_or(0)
        .max(1);
    let name_w = data
        .iter()
        .map(|r| chars(&r.participant))
        .max()
        .unwrap_or(0)
        .clamp(6, 22);
    let total_w = data
        .iter()
        .map(|r| chars(&r.total))
        .max()
        .unwrap_or(0)
        .max(5);
    let game_w = data
        .iter()
        .flat_map(|r| r.games.iter())
        .map(|g| chars(g))
        .max()
        .unwrap_or(0)
        .max(chars(&format!("G{game_count}")));
    let grid = format!(
        "grid-template-columns:{rank_w}ch {name_w}ch repeat({game_count},{game_w}ch) {total_w}ch;"
    );
    let rows = StoredValue::new(data);
    let head_games = (1..=game_count)
        .map(|i| view! { <span class="tft-h tft-g">{format!("G{i}")}</span> })
        .collect_view();
    let body = move || {
        let show = revealed.get();
        rows.get_value()
            .into_iter()
            .map(|r| {
                let games = (0..game_count)
                    .map(|i| {
                        let v = if show {
                            r.games.get(i).cloned().unwrap_or_default()
                        } else {
                            String::new()
                        };
                        view! { <span class="tft-g">{v}</span> }
                    })
                    .collect_view();
                let (name, total) = if show {
                    (r.participant, r.total)
                } else {
                    (String::new(), String::new())
                };
                view! {
                    <div class="tft-row">
                        <span class="tft-rank">{r.rank}</span>
                        <span class="tft-name">{name}</span>
                        {games}
                        <span class="tft-total">{total}</span>
                    </div>
                }
            })
            .collect_view()
    };
    view! {
        <section class="detail-section" id="tftsec-standings">
            <h2 class="section-title f1-results-title">"Standings"</h2>
            <button class="f1-session-head" on:click=toggle>
                <span class="f1-session-toggle">
                    {move || {
                        if revealed.get() {
                            "hide standings".to_string()
                        } else {
                            "show standings".to_string()
                        }
                    }}
                </span>
            </button>
            <div class="tft-standings-wrap">
                <div class="tft-standings" style=grid>
                    <div class="tft-row">
                        <span class="tft-h tft-rank">"#"</span>
                        <span class="tft-h tft-name">"Player"</span>
                        {head_games}
                        <span class="tft-h tft-total">"Total"</span>
                    </div>
                    {body}
                </div>
            </div>
        </section>
    }
    .into_any()
}

/// The division/conference/group standings for a single traditional league,
/// shown beneath the home-page schedule once the filters narrow to one league
/// (the team-sport analogue of the esports single-event bracket). Empty league =
/// nothing rendered.
#[component]
pub(crate) fn TraditionalStandings(league: Memo<String>) -> impl IntoView {
    let stages = Resource::new(
        move || league.get(),
        |lg| async move {
            if lg.is_empty() {
                Ok(Vec::new())
            } else {
                get_event_stages(lg).await
            }
        },
    );
    view! {
        // Transition, not Suspense: `stages` re-keys whenever the home filter
        // narrows to a different single league, so keep the current table visible
        // while the next loads instead of blanking it out between selections.
        <Transition>
            {move || {
                stages
                    .get()
                    .and_then(Result::ok)
                    .filter(|v| !v.is_empty())
                    .map(|v| view! { <EventStages stages=v /> })
            }}
        </Transition>
    }
}
