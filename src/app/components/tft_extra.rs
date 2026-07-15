//! CompeteTFT-only event sections: the per-player stream directory, official
//! broadcast channels, and per-round lobby breakdowns. Each owns a poller-cache
//! resource keyed by the full event name (empty → nothing renders), so they drop
//! into the event page next to [`super::standings::TftEventResults`].

use crate::app::pages::match_detail::fmt_viewers;
use crate::app::playoffs::section_reveal;
use crate::server::{get_event_broadcasts, get_tft_lobbies, get_tft_streamer_section};
use crate::types::{StreamView, TftLobbyRound, TftStreamerSection};
use leptos::prelude::*;

/// Split a stream URL into `(site, "/channel")` for the site's stream-link markup.
fn stream_parts(url: &str, platform: &str) -> (String, String) {
    let chan = url
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_string();
    (platform.to_string(), format!("/{chan}"))
}

/// One stream row in the esports two-column grid: an optional left gutter label,
/// the `site/channel` link, and a live badge + viewers when on air. `col` is the
/// grid-placement class the esports `streams-grid` uses.
fn stream_li(s: &StreamView, gutter: String, col: &'static str) -> impl IntoView {
    let (site, chan) = stream_parts(&s.url, platform_of(&s.url));
    let (live, viewers) = (s.live, s.viewers);
    let url = s.url.clone();
    view! {
        <li class=format!("stream {col}")>
            <span class="stream-lang">{gutter}</span>
            <a class="stream-name" href=url target="_blank" rel="noreferrer">
                <span class="stream-site">{site}</span>
                <span class="stream-chan">{chan}</span>
            </a>
            {live
                .then(|| {
                    let v = viewers.map(|n| format!(" {}", fmt_viewers(n))).unwrap_or_default();
                    view! { <span class="stream-live">"●"{v}</span> }
                })}
        </li>
    }
}

/// Split a list across the esports grid's two columns, as the match page does.
fn two_col(streams: Vec<StreamView>, gutter: impl Fn(&StreamView) -> String) -> Vec<AnyView> {
    let mid = streams.len().div_ceil(2);
    streams
        .into_iter()
        .enumerate()
        .map(|(i, s)| {
            let col = if i < mid { "stream-c1" } else { "stream-c2" };
            stream_li(&s, gutter(&s), col).into_any()
        })
        .collect()
}

/// The event's player co-streamers — live first, capped, two columns. The sheet
/// lists dozens, so the section links to it rather than printing them all. Not
/// spoiler-gated — links, not results.
#[component]
pub(crate) fn TftStreamers(event: Signal<String>) -> impl IntoView {
    let section = Resource::new(
        move || event.get(),
        |ev| async move {
            if ev.is_empty() {
                TftStreamerSection::default()
            } else {
                get_tft_streamer_section(ev).await.unwrap_or_default()
            }
        },
    );
    view! {
        <Transition>
            {move || {
                let TftStreamerSection { streams: list, sheet_url } = section
                    .get()
                    .unwrap_or_default();
                if list.is_empty() {
                    return ().into_any();
                }
                // The sheet carries every co-streamer; we show a handful.
                let more = (!sheet_url.is_empty())
                    .then(|| {
                        view! {
                            <a class="event-link" href=sheet_url target="_blank" rel="noreferrer">
                                "all streams ↗"
                            </a>
                        }
                    });
                view! {
                    <section class="detail-section">
                        <h2 class="section-title">"Player streams"</h2>
                        <ul class="streams-grid">
                            {two_col(list, |s| s.language.to_uppercase())}
                        </ul>
                        {more}
                    </section>
                }
                    .into_any()
            }}
        </Transition>
    }
}

