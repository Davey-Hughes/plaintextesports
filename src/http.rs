//! Shared HTTP plumbing (server-only): the project's outbound User-Agent and the
//! boxed error alias every network-facing helper returns.
//!
//! Many of the keyless public APIs we poll (Jolpica, OpenF1, ocblacktop,
//! Liquipedia, …) expect a real User-Agent with contact info, and a few rate-
//! limit by it. Keeping one copy here stops the feeds from drifting apart.

/// User-Agent sent on every outbound API request. Single source of truth.
pub const USER_AGENT: &str =
    "plaintextesports/0.1 (https://github.com/ralphpotato/plaintextesports; ralphpotato@gmail.com)";

/// Boxed, thread-safe error returned by the network/IO helpers across the feed
/// and push modules.
pub type DynError = Box<dyn std::error::Error + Send + Sync>;
