//! CompeteTFT-only event sections: the per-player stream directory, official
//! broadcast channels, and per-round lobby breakdowns. Each owns a poller-cache
//! resource keyed by the full event name (empty → nothing renders), so they drop
//! into the event page next to [`super::standings::TftEventResults`].

use crate::app::pages::match_detail::StreamsList;
use crate::app::playoffs::section_reveal;
use crate::server::{get_event_broadcasts, get_tft_lobbies, get_tft_streamers};
use crate::types::{TftLobbyRound, TftStreamer};
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

/// The event's per-player co-streamer directory (CompeteTFT sheet). Reuses the
/// esports `streams-grid` markup. Not spoiler-gated — links, not results.
#[component]
pub(crate) fn TftStreamers(event: Signal<String>) -> impl IntoView {
    let streamers = Resource::new(
        move || event.get(),
        |ev| async move {
            if ev.is_empty() {
                Vec::new()
            } else {
                get_tft_streamers(ev).await.unwrap_or_default()
            }
        },
    );
    view! {
        <Transition>
            {move || {
                let list = streamers.get().unwrap_or_default();
                if list.is_empty() {
                    return ().into_any();
                }
                view! {
                    <section class="detail-section">
                        <h2 class="section-title">"Player streams"</h2>
                        <ul class="streams-grid">
                            {list
                                .into_iter()
                                .map(|s: TftStreamer| {
                                    let (site, chan) = stream_parts(&s.url, &s.platform);
                                    view! {
                                        <li class="stream">
                                            <span class="stream-lang"></span>
                                            <a
                                                class="stream-name"
                                                href=s.url
                                                target="_blank"
                                                rel="noreferrer"
                                            >
                                                <span class="stream-site">{site}</span>
                                                <span class="stream-chan">{chan}</span>
                                            </a>
                                            <span class="stream-live"></span>
                                        </li>
                                    }
                                })
                                .collect_view()}
                        </ul>
                    </section>
                }
                    .into_any()
            }}
        </Transition>
    }
}

/// The event's official broadcast channels as a compact stream row — the same
/// treatment the other esports get on a match page (live badge + viewers when
/// on air). Empty (nothing rendered) until the poller has fetched the event.
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
                view! { <StreamsList streams=list /> }
            }}
        </Transition>
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
    // Round buttons show just the round number ("Day 1 · Round 3" -> "3").
    let round_btns = rounds
        .iter()
        .map(|r| r.label.clone())
        .enumerate()
        .map(|(i, label)| {
            let num = label.rsplit(' ').next().unwrap_or("").to_string();
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
    let rounds = StoredValue::new(rounds);
    let body = move || {
        if !revealed.get() {
            return ().into_any();
        }
        let all = rounds.get_value();
        let idx = sel.get().min(all.len().saturating_sub(1));
        all[idx]
            .lobbies
            .iter()
            .map(|l| {
                let caster = l.caster_url.clone();
                let players = l
                    .players
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
            .collect_view()
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
            <div class="lob-controls" class:hidden=move || !revealed.get()>
                <span class="lob-lbl">"Round"</span>
                {round_btns}
            </div>
            <div class="lob-grid">{body}</div>
        </section>
    }
}
