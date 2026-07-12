// The view tree's deeply-nested types overflow the default trait-solver depth
// under the release LTO build, so bump it (mirrors the bin's main.rs).
#![recursion_limit = "512"]

pub mod app;
pub mod bracket;
pub mod reveal;
pub mod server;
pub mod types;

#[cfg(feature = "ssr")]
pub mod bracket_build;
#[cfg(feature = "ssr")]
pub mod cache;
#[cfg(feature = "ssr")]
pub mod competetft;
#[cfg(feature = "ssr")]
pub mod config;
#[cfg(feature = "ssr")]
pub mod espn;
#[cfg(feature = "ssr")]
pub mod f1;
#[cfg(feature = "ssr")]
pub mod feed;
#[cfg(feature = "ssr")]
pub mod http;
#[cfg(feature = "ssr")]
pub mod icons;
#[cfg(feature = "ssr")]
pub mod liquipedia;
#[cfg(feature = "ssr")]
pub mod mlb;
#[cfg(feature = "ssr")]
pub mod nhl;
#[cfg(feature = "ssr")]
pub mod ocblacktop;
#[cfg(feature = "ssr")]
pub mod openf1;
#[cfg(feature = "ssr")]
pub mod pandascore;
#[cfg(feature = "ssr")]
pub mod push;
#[cfg(feature = "ssr")]
pub mod soop;
#[cfg(feature = "ssr")]
pub mod store;
#[cfg(feature = "ssr")]
pub mod tft;
#[cfg(feature = "ssr")]
pub mod tiering;
#[cfg(feature = "ssr")]
pub mod twitch;
#[cfg(feature = "ssr")]
pub mod twitch_discover;
#[cfg(feature = "ssr")]
pub mod twitch_gql;
#[cfg(feature = "ssr")]
pub mod watch;
#[cfg(feature = "ssr")]
pub mod youtube;

#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn hydrate() {
    use crate::app::*;
    console_error_panic_hook::set_once();
    leptos::mount::hydrate_body(App);
}
