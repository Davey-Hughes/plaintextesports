pub mod app;
pub mod server;
pub mod types;

#[cfg(feature = "ssr")]
pub mod cache;
#[cfg(feature = "ssr")]
pub mod config;
#[cfg(feature = "ssr")]
pub mod pandascore;
#[cfg(feature = "ssr")]
pub mod tiering;

#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn hydrate() {
    use crate::app::*;
    console_error_panic_hook::set_once();
    leptos::mount::hydrate_body(App);
}
