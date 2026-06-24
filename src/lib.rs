// The view tree's deeply-nested types overflow the default trait-solver depth
// under the release LTO build, so bump it (mirrors the bin's main.rs).
#![recursion_limit = "512"]

pub mod app;
pub mod bracket;
pub mod server;
pub mod types;

#[cfg(feature = "ssr")]
pub mod cache;
#[cfg(feature = "ssr")]
pub mod config;
#[cfg(feature = "ssr")]
pub mod f1;
#[cfg(feature = "ssr")]
pub mod liquipedia;
#[cfg(feature = "ssr")]
pub mod mlb;
#[cfg(feature = "ssr")]
pub mod pandascore;
#[cfg(feature = "ssr")]
pub mod push;
#[cfg(feature = "ssr")]
pub mod store;
#[cfg(feature = "ssr")]
pub mod tiering;

#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn hydrate() {
    use crate::app::*;
    console_error_panic_hook::set_once();
    leptos::mount::hydrate_body(App);
}
