//! CompeteTFT-only event sections: the per-player stream directory, official
//! broadcast channels, and per-round lobby breakdowns. Each owns a poller-cache
//! resource keyed by the full event name (empty → nothing renders), so they drop
//! into the event page next to [`super::standings::TftEventResults`].

use crate::server::{get_tft_broadcasts, get_tft_streamers};
use crate::types::{TftBroadcast, TftStreamer};
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

/// The event's official broadcast channels (regional + co-streams + watch parties).
/// Not spoiler-gated.
#[component]
pub(crate) fn TftBroadcasts(event: Signal<String>) -> impl IntoView {
    let broadcasts = Resource::new(
        move || event.get(),
        |ev| async move {
            if ev.is_empty() {
                Vec::new()
            } else {
                get_tft_broadcasts(ev).await.unwrap_or_default()
            }
        },
    );
    view! {
        <Transition>
            {move || {
                let list = broadcasts.get().unwrap_or_default();
                if list.is_empty() {
                    return ().into_any();
                }
                view! {
                    <section class="detail-section">
                        <h2 class="section-title">"Official broadcasts"</h2>
                        <ul class="bcast-grid">
                            {list
                                .into_iter()
                                .map(|b: TftBroadcast| {
                                    let label = if b.language.is_empty() {
                                        b.label.clone()
                                    } else {
                                        format!("{} · {}", b.label, b.language)
                                    };
                                    view! {
                                        <li class="bcast">
                                            <span class="b-reg">{label}</span>
                                            <span class="b-links">
                                                {b
                                                    .links
                                                    .into_iter()
                                                    .enumerate()
                                                    .map(|(i, (p, u))| {
                                                        view! {
                                                            {(i > 0).then_some(" · ")}
                                                            <a href=u target="_blank" rel="noreferrer">
                                                                {p}
                                                            </a>
                                                        }
                                                    })
                                                    .collect_view()}
                                            </span>
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
