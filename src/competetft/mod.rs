//! First-party TFT source: Riot's competetft.com (RSC) + the official published
//! Google Sheet (CSV). Produces the same output types as `crate::tft`
//! (Liquipedia), plus streamer/broadcast/lobby extras. Server-only.
pub mod rsc;
pub mod sheet;
