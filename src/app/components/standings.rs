//! Standings view components that aren't the playoff bracket: F1 / motorsport
//! championship tables and the traditional-sports standings wrapper.
use crate::app::*;
use crate::types::{TftDayPanel, TftPlacement, TftStandings};

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

/// Whether a participant label is a real competitor (non-blank, not "TBD").
/// Lives here rather than in the `tft` module because this component compiles for
/// the client too, where the server-only `tft` module isn't present.
fn tft_real(name: &str) -> bool {
    let n = name.trim();
    !n.is_empty() && !n.eq_ignore_ascii_case("TBD")
}

/// Shared column widths (monospace `ch`) so every day tab lays out identically —
/// the total column lines up across tabs, and switching tabs never shifts a
/// column. `max_games` reserves the widest day's game columns on every tab (left
/// blank where a day has fewer games).
#[derive(Clone, Copy)]
struct TabLayout {
    rank_w: usize,
    name_w: usize,
    max_games: usize,
    game_w: usize,
    /// Width of the aligned value column ("Total").
    val_w: usize,
    /// Width of the trailing prize column, or 0 when no row carries a prize (then
    /// the column is omitted entirely rather than left as dead space).
    prize_w: usize,
}

/// Compute the shared layout across all day panels.
fn tab_layout(panels: &[TftDayPanel]) -> TabLayout {
    let chars = |s: &str| s.chars().count();
    let mut rank_w = 1;
    let mut name_w = 6; // "Player"
    let mut val_w = 5; // fits "Total"
    let mut game_w = 2;
    let mut prize_w = 0; // stays 0 (column omitted) unless a row carries a prize
    let max_games = panels
        .iter()
        .map(|p| p.standings.game_count)
        .max()
        .unwrap_or(0);
    for p in panels {
        for r in &p.standings.rows {
            rank_w = rank_w.max(chars(&r.rank));
            name_w = name_w.max(chars(&r.participant));
            val_w = val_w.max(chars(&r.total));
            prize_w = prize_w.max(chars(&r.prize));
            for g in &r.games {
                game_w = game_w.max(chars(g));
            }
        }
    }
    if max_games > 0 {
        game_w = game_w.max(chars(&format!("G{max_games}")));
    }
    // Breathing room before the right-aligned trailing columns: the game columns
    // run right up against "Total" otherwise. The table is narrower than its
    // container, so this spends slack rather than forcing a scroll.
    val_w += 3;
    TabLayout {
        rank_w,
        name_w: name_w.clamp(6, 22),
        max_games,
        game_w,
        val_w,
        prize_w,
    }
}

/// The `grid-template-columns` for a standings grid: rank · name · (the shared
/// `max_games` game columns, when any) · the trailing value column(s) — "Total"
/// on a day tab. Shared so every day tab stays column-aligned.
fn grid_cols(l: &TabLayout, trailing: &[usize]) -> String {
    let tail: String = trailing.iter().map(|w| format!(" {w}ch")).collect();
    if l.max_games == 0 {
        format!("grid-template-columns:{}ch {}ch{tail};", l.rank_w, l.name_w)
    } else {
        format!(
            "grid-template-columns:{}ch {}ch repeat({},{}ch){tail};",
            l.rank_w, l.name_w, l.max_games, l.game_w
        )
    }
}

