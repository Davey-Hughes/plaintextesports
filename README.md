# plaintextesports

A fast, plain-text-style schedule for **tier-1 Counter-Strike 2 and League of
Legends** matches — inspired by [plaintextsports.com](https://plaintextsports.com).
Built with [Leptos](https://leptos.dev) (SSR + hydration) on Axum.

It filters out the noise: only top-tier events (PandaScore tier **S + A**, tuned
with an allowlist/denylist) are shown.

## How it works

- A background task polls the [PandaScore](https://pandascore.co) API,
  normalizes matches, applies the tier-1 filter, and caches the result in
  memory. Page requests read the cache — they never block on the API.
- Polling is **adaptive**: schedules change slowly (and the free tier has no
  live feed), so it idles at ~20 min and bursts to ~1 min only while a match is
  live or starts within ~15 min, to catch final scores/status. This stays far
  under the free tier's 1,000 req/hr limit.
- Matches are also persisted to a small **SQLite** database (`DB_PATH`), keyed by
  `(id, game)` and upserted each poll with a 2-day retention window. On restart
  the app serves the last-known data instantly (no re-fetch burst), and a match
  that finishes and drops out of the API window is retained until it ages out.
- Times are formatted and matches are grouped by day in a configurable timezone
  (default `America/Los_Angeles`).
- With no API token configured, the app serves built-in demo data so the UI is
  fully usable for development.

## Tuning what counts as "tier 1"

Edit the allowlist/denylist in [`src/tiering.rs`](src/tiering.rs):

- **Base:** keep matches whose PandaScore tournament tier is `S` or `A`.
- **Denylist** (substring on any slug): force-exclude noise tagged S/A
  (academy/challenger/qualifier/showmatch…).
- **Allowlist** (exact full-slug): force-include events the API mis-tiers.
- Precedence: allowlist > denylist > base. Covered by unit tests.

## Setup

1. Get a free PandaScore token (no card): https://pandascore.co
2. Create `.env` from the example and add your token:
   ```sh
   cp .env.example .env
   # edit .env, set PANDASCORE_TOKEN=...
   ```

## Develop

```sh
cargo leptos watch        # http://127.0.0.1:4000
cargo test --features ssr # tiering + deserialization tests
```

## Configuration (env vars)

| Var | Default | Purpose |
|---|---|---|
| `PANDASCORE_TOKEN` | _(none)_ | API token; unset = demo data |
| `DISPLAY_TZ` | `America/Los_Angeles` | IANA tz for times + day grouping |
| `POLL_INTERVAL_SECS` | `1200` | Idle poll interval, seconds (min 60) |
| `POLL_ACTIVE_SECS` | `60` | Poll interval while live/imminent, seconds (min 30) |
| `UPCOMING_DAYS` | `7` | Days ahead on the homepage |
| `DB_PATH` | `data/cache.db` | SQLite cache path; empty = memory-only |

## Deploy (Docker)

```sh
docker build -t plaintextesports .
docker run -p 8080:8080 -e PANDASCORE_TOKEN=xxxx -v pte-data:/app/data plaintextesports
```

Serves on `:8080`.

Mount a volume at `/app/data` (as above) so the SQLite cache survives container
recreation. Without it the cache is ephemeral — the container still works, but
it rebuilds the cache from scratch on the first poll after each redeploy.

## Limitations

The PandaScore free tier has no real-time score feed, so "live" is inferred
(start time passed, not yet finished) and scores update on the next poll, not
instantly. Recent results depend on what the free tier returns.
