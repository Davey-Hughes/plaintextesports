# Setup and deployment

Getting a token, the settings in `config.toml`, turning on reminders, and running
the Docker image.
Back to the [README](../README.md).

## Setup

1. Get a free PandaScore token (no card): https://pandascore.co
2. Create `config.toml` from the example and add your token:
   ```sh
   cp config.toml.example config.toml
   # edit config.toml, set pandascore_token = "..."
   ```

## Configuration (`config.toml`)

Settings live in `config.toml` (see `config.toml.example`). The path is
overridable with `CONFIG_PATH`, and any setting can be overridden by the
matching env var shown below — handy for `DEMO=1 cargo leptos watch` or
container `-e` flags.

| `config.toml` key | env override | Default | Purpose |
|---|---|---|---|
| `pandascore_token` | `PANDASCORE_TOKEN` | _(none)_ | API token; unset = demo data |
| `ocblacktop_token` | `OCBLACKTOP_TOKEN` | _(none)_ | [ocblacktop.com](https://ocblacktop.com/api) key for WRC + WEC + MotoGP; unset = those series skipped |
| `demo` | `DEMO` | `false` | `true` forces fixture data even with a token/DB |
| `display_tz` | `DISPLAY_TZ` | `America/Los_Angeles` | Fallback tz (viewers' own is auto-detected) |
| `idle_poll_secs` | `POLL_INTERVAL_SECS` | `1200` | Idle poll interval (min 60) |
| `active_poll_secs` | `POLL_ACTIVE_SECS` | `60` | Poll interval while live/imminent (min 30) |
| `upcoming_days` | `UPCOMING_DAYS` | `30` | Days ahead on the homepage (1–60) |
| `db_path` | `DB_PATH` | `data/cache.db` | SQLite cache path; empty = memory-only |
| `resolve_links` | `ENABLE_LIQUIPEDIA` | `true` | Resolve exact event pages via Liquipedia |
| `competetft_enabled` | `COMPETETFT_ENABLED` | `false` | The TFT source: Riot's competetft.com + official sheet (schedule, standings, prizes, player streams, broadcasts, lobbies) |
| `competetft_autodiscover` | `COMPETETFT_AUTODISCOVER` | `true` | Auto-discover TFT tournaments from competetft's schedule feed |
| `competetft_pins` / `competetft_exclude` | _(TOML only)_ | _(none)_ | Tournament ids to always include / never include |
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
docker run -p 8080:8080 -v /path/to/plaintextesports:/data plaintextesports
```

Serves on `:8080`.

Mount a single host directory at `/data` containing your `config.toml`,
and (optionally) an `icons/` subdirectory. The `data/` subdirectory (for
the SQLite cache) is created automatically. Without a token the app
serves demo fixture data.