/// A raw day-panel grid: rank · player · per-game points · total. The game columns
/// are the shared `max_games` width (blank past this day's own game count), so the
/// total column lines up with the other tabs. Reveal-blanked.
fn day_grid_view(s: &TftStandings, l: &TabLayout, show: bool) -> AnyView {
    let max_games = l.max_games;
    // Prize is its own column after "Total" (there's room), rather than borrowing a
    // game cell. Omitted entirely when no row carries one.
    let grid = if l.prize_w > 0 {
        grid_cols(l, &[l.val_w, l.prize_w])
    } else {
        grid_cols(l, &[l.val_w])
    };
    let has_prize_col = l.prize_w > 0;
    let gc = s.game_count;
    let head_games = (0..max_games)
        .map(|i| {
            let lab = if i < gc {
                format!("G{}", i + 1)
            } else {
                String::new()
            };
            view! { <span class="tft-h tft-g">{lab}</span> }
        })
        .collect_view();
    // The eliminated block is contiguous at the bottom (day2/finals rows are
    // built that way in `split_day_panels`), so one divider (before the first
    // eliminated row) cleanly splits live from final.
    let mut sep_shown = false;
    // Every tab emits exactly one divider so the section's height doesn't jump
    // when switching tabs: a day with nobody eliminated (Day 1) reserves the same
    // box with an invisible one. `visibility:hidden` (not an empty div) because the
    // divider's height comes from its text line, which would otherwise collapse.
    let has_elim = s.rows.iter().any(|r| r.eliminated);
    let rows = s
        .rows
        .iter()
        .map(|r| {
            // Eliminated: greyed, prize instead of per-game cells, cumulative
            // total retained.
            let elim = r.eliminated;
            let prize = r.prize.clone();
            let sep = (elim && !sep_shown)
                .then(|| view! { <div class="tft-elim-sep">"eliminated"</div> });
            if elim {
                sep_shown = true;
            }
            let games = (0..max_games)
                .map(|i| {
                    // An eliminated player played no games this stage, so their game
                    // cells are blank; their prize sits in the trailing column.
                    let v = if !show || elim {
                        String::new()
                    } else if i < gc {
                        r.games.get(i).cloned().unwrap_or_default()
                    } else {
                        String::new()
                    };
                    view! { <span class="tft-g">{v}</span> }
                })
                .collect_view();
            let (name, total) = if show {
                (r.participant.clone(), r.total.clone())
            } else {
                (String::new(), String::new())
            };
            let prize_cell = has_prize_col.then(|| {
                let p = if show { prize.clone() } else { String::new() };
                view! { <span class="tft-prize">{p}</span> }
            });
            let rank = r.rank.clone();
            view! {
                {sep}
                <div class="tft-row" class:tft-final=elim>
                    <span class="tft-rank">{rank}</span>
                    <span class="tft-name">{name}</span>
                    {games}
                    <span class="tft-total">{total}</span>
                    {prize_cell}
                </div>
            }
        })
        .collect_view();
    view! {
        <div class="tft-standings" style=grid>
            <div class="tft-row">
                <span class="tft-h tft-rank">"#"</span>
                <span class="tft-h tft-name">"Player"</span>
                {head_games}
                <span class="tft-h tft-total">"Total"</span>
                {has_prize_col.then(|| view! { <span class="tft-h tft-prize"></span> })}
            </div>
            {rows}
            {(!has_elim).then(|| {
                view! { <div class="tft-elim-sep" style="visibility:hidden">"eliminated"</div> }
            })}
        </div>
    }
    .into_any()
}

/// A TFT event's standings shown as tabs: one per day/stage panel, each carrying
/// the full field for that stage (eliminated players greyed, showing their
/// prize). One spoiler reveal gates them all; the tab bar and positions stay
/// visible while the values hide. Defaults to the latest day's tab.
#[component]
fn TftResultsTabs(
    panels: Vec<TftDayPanel>,
    _placements: Vec<TftPlacement>,
    event: String,
) -> impl IntoView {
    if panels.is_empty() {
        return ().into_any();
    }
    let (revealed, toggle) = section_reveal(format!("tftres:{event}"));
    // Tabs are the day panels; the latest is the default. (The old synthesized
    // "Current" tab is gone: each panel now carries the full field itself, with
    // eliminated players greyed and carrying their prize.)
    let tabs: Vec<(usize, String)> = panels.iter().map(|p| p.label.clone()).enumerate().collect();
    let active = RwSignal::new(panels.len().saturating_sub(1));
    let layout = tab_layout(&panels);
    let panels = StoredValue::new(panels);
    let tab_bar = tabs
        .into_iter()
        .map(|(i, label)| {
            view! {
                <button
                    class="tft-tab"
                    class:tft-tab-active=move || active.get() == i
                    on:click=move |_| active.set(i)
                >
                    {label}
                </button>
            }
        })
        .collect_view();
    let body = move || {
        let show = revealed.get();
        let idx = active.get();
        let days = panels.get_value();
        day_grid_view(&days[idx.min(days.len() - 1)].standings, &layout, show)
    };
    view! {
        <section class="detail-section" id="tftsec-results">
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
            <div class="tft-tabs">{tab_bar}</div>
            <div class="tft-standings-wrap">{body}</div>
        </section>
    }
    .into_any()
}

