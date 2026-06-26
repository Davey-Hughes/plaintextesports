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
  live or starts within ~15 min, to catch final scores/status.
- Fetching is **depth-aware**: the idle cadence does a *deep scan*, paginating
  the upcoming feed across the whole `UPCOMING_DAYS` window so a tier-1 event is
  found even behind hundreds of low-tier matches; frequent active polls fetch
  only the first page. A rate-limited or failed later page returns a partial
  result and the SQLite cache fills the rest in on the next poll. All of this
  stays far under the free tier's 1,000 req/hr limit.
- Matches are also persisted to a small **SQLite** database (`DB_PATH`), keyed by
  `(id, game)` and upserted each poll with a 2-day retention window. On restart
  the app serves the last-known data instantly (no re-fetch burst), and a match
  that finishes and drops out of the API window is retained until it ages out.
- Times are shown in the **viewer's own timezone** (auto-detected in the browser
  and sent to the server, which formats + groups by day accordingly). `DISPLAY_TZ`
  is the fallback used for the first server render / non-JS clients.
- Time format is toggleable **24h / 12h** (default 24h), remembered in
  `localStorage`. Each league/event gets a stable color, and can be filtered.
- **Scores are hidden by default** (spoiler guard). A top-right "show scores"
  toggle reveals all (persisted), or a per-event "show score" button reveals
  just one event — so it's not all-or-nothing.
- Each event header links to the **exact Liquipedia event page** when it can be
  resolved (looked up once per event via Liquipedia's search API and cached in
  SQLite, with validation + fallback so it never regresses to a worse link),
  else the official site or a Liquipedia search. Match rows link to the official
  stream.
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
2. Create `config.toml` from the example and add your token:
   ```sh
   cp config.toml.example config.toml
   # edit config.toml, set pandascore_token = "..."
   ```

## Develop

```sh
cargo leptos watch        # http://127.0.0.1:4000
DEMO=1 cargo leptos watch # force fixture data (ignores token + cache db)
cargo test --features ssr # tiering + deserialization tests
```

## Configuration (`config.toml`)

Settings live in `config.toml` (see `config.toml.example`). The path is
overridable with `CONFIG_PATH`, and any setting can be overridden by the
matching env var shown below — handy for `DEMO=1 cargo leptos watch` or
container `-e` flags.

| `config.toml` key | env override | Default | Purpose |
|---|---|---|---|
| `pandascore_token` | `PANDASCORE_TOKEN` | _(none)_ | API token; unset = demo data |
| `ocblacktop_token` | `OCBLACKTOP_TOKEN` | _(none)_ | [ocblacktop.com](https://ocblacktop.com/api) key for WRC + WEC; unset = those series skipped |
| `demo` | `DEMO` | `false` | `true` forces fixture data even with a token/DB |
| `display_tz` | `DISPLAY_TZ` | `America/Los_Angeles` | Fallback tz (viewers' own is auto-detected) |
| `idle_poll_secs` | `POLL_INTERVAL_SECS` | `1200` | Idle poll interval (min 60) |
| `active_poll_secs` | `POLL_ACTIVE_SECS` | `60` | Poll interval while live/imminent (min 30) |
| `upcoming_days` | `UPCOMING_DAYS` | `30` | Days ahead on the homepage (1–60) |
| `db_path` | `DB_PATH` | `data/cache.db` | SQLite cache path; empty = memory-only |
| `resolve_links` | `ENABLE_LIQUIPEDIA` | `true` | Resolve exact event pages via Liquipedia |
| `[vapid] public/private/subject` | `VAPID_*` | _(none)_ | Web Push reminder keys; all three enable reminders |

## Reminders (Web Push)

Star (★) any upcoming match to get a browser notification ~10 minutes before it
starts — delivered even if the site is closed, via a service worker + Web Push.

Enable it by generating a VAPID keypair once and putting it in the `[vapid]`
table of `config.toml`. Use the built-in generator (no extra tooling), or `npx`:

```sh
cargo run --example gen_vapid --features ssr   # prints keys for config.toml
# or: npx web-push generate-vapid-keys
```

How it works: the browser subscribes via `pushManager`; the subscription +
chosen matches are stored in SQLite (`DB_PATH` required); a background sender
delivers each reminder at its time (encrypted with `web-push-native`, pure Rust,
no OpenSSL) and prunes dead subscriptions. The ★ buttons only appear when push
is configured.

Requirements: **HTTPS in production** (Web Push needs a secure context), a
writable `DB_PATH`, and on **iOS** the site must be installed to the home screen
as a PWA before notifications work.

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