/// The gutter label for an official broadcast, as a short code.
///
/// CompeteTFT leaves the sheet's `language` column blank on its regional channels
/// and puts the region in the label instead — but inconsistently: some are already
/// codes ("KR", "BR", "FR"), others are prose ("APAC Vietnam", "Official English
/// Broadcast"). Left alone they'd set the gutter's width by the longest one and
/// wouldn't line up with the player streams' ISO codes beside them, so the prose
/// forms are mapped down to their two-letter equivalent.
///
/// A region we have no code for keeps its (trimmed) name: a wrong code is worse
/// than a long label, and this list only covers what the sheet has actually used.
fn region_of(label: &str) -> String {
    let name = label
        .trim()
        .trim_start_matches("Official ")
        .trim_end_matches(" Broadcast")
        .trim_start_matches("APAC ")
        .trim();
    // Already a code (the sheet's own "KR"/"BR"/"FR").
    if name.len() <= 3 && name.chars().all(|c| c.is_ascii_alphabetic()) {
        return name.to_ascii_uppercase();
    }
    let code = match name.to_ascii_lowercase().as_str() {
        "english" => "EN",
        "vietnam" | "vietnamese" => "VN",
        "taiwan" => "TW",
        "korea" | "korean" => "KR",
        "brazil" | "brazilian" | "portuguese" => "BR",
        "france" | "french" => "FR",
        "spain" | "spanish" => "ES",
        "japan" | "japanese" => "JP",
        "china" | "chinese" | "mandarin" => "CN",
        "germany" | "german" => "DE",
        "turkey" | "turkish" => "TR",
        "russia" | "russian" => "RU",
        "thailand" | "thai" => "TH",
        "poland" | "polish" => "PL",
        _ => return name.to_string(),
    };
    code.to_string()
}

/// The lowercase platform for a stream URL, matching the sheet's own platform
/// strings so a broadcast link reads exactly like a player-stream link
/// ("twitch/teamfighttactics").
fn platform_of(url: &str) -> &'static str {
    let u = url.to_ascii_lowercase();
    if u.contains("twitch") {
        "twitch"
    } else if u.contains("youtube") || u.contains("youtu.be") {
        "youtube"
    } else {
        "other"
    }
}

/// The event's official broadcast channels, rendered like the player-stream links
/// (`twitch/teamfighttactics`) rather than the sheet's prose labels, plus a live
/// badge + viewer count while a channel is on air. Empty (nothing rendered) until
/// the poller has fetched the event.
#[component]
pub(crate) fn TftBroadcasts(event: Signal<String>) -> impl IntoView {
    let streams = Resource::new(
        move || event.get(),
        |ev| async move {
            if ev.is_empty() {
                Vec::new()
            } else {
                get_event_broadcasts(ev).await.unwrap_or_default()
            }
        },
    );
    view! {
        <Transition>
            {move || {
                let list = streams.get().unwrap_or_default();
                if list.is_empty() {
                    return ().into_any();
                }
                view! {
                    <section class="detail-section">
                        <h2 class="section-title">"Broadcasts"</h2>
                        <ul class="streams-grid">{two_col(list, |s| region_of(&s.name))}</ul>
                    </section>
                }
                    .into_any()
            }}
        </Transition>
    }
}

/// The placeholder score/name rows a withheld lobby box is filled with: real
/// enough to size the box, empty of anything worth hiding.
fn lob_blank_rows(seats: usize) -> impl IntoView {
    (0..seats)
        .map(|_| {
            view! {
                <span class="p">"0"</span>
                <span class="n">"—"</span>
            }
        })
        .collect_view()
}

/// An invisible stand-in for a lobby box that doesn't exist this round: same
/// structure, nothing shown. Pads short rounds out to the widest one so the grid's
/// height doesn't change as you switch rounds.
fn lob_spacer(seats: usize) -> impl IntoView {
    view! {
        <div class="lob" style="visibility:hidden" aria-hidden="true">
            <div class="lob-head">
                <span>"—"</span>
            </div>
            <div class="lob-body">{lob_blank_rows(seats)}</div>
        </div>
    }
}

