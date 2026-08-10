// The view tree's deeply-nested types overflow the default trait-solver depth
// under the release LTO build, so bump it (mirrors the bin's main.rs).
#![recursion_limit = "512"]
// Nine doc comments on `pub` fns link to a private neighbour (`box_width_em` to
// `FIT_BUDGET_EM`, `load_all` to `row_to_match`, ...). rustdoc warns because the
// *default* doc build would not contain the target — but this crate is `pub`
// only so the binary and the wasm build can consume it, and the docs worth
// reading are `cargo doc --document-private-items`, where every one of these
// resolves. That is the build CI runs, so the links are checked rather than
// merely tolerated. Genuinely broken links are a different lint
// (`broken_intra_doc_links`) and stay denied.
#![allow(rustdoc::private_intra_doc_links)]

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
    use crate::app::App;
    console_error_panic_hook::set_once();
    leptos::mount::hydrate_body(App);
}
