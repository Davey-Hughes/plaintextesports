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
/// Mirrors the server-only `tft::is_real_participant` — re-inlined because this
/// component compiles for the client too, where the `tft` module isn't present.
fn tft_real(name: &str) -> bool {
    let n = name.trim();
    !n.is_empty() && !n.eq_ignore_ascii_case("TBD")
}

/// One row of the merged TFT results ladder: a tournament position with whoever
/// holds it, plus the value we have for them — live points for a player still in,
/// prize money for one who's out.
#[derive(Clone)]
struct MergedRow {
    pos: String,
    player: String,
    points: String,
    prize: String,
    /// Placement locked in (player eliminated) — muted, below the divider.
    is_final: bool,
    /// An open slot with no live-standings player behind it — rendered "—".
    blank: bool,
    /// Qualification note carried from the standing row (CompeteTFT), e.g. "→ Day 3".
    status: String,
}

/// Fold a stage's live standings and the tournament's final placements into one
/// ranked ladder. The two are disjoint: standings holds the players still
/// competing (ranked by points, placement still "TBD"); the *decided* placement
/// rows are the eliminated players (final place + prize). Since anyone still in
/// finishes above anyone already out, they stack cleanly — active players fill the
/// open ("TBD") slots from the top in standings order, eliminated players sit
/// below with their locked place. Positions come from the placement ladder; with
/// no placements yet it's simply the standings.
fn merge_tft_results(standings: &TftStandings, placements: &[TftPlacement]) -> Vec<MergedRow> {
    // Participants with a decided (eliminated) placement. CompeteTFT's standings
    // panel lists everyone (including eliminated players), so we key on this to
    // avoid showing a player both as a live row and a final-placement row.
    let finalized: std::collections::HashSet<&str> = placements
        .iter()
        .filter(|p| tft_real(&p.participant))
        .map(|p| p.participant.as_str())
        .collect();
    let mut active = standings.rows.iter();
    let mut out = Vec::new();
    for p in placements {
        if tft_real(&p.participant) {
            // A decided (eliminated) place: final, carrying the prize.
            out.push(MergedRow {
                pos: p.place.clone(),
                player: p.participant.clone(),
                points: String::new(),
                prize: p.prize.clone(),
                is_final: true,
                blank: false,
                status: String::new(),
            });
        } else if let Some(a) = active.next() {
            // A still-open place, filled by the next live-standings player.
            out.push(MergedRow {
                pos: p.place.clone(),
                player: a.participant.clone(),
                points: a.total.clone(),
                prize: String::new(),
                is_final: false,
                blank: false,
                status: a.status.clone(),
            });
        } else {
            // Open place with no standings behind it (e.g. a finals with no live
            // table) — a blank slot so the ladder still reads as complete.
            out.push(MergedRow {
                pos: p.place.clone(),
                player: String::new(),
                points: String::new(),
                prize: String::new(),
                is_final: false,
                blank: true,
                status: String::new(),
            });
        }
    }
    // Remaining live players (no placement slot, or CompeteTFT's still-qualified
    // players carrying only a status). Skip anyone already shown as a finalized
    // placement so they aren't listed twice.
    for a in active {
        if finalized.contains(a.participant.as_str()) {
            continue;
        }
        // Use the standing row's own rank (CompeteTFT's still-qualified players
        // rank above the eliminated block); fall back to append order if absent.
        let pos = if a.rank.is_empty() {
            (out.len() + 1).to_string()
        } else {
            a.rank.clone()
        };
        out.push(MergedRow {
            pos,
            player: a.participant.clone(),
            points: a.total.clone(),
            prize: String::new(),
            is_final: false,
            blank: false,
            status: a.status.clone(),
        });
    }
    // Order by finishing position so still-in players (low positions) sit above
    // the eliminated block regardless of which source populated the rows. Ties
    // like "3-4" sort by their leading number. A no-op for Liquipedia (already
    // ordered); required for CompeteTFT, whose placements start below the cut.
    out.sort_by_key(|r| {
        r.pos
            .split('-')
            .next()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .unwrap_or(u32::MAX)
    });
    out
}

/// Shared column widths (monospace `ch`) so every tab lays out identically — the
/// total/points column lines up across the day tabs and with the current tab, and
/// switching tabs never shifts a column. `max_games` reserves the widest day's
/// game columns on every tab (left blank where a day / the current view has none).
#[derive(Clone, Copy)]
struct TabLayout {
    rank_w: usize,
    name_w: usize,
    max_games: usize,
    game_w: usize,
    /// Width of the aligned value column — "Total" on day tabs, "Pts" on current.
    val_w: usize,
    prize_w: usize,
    /// Width of the CompeteTFT qualification-status column (0 when none present).
    status_w: usize,
}

