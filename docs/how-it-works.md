# How it works

How the server gets its data and keeps it fresh, how the tier-1 filter decides what
to show, and what the free data tier can't do.
Back to the [README](../README.md).

- A background task polls the [PandaScore](https://pandascore.co) API,
  normalizes matches, applies the tier-1 filter, and caches the result in
  memory. Page requests read the cache — they never block on the API.
- Polling is **adaptive**: schedules change slowly (and the free tier has no
  live feed), so it idles at ~20 min and bursts to ~1 min only while a match is
  live or starts within ~15 min, to catch final scores/status.
- A **live fast lane** covers NFL, NBA, NHL and MLB: while one of their games
  is live or starts within 5 min, a separate task polls just that day's
  scoreboard every `live_poll_secs` (default 10 s) and patches score, status and
  a period/clock label (`Q2 4:08`, `P1 INT`, `Bot 4th`) in memory. Open pages
  refetch every 15 s while a live game is on screen, so a live score trails the
  upstream feed by roughly 5–25 s. The label stays hidden with the score.
- Fetching is **depth-aware**: the idle cadence does a *deep scan*, paginating
  the upcoming feed across the whole `UPCOMING_DAYS` window so a tier-1 event is
  found even behind hundreds of low-tier matches; frequent active polls fetch
  only the first page. A rate-limited or failed later page returns a partial
  result and the SQLite cache fills the rest in on the next poll. All of this
  stays far under the free tier's 1,000 req/hr limit.
- Matches are also persisted to a small **SQLite** database (`DB_PATH`), keyed by
  `(id, sport)` and upserted each poll with a 2-day retention window. On restart
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

Edit the allowlist/denylist in [`src/tiering.rs`](../src/tiering.rs):

- **Base:** keep matches whose PandaScore tournament tier is `S` or `A`.
- **Denylist** (substring on any slug): force-exclude noise tagged S/A
  (academy/challenger/qualifier/showmatch…).
- **Allowlist** (exact full-slug): force-include events the API mis-tiers.
- Precedence: allowlist > denylist > base. Covered by unit tests.

## Limitations

The PandaScore free tier has no real-time score feed, so "live" is inferred
(start time passed, not yet finished) and scores update on the next poll, not
instantly. Recent results depend on what the free tier returns.
