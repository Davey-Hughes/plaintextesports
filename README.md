# plaintextesports

A plain-text schedule of the big esports events and the major traditional sports,
inspired by [plaintextsports.com](https://plaintextsports.com). It runs at
[plaintextesports.com](https://plaintextesports.com).

[![The plaintextesports.com schedule partway through a day of Worlds 2026, with one match live](https://daveyhughes.com/readme/plaintextesports/demo.webp)](https://plaintextesports.com)

The esports side covers tier-1 Counter-Strike 2, League of Legends and Teamfight
Tactics, and a toggle switches to the NFL, NBA, NHL, MLB, soccer and motorsport.
Scores stay hidden until you ask for them, so you can catch up on a day's matches
without spoilers. Times are shown in your own timezone.

## What it does

- CS2 and LoL matches come from PandaScore, kept to its S and A tier events, with an
  allowlist and denylist in `src/tiering.rs` for events it tiers wrong. TFT, when it's
  turned on, comes from Riot's competetft.com and the official published sheet.
- The sports side reads each league's own public feed: ESPN for the NFL, NBA and soccer
  (the Premier League and the World Cup), the NHL and MLB APIs, Jolpica and OpenF1 for
  F1, and Orange Cat Blacktop for WRC, WEC and MotoGP.
- While an NFL, NBA, NHL or MLB game is on, the server polls that day's scoreboard every
  10 seconds and open pages refresh every 15, so the score, period and clock stay close
  to live.
- Scores, standings and brackets are hidden by default. You can show them all at once,
  reveal one match, or step through a bracket a round at a time.
- Sport tabs and event chips filter the schedule, and the address bar keeps the filter
  so a view can be shared. There's a 12h/24h switch and dark, light and OLED themes.
- Each event has a page with its full schedule, standings and bracket, and an esports
  event links to its Liquipedia page when one can be found. A match's page lists where
  to watch it (official streams, co-streams and TV) and, once it's over, the box score
  where the source has one.
- With VAPID keys set, starring a match, an event or a whole sport sends a Web Push
  notification before each match starts, even with the site closed.
- Without an API token it serves built-in demo data, so the UI works out of the box.

## How it's built

It's Rust, on a pinned nightly toolchain. [Leptos](https://leptos.dev) renders each page
on an [Axum](https://github.com/tokio-rs/axum) server and hydrates it in the browser as
WebAssembly. Background tasks poll each source on their own schedules and keep a
normalized snapshot in memory, so a page request never waits on an upstream API, and
the matches are also written to SQLite so a restart serves the last data straight away.
Reminders are encrypted and signed with `web-push-native`, in pure Rust. It ships as a
multi-stage Docker image on Alpine. Forgejo CI runs fmt, clippy with the pedantic group,
the tests and rustdoc under both the server and client feature sets, builds the release
WebAssembly against a size budget, and checks `Cargo.lock` for RustSec advisories. A
weekly job runs a live smoke test against the TFT source.

## Docs

- [How it works](docs/how-it-works.md): polling, caching and the live-score lane,
  Liquipedia links, the tier-1 filter, and the limits of PandaScore's free tier.
- [Setup and deployment](docs/setup.md): the PandaScore token, `config.toml` and its
  environment overrides, Web Push reminders, and the Docker image.
- [Development](docs/development.md): running locally, the tests and live smoke tests,
  why the watch loop needs `LEPTOS_HASH_FILES=false`, and the performance tooling.
- [Conventions](docs/conventions.md): how changes reach `main`, from linear history to
  branch protection.

## License

GNU General Public License v3.0. See [LICENSE](LICENSE).
