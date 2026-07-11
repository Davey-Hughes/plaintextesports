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
            });
        }
    }
    // No placement ladder yet (early stage) — show the standings on their own.
    for a in active {
        out.push(MergedRow {
            pos: (out.len() + 1).to_string(),
            player: a.participant.clone(),
            points: a.total.clone(),
            prize: String::new(),
            is_final: false,
            blank: false,
        });
    }
    out
}

/// A TFT event's results as ONE ranked ladder (# · player · pts · prize) — the
/// live lobby standings and the tournament's final placements merged by
/// [`merge_tft_results`]. Players still competing sit on top with their points; a
/// labelled divider separates the eliminated players below, muted, with their
/// final place + prize. Spoiler-gated as one block; positions stay visible to
/// reserve height while names/points/prizes hide. Column widths are precomputed
/// in monospace `ch` so revealing never reflows. Nothing renders when empty.
#[component]
fn TftResultsTable(
    standings: TftStandings,
    placements: Vec<TftPlacement>,
    event: String,
) -> impl IntoView {
    let data = merge_tft_results(&standings, &placements);
    if data.is_empty() {
        return ().into_any();
    }
    let (revealed, toggle) = section_reveal(format!("tftres:{event}"));
    let chars = |s: &str| s.chars().count();
    let pos_w = data.iter().map(|r| chars(&r.pos)).max().unwrap_or(1).max(1);
    let name_w = data
        .iter()
        .map(|r| chars(&r.player))
        .max()
        .unwrap_or(0)
        .clamp(6, 22);
    let pts_w = data
        .iter()
        .map(|r| chars(&r.points))
        .max()
        .unwrap_or(0)
        .max(3);
    let prize_w = data
        .iter()
        .map(|r| chars(&r.prize))
        .max()
        .unwrap_or(0)
        .max(5);
    let grid = format!("grid-template-columns:{pos_w}ch {name_w}ch {pts_w}ch {prize_w}ch;");
    let rows = StoredValue::new(data);
    let body = move || {
        let show = revealed.get();
        // The eliminated block is contiguous at the bottom, so one divider (before
        // the first final row) cleanly splits live from final.
        let mut sep_shown = false;
        rows.get_value()
            .into_iter()
            .map(|r| {
                let sep = (r.is_final && !sep_shown)
                    .then(|| view! { <div class="tft-elim-sep">"eliminated"</div> });
                if r.is_final {
                    sep_shown = true;
                }
                let is_final = r.is_final;
                let pos = r.pos.clone();
                let (name, points, prize) = if show {
                    let name = if r.blank {
                        "—".to_string()
                    } else {
                        r.player.clone()
                    };
                    (name, r.points.clone(), r.prize.clone())
                } else {
                    (String::new(), String::new(), String::new())
                };
                view! {
                    {sep}
                    <div class="tft-row" class:tft-final=is_final>
                        <span class="tft-rank">{pos}</span>
                        <span class="tft-name">{name}</span>
                        <span class="tft-pts">{points}</span>
                        <span class="tft-prize">{prize}</span>
                    </div>
                }
            })
            .collect_view()
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
            <div class="tft-standings-wrap">
                <div class="tft-standings" style=grid>
                    <div class="tft-row">
                        <span class="tft-h tft-rank">"#"</span>
                        <span class="tft-h tft-name">"Player"</span>
                        <span class="tft-h tft-pts">"Pts"</span>
                        <span class="tft-h tft-prize">"Prize"</span>
                    </div>
                    {body}
                </div>
            </div>
        </section>
    }
    .into_any()
}

/// A TFT event's results — the live lobby standings and the tournament's final
/// placements folded into one ranked ladder ([`TftResultsTable`]). Self-contained:
/// it owns the two Liquipedia-poller-cache resources keyed by `event` (the full
/// event name; empty → nothing renders), so it drops into both the event page and
/// the front-page single-event section.
#[component]
pub(crate) fn TftEventResults(event: Signal<String>) -> impl IntoView {
    let standings = Resource::new(
        move || event.get(),
        |ev| async move {
            if ev.is_empty() {
                TftStandings::default()
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
        // single-event filter — keeps the current table up while the next loads.
        <Transition>
            {move || {
                let ev = event.get();
                if ev.is_empty() {
                    return ().into_any();
                }
                let s = standings.get().unwrap_or_default();
                let p = placements.get().unwrap_or_default();
                // Show only when there's real content: live standings, or at least
                // one decided placement. (All-"TBD" placements with no standings —
                // an untouched prizepool — render nothing.)
                if s.is_empty() && !p.iter().any(|x| tft_real(&x.participant)) {
                    return ().into_any();
                }
                view! { <TftResultsTable standings=s placements=p event=ev /> }.into_any()
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