/// A TFT event's results — the tournament's day/stage standings panels plus its
/// final placements, shown as tabs ([`TftResultsTabs`]). Self-contained: it owns
/// the two TFT-poller-cache resources keyed by `event` (the full event
/// name; empty → nothing renders), so it drops into both the event page and the
/// front-page single-event section.
#[component]
pub(crate) fn TftEventResults(event: Signal<String>) -> impl IntoView {
    let panels = Resource::new(
        move || event.get(),
        |ev| async move {
            if ev.is_empty() {
                Vec::new()
            } else {
                get_tft_standings(ev).await.unwrap_or_default()
            }
        },
    );
    let placements = Resource::new(
        move || event.get(),
        |ev| async move {
            if ev.is_empty() {
                Vec::new()
            } else {
                get_tft_placements(ev).await.unwrap_or_default()
            }
        },
    );
    view! {
        // Transition (not Suspense) so re-keying on a new event — switching the
        // single-event filter — keeps the current tabs up while the next loads.
        <Transition>
            {move || {
                let ev = event.get();
                if ev.is_empty() {
                    return ().into_any();
                }
                let ps = panels.get().unwrap_or_default();
                let pl = placements.get().unwrap_or_default();
                // Show only when there's real content: at least one non-empty day
                // panel, or a decided placement. (An untouched all-"TBD" prizepool
                // with no standings renders nothing.)
                let has_panels = ps.iter().any(|p| !p.standings.is_empty());
                let has_decided = pl.iter().any(|x| tft_real(&x.participant));
                if !has_panels && !has_decided {
                    return ().into_any();
                }
                view! { <TftResultsTabs panels=ps _placements=pl event=ev /> }.into_any()
            }}
        </Transition>
    }
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

#[cfg(test)]
mod tests {
    use super::{grid_cols, tab_layout, tft_real};
    use crate::types::{TftDayPanel, TftStandingRow, TftStandings};

    fn row(rank: &str, name: &str, total: &str, games: &[&str], prize: &str) -> TftStandingRow {
        TftStandingRow {
            rank: rank.to_string(),
            participant: name.to_string(),
            total: total.to_string(),
            games: games.iter().map(|g| (*g).to_string()).collect(),
            status: String::new(),
            prize: prize.to_string(),
            eliminated: !prize.is_empty(),
        }
    }

    fn panel(label: &str, game_count: usize, rows: Vec<TftStandingRow>) -> TftDayPanel {
        TftDayPanel {
            label: label.to_string(),
            standings: TftStandings { game_count, rows },
        }
    }

    #[test]
    fn tft_real_rejects_blank_and_tbd() {
        assert!(tft_real("Dishsoap"));
        assert!(!tft_real(""));
        assert!(!tft_real("   "));
        assert!(!tft_real("TBD"));
        assert!(!tft_real("tbd"));
    }

    #[test]
    fn tab_layout_is_shared_across_days_so_tabs_dont_shift() {
        // Day 2 has more games and a longer name; both days must still get the
        // widest day's widths, or switching tabs moves the columns.
        let panels = vec![
            panel("Day 1", 6, vec![row("1", "abc", "40", &["8"], "")]),
            panel(
                "Day 2",
                13,
                vec![row("10", "a_very_long_player", "128", &["8"], "")],
            ),
        ];
        let l = tab_layout(&panels);
        assert_eq!(l.max_games, 13, "widest day's game columns reserved");
        assert_eq!(l.rank_w, 2, "fits \"10\"");
        assert_eq!(l.name_w, 18, "fits the longest name");
        assert_eq!(l.game_w, 3, "fits the \"G13\" header, wider than any score");
    }

    #[test]
    fn tab_layout_pads_total_for_breathing_room() {
        let panels = vec![panel("Day 1", 1, vec![row("1", "x", "8", &["8"], "")])];
        // "Total" is 5 wide; the column is padded past it so the game columns
        // don't run right up against it.
        assert_eq!(tab_layout(&panels).val_w, 8);
    }

    #[test]
    fn tab_layout_omits_prize_column_until_a_row_carries_one() {
        let none = vec![panel("Day 1", 1, vec![row("1", "x", "8", &["8"], "")])];
        assert_eq!(tab_layout(&none).prize_w, 0, "no prize ⇒ no column");

        let some = vec![panel(
            "Finals",
            1,
            vec![
                row("1", "x", "8", &["8"], ""),
                row("2", "y", "7", &["7"], "$11,000"),
            ],
        )];
        assert_eq!(tab_layout(&some).prize_w, 7, "sized to \"$11,000\"");
    }

    #[test]
    fn tab_layout_clamps_name_column() {
        let short = vec![panel("D", 0, vec![row("1", "ab", "8", &[], "")])];
        assert_eq!(
            tab_layout(&short).name_w,
            6,
            "never narrower than \"Player\""
        );

        let long = vec![panel("D", 0, vec![row("1", &"x".repeat(40), "8", &[], "")])];
        assert_eq!(
            tab_layout(&long).name_w,
            22,
            "capped so the grid still fits"
        );
    }

    #[test]
    fn grid_cols_reserves_every_game_column_plus_trailing() {
        let panels = vec![panel("Day 1", 3, vec![row("1", "x", "8", &["8"], "")])];
        let l = tab_layout(&panels);
        assert_eq!(
            grid_cols(&l, &[l.val_w]),
            "grid-template-columns:1ch 6ch repeat(3,2ch) 8ch;"
        );
        // Prize rides in its own trailing column, after the total.
        assert_eq!(
            grid_cols(&l, &[l.val_w, 7]),
            "grid-template-columns:1ch 6ch repeat(3,2ch) 8ch 7ch;"
        );
    }

    #[test]
    fn grid_cols_drops_the_game_columns_when_there_are_none() {
        let panels = vec![panel("Placements", 0, vec![row("1", "x", "8", &[], "")])];
        let l = tab_layout(&panels);
        assert_eq!(
            grid_cols(&l, &[l.val_w]),
            "grid-template-columns:1ch 6ch 8ch;"
        );
    }
}
