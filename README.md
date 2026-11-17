# plaintextesports

A plain-text schedule for esports and traditional sports that hides every score until
you ask for it. It runs at [plaintextesports.com](https://plaintextesports.com).

[![The plaintextesports.com schedule partway through a day of Worlds 2026, with one match live](https://daveyhughes.com/readme/plaintextesports/demo.webp)](https://plaintextesports.com)

Each day is a list of matches, one to a line: the start time in your own timezone,
who's playing, and whether the match is upcoming, live or over. The results stay
hidden, so you can check what's on, or catch up on a day you missed, without finding
out who won.

The site opens on the top Counter-Strike 2, League of Legends and Teamfight Tactics
events, and a toggle switches it to the NFL, NBA, NHL, MLB, soccer and motorsport. The
format comes from [plaintextsports.com](https://plaintextsports.com), a text-only
scoreboard for traditional sports.

## Features

- Scores, standings and brackets stay hidden until you ask. You can show everything at
  once, reveal a single match, or step through a bracket one round at a time.
- During an NFL, NBA, NHL or MLB game, the score, period and clock update on the page
  every 15 seconds, without a reload.
- Tabs for each sport and chips for each event narrow the schedule. The filter stays in
  the address bar, so you can send someone the exact view you're looking at.
- Every event has a page with its full schedule, standings and bracket. An esports
  event also links to its Liquipedia page when one can be found.
- Every match has a page that lists where to watch it: official streams, co-streams and
  TV. Once the match is over, the page shows the box score if the source has one.
- Star a match, an event or a whole sport, and your browser will notify you before each
  of its matches starts, even with the site closed.
- You can switch between 12-hour and 24-hour times, and between dark, light and OLED
  themes.

## Sports and data sources

| Sport | Data from |
|---|---|
| Counter-Strike 2 and League of Legends, top-tier events only | [PandaScore](https://pandascore.co) |
| Teamfight Tactics | Riot's competetft.com and its published sheet |
| NFL and NBA | ESPN |
| NHL | the NHL's own API |
| MLB | the MLB Stats API |
| Soccer: the Premier League and the World Cup | ESPN |
| F1 | Jolpica and OpenF1 |
| WRC, WEC and MotoGP | Orange Cat Blacktop |

"Top-tier" means the events PandaScore rates S or A, minus qualifiers, academy leagues
and show matches. PandaScore's free tier has no live feed, so during an esports match
the score changes when the next poll lands, about once a minute.

## Run it locally

You need Rust, installed through [rustup](https://rustup.rs), and cargo-leptos:

```sh
git clone https://github.com/Davey-Hughes/plaintextesports
cd plaintextesports
cargo install cargo-leptos --version 0.3.11 --locked
cargo leptos serve
```

Then open <http://127.0.0.1:4000>. On first use, rustup reads `rust-toolchain.toml` and
installs the pinned nightly toolchain and its WebAssembly target. Without a PandaScore
token the site shows built-in demo data. [Setup and deployment](docs/setup.md) covers
the token, the other settings and the Docker image, and
[Development](docs/development.md) covers the edit-and-reload loop and the tests.

## How it's built

It's Rust, on a pinned nightly toolchain. [Leptos](https://leptos.dev) renders each page
on an [Axum](https://github.com/tokio-rs/axum) server, and the browser then runs the same
page as WebAssembly. Background tasks poll each source on its own schedule and keep the
results in memory, so a page request never waits on an outside API. Matches are also
saved to SQLite, so a restart serves the last data straight away. Reminders use
Web Push, encrypted and signed in pure Rust with `web-push-native`. The app ships as a
multi-stage Docker image on Alpine.

Forgejo CI runs rustfmt, then runs clippy (with its pedantic lints), the tests and
rustdoc against both the server and browser builds. It also holds the release WebAssembly to a size
budget and checks `Cargo.lock` for RustSec advisories. A weekly job runs a live smoke
test against the TFT source.

## Docs

- [How it works](docs/how-it-works.md): polling, caching and the live-score lane,
  Liquipedia links, the tier-1 filter, and the limits of PandaScore's free tier.
- [Setup and deployment](docs/setup.md): the PandaScore token, `config.toml` and its
  environment overrides, Web Push reminders, and the Docker image.
- [Development](docs/development.md): running locally, the tests and live smoke tests,
  why the watch loop needs `LEPTOS_HASH_FILES=false`, and the performance tooling.
- [Conventions](docs/conventions.md): how changes reach `main`, from linear history to
  branch protection, and how code comments refer to UI controls.

## License

GNU General Public License v3.0. See [LICENSE](LICENSE).