/// A lobby box with its results withheld: the frame and its "Lobby N" head stay
/// put, only the score/name rows are blanked. Used while the section is collapsed,
/// so the boxes still read as boxes and the page keeps its shape. The rows are
/// placeholders rather than the real ones hidden by CSS, so a collapsed section
/// keeps the placements it's hiding out of the DOM entirely.
fn lob_withheld(seats: usize, label: &str) -> impl IntoView {
    view! {
        <div class="lob">
            <div class="lob-head">
                <span>{label.to_string()}</span>
            </div>
            <div class="lob-body" style="visibility:hidden">
                {lob_blank_rows(seats)}
            </div>
        </div>
    }
}

/// The event's per-round lobby breakdowns (who is in each 8-player lobby, their
/// result, and the caster covering it). Spoiler-gated like standings — a round's
/// placements reveal outcomes. A round selector switches the shown round.
#[component]
pub(crate) fn TftLobbies(event: Signal<String>) -> impl IntoView {
    let rounds = Resource::new(
        move || event.get(),
        |ev| async move {
            if ev.is_empty() {
                Vec::new()
            } else {
                get_tft_lobbies(ev).await.unwrap_or_default()
            }
        },
    );
    view! {
        <Transition>
            {move || {
                let list = rounds.get().unwrap_or_default();
                if list.is_empty() {
                    return ().into_any();
                }
                let ev = event.get();
                view! { <TftLobbiesInner rounds=list event=ev /> }.into_any()
            }}
        </Transition>
    }
}

#[component]
fn TftLobbiesInner(rounds: Vec<TftLobbyRound>, event: String) -> impl IntoView {
    let (revealed, toggle) = section_reveal(format!("tftlob:{event}"));
    let sel = RwSignal::new(0usize);
    // One row of round buttons per day: a full event is ~21 rounds, which wrap into
    // an unreadable block on a single line. Grouping by the label's day ("Day 1 ·
    // Round 3") also gives each row few enough buttons to size them up.
    let mut by_day: Vec<(String, Vec<(usize, String)>)> = Vec::new();
    for (i, r) in rounds.iter().enumerate() {
        let (day, num) = match r.label.split_once('·') {
            Some((d, rest)) => (
                d.trim().to_string(),
                rest.trim().rsplit(' ').next().unwrap_or("").to_string(),
            ),
            // No day in the label: one unlabelled row holding every round.
            None => (
                String::new(),
                r.label.rsplit(' ').next().unwrap_or("").to_string(),
            ),
        };
        match by_day.last_mut() {
            Some((d, v)) if *d == day => v.push((i, num)),
            _ => by_day.push((day, vec![(i, num)])),
        }
    }
    let day_rows = by_day
        .into_iter()
        .map(|(day, items)| {
            let btns = items
                .into_iter()
                .map(|(i, num)| {
                    view! {
                        <button
                            class="rnd"
                            class:active=move || sel.get() == i
                            on:click=move |_| sel.set(i)
                        >
                            {num}
                        </button>
                    }
                })
                .collect_view();
            let lbl = if day.is_empty() {
                "Round".to_string()
            } else {
                day
            };
            view! {
                <div class="lob-controls" class:hidden=move || !revealed.get()>
                    <span class="lob-lbl">{lbl}</span>
                    {btns}
                </div>
            }
        })
        .collect_view();
    let rounds = StoredValue::new(rounds);
    let body = move || {
        let all = rounds.get_value();
        // Rounds differ in lobby count (a 5-lobby Swiss round wraps to two grid
        // rows, an 8-player final is one), which shifts everything below when you
        // switch rounds. Pad every round out to the widest with invisible boxes so
        // the grid's height is constant. `visibility:hidden` (not an empty box)
        // because the height comes from the rows inside.
        let max_lobbies = all.iter().map(|r| r.lobbies.len()).max().unwrap_or(0);
        let seats = all
            .iter()
            .flat_map(|r| r.lobbies.iter())
            .map(|l| l.players.len())
            .max()
            .unwrap_or(8);
        let idx = sel.get().min(all.len().saturating_sub(1));
        let pad = max_lobbies.saturating_sub(all[idx].lobbies.len());
        let spacers = (0..pad).map(|_| lob_spacer(seats)).collect_view();
        if !revealed.get() {
            // Collapsed: keep this round's boxes, blank their rows. Same layout as
            // revealed — down to the padding — so the toggle only changes what's in
            // the boxes, never where anything sits.
            let withheld = all[idx]
                .lobbies
                .iter()
                .map(|l| lob_withheld(l.players.len(), &l.label))
                .collect_view();
            return view! {
                {withheld}
                {spacers}
            }
            .into_any();
        }
        let boxes = all[idx]
            .lobbies
            .iter()
            .map(|l| {
                let caster = l.caster_url.clone();
                // The sheet lists a lobby's players in seating order; show them by
                // result instead. `placement` is the TFT points score (8 = 1st), so
                // descending puts the winner on top. An unknown/blank score sorts last.
                let mut ranked = l.players.clone();
                ranked.sort_by_key(|p| {
                    std::cmp::Reverse(p.placement.trim().parse::<u32>().unwrap_or(0))
                });
                let players = ranked
                    .iter()
                    .map(|p| {
                        let win = p.placement == "8";
                        view! {
                            <span class="p" class:win=win>
                                {p.placement.clone()}
                            </span>
                            <span class="n">{p.player.clone()}</span>
                        }
                    })
                    .collect_view();
                view! {
                    <div class="lob">
                        <div class="lob-head">
                            <span>{l.label.clone()}</span>
                            {caster
                                .map(|u| {
                                    view! {
                                        <span class="lob-cast">
                                            <a href=u target="_blank" rel="noreferrer">
                                                "cast"
                                            </a>
                                        </span>
                                    }
                                })}
                        </div>
                        <div class="lob-body">{players}</div>
                    </div>
                }
            })
            .collect_view();
        view! {
            {boxes}
            {spacers}
        }
        .into_any()
    };
    view! {
        <section class="detail-section">
            <h2 class="section-title">"Lobby breakdown"</h2>
            <button class="f1-session-head" on:click=toggle>
                <span class="f1-session-toggle">
                    {move || {
                        if revealed.get() {
                            "hide lobbies".to_string()
                        } else {
                            "show lobbies".to_string()
                        }
                    }}
                </span>
            </button>
            {day_rows}
            <div class="lob-grid">{body}</div>
        </section>
    }
}