/// Compute the shared layout across all day panels and the current-tab rows.
fn tab_layout(panels: &[TftDayPanel], current: &[MergedRow]) -> TabLayout {
    let chars = |s: &str| s.chars().count();
    let mut rank_w = 1;
    let mut name_w = 6; // "Player"
    let mut val_w = 5; // fits "Total" (and "Pts")
    let mut game_w = 2;
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
            for g in &r.games {
                game_w = game_w.max(chars(g));
            }
        }
    }
    let mut prize_w = 5; // "Prize"
    let mut status_w = 0;
    for r in current {
        rank_w = rank_w.max(chars(&r.pos));
        name_w = name_w.max(chars(&r.player));
        val_w = val_w.max(chars(&r.points));
        prize_w = prize_w.max(chars(&r.prize));
        status_w = status_w.max(chars(&r.status));
    }
    if max_games > 0 {
        game_w = game_w.max(chars(&format!("G{max_games}")));
    }
    TabLayout {
        rank_w,
        name_w: name_w.clamp(6, 22),
        max_games,
        game_w,
        val_w,
        prize_w,
        status_w,
    }
}

/// The `grid-template-columns` for a standings grid: rank · name · (the shared
/// `max_games` game columns, when any) · the trailing value column(s) — "Total"
/// on a day tab, "Pts" + "Prize" on the current tab. Shared so the two grids stay
/// column-aligned.
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
    let grid = grid_cols(l, &[l.val_w]);
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
    let rows = s
        .rows
        .iter()
        .map(|r| {
            let games = (0..max_games)
                .map(|i| {
                    let v = if show && i < gc {
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
            let rank = r.rank.clone();
            view! {
                <div class="tft-row">
                    <span class="tft-rank">{rank}</span>
                    <span class="tft-name">{name}</span>
                    {games}
                    <span class="tft-total">{total}</span>
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
            </div>
            {rows}
        </div>
    }
    .into_any()
}

/// The merged "current" grid: rank · player · live points · prize — players still
/// in on top, a muted "eliminated" divider, then the eliminated (dimmed) with their
/// final place + prize. The `max_games` game columns are reserved (blank) so the
/// "Pts" column lines up with the day tabs' "Total". Reveal-blanked.
fn current_grid_view(rows: &[MergedRow], l: &TabLayout, show: bool) -> AnyView {
    let max_games = l.max_games;
    let has_status = l.status_w > 0;
    let trailing: Vec<usize> = if has_status {
        vec![l.val_w, l.prize_w, l.status_w]
    } else {
        vec![l.val_w, l.prize_w]
    };
    let grid = grid_cols(l, &trailing);
    // The eliminated block is contiguous at the bottom, so one divider (before the
    // first final row) cleanly splits live from final.
    let mut sep_shown = false;
    let body = rows
        .iter()
        .map(|r| {
            let sep = (r.is_final && !sep_shown)
                .then(|| view! { <div class="tft-elim-sep">"eliminated"</div> });
            if r.is_final {
                sep_shown = true;
            }
            let is_final = r.is_final;
            let pos = r.pos.clone();
            let (name, points, prize, status) = if show {
                let name = if r.blank {
                    "—".to_string()
                } else {
                    r.player.clone()
                };
                (name, r.points.clone(), r.prize.clone(), r.status.clone())
            } else {
                (String::new(), String::new(), String::new(), String::new())
            };
            let is_q = status.starts_with('→');
            let spacers = (0..max_games)
                .map(|_| view! { <span class="tft-g"></span> })
                .collect_view();
            view! {
                {sep}
                <div class="tft-row" class:tft-final=is_final>
                    <span class="tft-rank">{pos}</span>
                    <span class="tft-name">{name}</span>
                    {spacers}
                    <span class="tft-pts">{points}</span>
                    <span class="tft-prize">{prize}</span>
                    {has_status
                        .then(|| view! { <span class="tft-status" class:q=is_q>{status}</span> })}
                </div>
            }
        })
        .collect_view();
    let head_spacers = (0..max_games)
        .map(|_| view! { <span class="tft-h tft-g"></span> })
        .collect_view();
    view! {
        <div class="tft-standings" style=grid>
            <div class="tft-row">
                <span class="tft-h tft-rank">"#"</span>
                <span class="tft-h tft-name">"Player"</span>
                {head_spacers}
                <span class="tft-h tft-pts">"Pts"</span>
                <span class="tft-h tft-prize">"Prize"</span>
                {has_status.then(|| view! { <span class="tft-h tft-status"></span> })}
            </div>
            {body}
        </div>
    }
    .into_any()
}

/// A TFT event's standings shown as tabs: one per day/stage panel (each with its
/// per-game detail) plus a synthesized "Current" tab — the latest day's standing
/// folded together with the final placements (eliminated players + prizes). One
/// spoiler reveal gates them all; the tab bar and positions stay visible while the
/// values hide. Defaults to the "Current" tab.
#[component]
fn TftResultsTabs(
    panels: Vec<TftDayPanel>,
    placements: Vec<TftPlacement>,
    event: String,
) -> impl IntoView {
    let latest = panels
        .last()
        .map(|p| p.standings.clone())
        .unwrap_or_default();
    let current = merge_tft_results(&latest, &placements);
    let has_current = !current.is_empty();
    if panels.is_empty() && !has_current {
        return ().into_any();
    }
    let (revealed, toggle) = section_reveal(format!("tftres:{event}"));
    let n_days = panels.len();
    // Tabs: day panels at 0..n_days, then "Current" at n_days. Start on Current.
    let mut tabs: Vec<(usize, String)> =
        panels.iter().map(|p| p.label.clone()).enumerate().collect();
    if has_current {
        tabs.push((n_days, "Current".to_string()));
    }
    let active = RwSignal::new(if has_current {
        n_days
    } else {
        n_days.saturating_sub(1)
    });
    // One shared layout so the total/points column lines up across every tab.
    let layout = tab_layout(&panels, &current);
    let panels = StoredValue::new(panels);
    let current = StoredValue::new(current);
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
        if idx < days.len() {
            day_grid_view(&days[idx].standings, &layout, show)
        } else {
            current_grid_view(&current.get_value(), &layout, show)
        }
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
/// the two Liquipedia-poller-cache resources keyed by `event` (the full event
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
                view! { <TftResultsTabs panels=ps placements=pl event=ev /> }.into_any()
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
    use super::*;
    use crate::types::TftStandingRow;

    fn stand(rows: &[(&str, &str)]) -> TftStandings {
        TftStandings {
            game_count: 0,
            rows: rows
                .iter()
                .enumerate()
                .map(|(i, (name, total))| TftStandingRow {
                    rank: (i + 1).to_string(),
                    participant: (*name).to_string(),
                    total: (*total).to_string(),
                    games: Vec::new(),
                    status: String::new(),
                })
                .collect(),
        }
    }
    fn place(p: &str, name: &str, prize: &str) -> TftPlacement {
        TftPlacement {
            place: p.to_string(),
            participant: name.to_string(),
            prize: prize.to_string(),
        }
    }

    #[test]
    fn merges_live_standings_over_eliminated_placements() {
        let standings = stand(&[("Alpha", "40"), ("Beta", "37")]);
        // Two open (TBD) places on top, one decided (eliminated) below.
        let placements = vec![
            place("1", "TBD", "$100"),
            place("2", "", "$50"),
            place("3", "Zed", "$25"),
        ];
        let rows = merge_tft_results(&standings, &placements);
        assert_eq!(rows.len(), 3);
        // Open slots filled by live standings, in standings order — points, no
        // prize, provisional.
        assert_eq!(
            (rows[0].player.as_str(), rows[0].points.as_str()),
            ("Alpha", "40")
        );
        assert!(rows[0].prize.is_empty() && !rows[0].is_final && !rows[0].blank);
        assert_eq!(rows[1].player, "Beta");
        // Decided place: final, muted, prize, no live points.
        assert_eq!(
            (rows[2].player.as_str(), rows[2].prize.as_str()),
            ("Zed", "$25")
        );
        assert!(rows[2].is_final && rows[2].points.is_empty());
    }

    #[test]
    fn merge_without_placements_is_just_standings() {
        let rows = merge_tft_results(&stand(&[("Solo", "9")]), &[]);
        assert_eq!(rows.len(), 1);
        assert_eq!(
            (rows[0].pos.as_str(), rows[0].player.as_str()),
            ("1", "Solo")
        );
        assert!(!rows[0].is_final);
    }

    #[test]
    fn merge_without_standings_blanks_open_slots() {
        let placements = vec![place("1", "TBD", "$100"), place("2", "Out", "$50")];
        let rows = merge_tft_results(&TftStandings::default(), &placements);
        assert_eq!(rows.len(), 2);
        assert!(
            rows[0].blank,
            "an open slot with no live standings is blank"
        );
        assert!(rows[1].is_final && rows[1].player == "Out");
    }
}