#[cfg(test)]
mod tests {
    use super::{platform_of, region_of, stream_parts};

    #[test]
    fn platform_of_names_the_site() {
        assert_eq!(platform_of("https://www.twitch.tv/frodan"), "twitch");
        assert_eq!(platform_of("https://youtube.com/@playtft"), "youtube");
        assert_eq!(platform_of("https://youtu.be/abc"), "youtube");
        assert_eq!(platform_of("https://www.tiktok.com/@bogiatft"), "other");
    }

    #[test]
    fn stream_parts_splits_into_site_and_channel() {
        assert_eq!(
            stream_parts("https://www.twitch.tv/teamfighttactics", "twitch"),
            ("twitch".to_string(), "/teamfighttactics".to_string())
        );
        // A trailing slash is the sheet's habit, not a channel named "".
        assert_eq!(
            stream_parts("https://www.twitch.tv/send/", "twitch"),
            ("twitch".to_string(), "/send".to_string())
        );
    }

    #[test]
    fn region_of_shortens_prose_labels() {
        assert_eq!(region_of("Official English Broadcast"), "EN");
        assert_eq!(region_of("APAC Vietnam"), "VN");
        assert_eq!(region_of("APAC Taiwan"), "TW");
    }

    #[test]
    fn region_of_keeps_codes_the_sheet_already_uses() {
        for c in ["KR", "BR", "FR"] {
            assert_eq!(region_of(c), c);
        }
        assert_eq!(region_of(" fr "), "FR");
    }

    #[test]
    fn region_of_keeps_unmapped_regions_readable() {
        // Better a long label than a confidently wrong code.
        assert_eq!(region_of("APAC Philippines"), "Philippines");
        assert_eq!(region_of("Latin America"), "Latin America");
    }
}
