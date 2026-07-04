//! Persistent SQLite cache for normalized matches (server-only).
//!
//! The in-memory snapshot stays the hot path for serving requests; this layer
//! only persists across restarts. It's touched at boot (load) and after each
//! poll (upsert + prune). Matches are keyed by `(id, sport)` and upserted, so a
//! match that finishes and drops out of the API's window is retained until it
//! ages past the retention cutoff.

use crate::feed::{NormalizedMatch, NormalizedTeam};
use crate::types::{MatchStatus, MotorResultRef, Sport};
use chrono::{DateTime, Utc};
use once_cell::sync::OnceCell;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashSet;
use std::path::Path;
use std::sync::{Mutex, PoisonError};

/// One week in ms — the ceiling for a reminder lead offset.
const WEEK_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// Grace window (ms) after a match's start during which a still-unsent reminder
/// may yet be delivered. A reminder is "due" only within `[notify_at, begin_at +
/// grace)` (with `begin_at = notify_at + lead`), so one whose window elapsed
/// while the server was down — e.g. a restart spanning the start of the match —
/// is skipped instead of fired hours late. Wide enough to ride out a normal
/// restart/deploy, short enough that a real outage suppresses a now-pointless
/// "starts soon" ping. A far-out timer (e.g. a 1-week lead) stays deliverable
/// almost until the match begins, so a brief delay never drops it. Also bounds
/// the "still worth a heads-up" window for a rescheduled-earlier match (see
/// [`crate::cache::reschedule_plan`]).
pub(crate) const DELIVER_GRACE_MS: i64 = 5 * 60 * 1000;

/// The most a reminder may lag its scheduled notify time and still be delivered.
/// Bounds the previous rule for *long* leads: a 1-day/1-week timer's
/// `begin_at + grace` window stays open for days, so a server that was off across
/// its notify time would otherwise fire it hours/days late. Past this cap the
/// delivery is dropped as stale. Comfortably longer than any restart/deploy, so
/// an on-time-armed reminder delivered a little late by a brief outage still goes.
const MAX_LATE_MS: i64 = 2 * 60 * 60 * 1000;

const FETCHED_AT_KEY: &str = "fetched_at_ms";

/// Process-wide write connection shared by the request handlers (reminder /
/// subscription server fns). Opened once on first use so each request doesn't
/// re-`open()` the DB and re-run the schema/migrations. The poller and push
/// sender keep their own long-lived connections (WAL allows the extra writer).
static SHARED: OnceCell<Mutex<Connection>> = OnceCell::new();

/// Borrow the shared request-handler connection, opening (and migrating) it once.
/// Recovers from a poisoned mutex since the connection itself stays valid.
pub fn shared(path: &str) -> rusqlite::Result<std::sync::MutexGuard<'static, Connection>> {
    let m = SHARED.get_or_try_init(|| open(path).map(Mutex::new))?;
    Ok(m.lock().unwrap_or_else(PoisonError::into_inner))
}

/// Open (creating if needed) the cache DB and ensure the schema exists.
pub fn open(path: &str) -> rusqlite::Result<Connection> {
    if let Some(parent) = Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(parent);
        }
    }
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    // The poller, push sender, and request handlers (see `shared`) each hold
    // their own connection; WAL + this busy timeout lets the writers coexist.
    conn.pragma_update(None, "busy_timeout", 5000)?;
    // `meta` first, so the migration flag below is readable.
    conn.execute(
        "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
        [],
    )?;
    // One-time: the reminders primary key gained `sport` (a match id isn't unique
    // across games/sources). Drop the old single-id-keyed table once so the
    // CREATE below rebuilds it with the (endpoint, match_id, sport) key. Reminders
    // are transient (re-armed when starred), so dropping them is safe.
    if get_meta(&conn, "reminders_pk_v2").is_none() {
        conn.execute("DROP TABLE IF EXISTS reminders", [])?;
        set_meta(&conn, "reminders_pk_v2", "1")?;
    }
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS matches (
            id           INTEGER NOT NULL,
            sport        TEXT    NOT NULL,
            league       TEXT    NOT NULL,
            league_url   TEXT,
            series_name  TEXT,
            tier         TEXT    NOT NULL,
            begin_at_ms  INTEGER NOT NULL,
            status       TEXT    NOT NULL,
            best_of      INTEGER,
            team_a_label TEXT    NOT NULL,
            team_a_score INTEGER,
            team_b_label TEXT    NOT NULL,
            team_b_score INTEGER,
            team_a_name  TEXT,
            team_b_name  TEXT,
            team_a_abbrev TEXT,
            team_b_abbrev TEXT,
            venue_tz     TEXT,
            venue_name   TEXT,
            venue_location TEXT,
            team_a_logo  TEXT,
            team_b_logo  TEXT,
            stream_url   TEXT,
            tournament_id INTEGER,
            -- Serialized MotorResultRef (`series|event_id|session_id`) for WRC/WEC/
            -- MotoGP rows, so a finished stage's results survive a restart without
            -- waiting for the next (infrequent) ocblacktop fetch. NULL otherwise.
            motor_ref    TEXT,
            PRIMARY KEY (id, sport)
        );
        CREATE INDEX IF NOT EXISTS idx_matches_begin ON matches (begin_at_ms);
        CREATE TABLE IF NOT EXISTS meta (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS event_links (
            key           TEXT PRIMARY KEY,
            url           TEXT,
            checked_at_ms INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS reminders (
            endpoint     TEXT    NOT NULL,
            p256dh       TEXT    NOT NULL,
            auth         TEXT    NOT NULL,
            match_id     INTEGER NOT NULL,
            -- Lead time (ms before start) this reminder fires at. Part of the key
            -- so a match can carry several timers (e.g. 1d / 1h / 15m before), one
            -- row each. Keyed on lead_ms (not notify_at_ms) so a rescheduled match
            -- updates the same timer row instead of spawning a duplicate.
            lead_ms      INTEGER NOT NULL DEFAULT 900000,
            notify_at_ms INTEGER NOT NULL,
            title        TEXT    NOT NULL,
            body         TEXT    NOT NULL,
            url          TEXT    NOT NULL,
            sport        TEXT    NOT NULL DEFAULT '',
            league       TEXT    NOT NULL DEFAULT '',
            team_a       TEXT    NOT NULL DEFAULT '',
            team_b       TEXT    NOT NULL DEFAULT '',
            -- Full event name (league + edition, e.g. F1 Austrian Grand Prix), so
            -- an event subscription's reminders can be cleaned up on unsub.
            event        TEXT    NOT NULL DEFAULT '',
            -- Subscriber's zone + clock format, so the sender can re-render this
            -- reminder's start-time body when the match is rescheduled.
            tz           TEXT    NOT NULL DEFAULT '',
            hour24       INTEGER NOT NULL DEFAULT 0,
            sent         INTEGER NOT NULL DEFAULT 0,
            -- Keyed by (match_id, sport, lead_ms): an id isn't unique across games,
            -- and a match holds one row per lead time.
            PRIMARY KEY (endpoint, match_id, sport, lead_ms)
        );
        -- A per-match opt-out of a covering subscription (replaces the old
        -- excluded=1 tombstone row): a single row blocks every timer for the
        -- match. Expansion skips matches listed here; an explicit star clears it.
        CREATE TABLE IF NOT EXISTS exclusions (
            endpoint  TEXT    NOT NULL,
            match_id  INTEGER NOT NULL,
            sport     TEXT    NOT NULL DEFAULT '',
            PRIMARY KEY (endpoint, match_id, sport)
        );
        CREATE TABLE IF NOT EXISTS subscriptions (
            endpoint    TEXT    NOT NULL,
            p256dh      TEXT    NOT NULL,
            auth        TEXT    NOT NULL,
            scope_kind  TEXT    NOT NULL,
            scope_value TEXT    NOT NULL,
            -- Legacy single lead, kept for back-compat; lead_list is authoritative.
            lead_ms     INTEGER NOT NULL,
            -- Comma-separated lead offsets (ms) this scope arms; '' ⇒ [lead_ms].
            lead_list   TEXT    NOT NULL DEFAULT '',
            -- Subscriber's IANA tz, for formatting reminder bodies; '' ⇒ server tz.
            tz          TEXT    NOT NULL DEFAULT '',
            -- Subscriber's 12h/24h pref, for formatting reminder bodies; 0 ⇒ 12h.
            hour24      INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (endpoint, scope_kind, scope_value)
        );
        CREATE TABLE IF NOT EXISTS result_cache (
            ns            TEXT    NOT NULL,
            key           TEXT    NOT NULL,
            value         TEXT    NOT NULL,
            fetched_at_ms INTEGER NOT NULL,
            PRIMARY KEY (ns, key)
        );",
    )?;
    // Column renames game->sport and serie_name->series_name (the data model is
    // `Sport`, and "series" is the English spelling). Upgrade existing DBs in
    // place; each is a no-op (error ignored) on a DB already on the new names.
    // Must precede the ADD COLUMN block so an old `serie_name` is renamed rather
    // than left alongside a freshly-added `series_name`.
    let _ = conn.execute("ALTER TABLE matches RENAME COLUMN game TO sport", []);
    let _ = conn.execute(
        "ALTER TABLE matches RENAME COLUMN serie_name TO series_name",
        [],
    );
    let _ = conn.execute("ALTER TABLE reminders RENAME COLUMN game TO sport", []);
    // Add columns missing from older DBs (ignored if the column already exists).
    let _ = conn.execute("ALTER TABLE matches ADD COLUMN league_url TEXT", []);
    let _ = conn.execute("ALTER TABLE matches ADD COLUMN tournament_id INTEGER", []);
    let _ = conn.execute("ALTER TABLE matches ADD COLUMN series_name TEXT", []);
    let _ = conn.execute("ALTER TABLE matches ADD COLUMN team_a_name TEXT", []);
    let _ = conn.execute("ALTER TABLE matches ADD COLUMN team_b_name TEXT", []);
    let _ = conn.execute("ALTER TABLE matches ADD COLUMN team_a_abbrev TEXT", []);
    let _ = conn.execute("ALTER TABLE matches ADD COLUMN team_b_abbrev TEXT", []);
    let _ = conn.execute("ALTER TABLE matches ADD COLUMN venue_tz TEXT", []);
    let _ = conn.execute("ALTER TABLE matches ADD COLUMN venue_name TEXT", []);
    let _ = conn.execute("ALTER TABLE matches ADD COLUMN venue_location TEXT", []);
    let _ = conn.execute("ALTER TABLE matches ADD COLUMN team_a_logo TEXT", []);
    let _ = conn.execute("ALTER TABLE matches ADD COLUMN team_b_logo TEXT", []);
    let _ = conn.execute("ALTER TABLE matches ADD COLUMN motor_ref TEXT", []);
    let _ = conn.execute(
        "ALTER TABLE reminders ADD COLUMN sport TEXT NOT NULL DEFAULT ''",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE reminders ADD COLUMN league TEXT NOT NULL DEFAULT ''",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE reminders ADD COLUMN team_a TEXT NOT NULL DEFAULT ''",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE reminders ADD COLUMN team_b TEXT NOT NULL DEFAULT ''",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE reminders ADD COLUMN event TEXT NOT NULL DEFAULT ''",
        [],
    );
    // Existing subscriptions predate the per-scope timer list.
    let _ = conn.execute(
        "ALTER TABLE subscriptions ADD COLUMN lead_list TEXT NOT NULL DEFAULT ''",
        [],
    );
    // …and the per-subscriber timezone (older rows fall back to the server tz).
    let _ = conn.execute(
        "ALTER TABLE subscriptions ADD COLUMN tz TEXT NOT NULL DEFAULT ''",
        [],
    );
    // …and the per-subscriber 12h/24h pref (older rows fall back to 12-hour).
    let _ = conn.execute(
        "ALTER TABLE subscriptions ADD COLUMN hour24 INTEGER NOT NULL DEFAULT 0",
        [],
    );

    // One-time: the reminders PK gained `lead_ms` so a match can hold several
    // timers (one row per lead time). SQLite can't alter a PK in place and
    // `CREATE TABLE IF NOT EXISTS` won't reshape an existing table, so recreate
    // it — copying live rows (the legacy single timer ⇒ the 15m default lead) and
    // moving old excluded=1 tombstones into the new `exclusions` table. Gated on a
    // meta flag and the presence of the now-dropped `excluded` column, so it runs
    // exactly once and is a no-op on a fresh (already-v3) DB.
    if get_meta(&conn, "reminders_pk_v3").is_none() {
        if reminders_has_excluded_column(&conn) {
            conn.execute_batch(
                "DROP TABLE IF EXISTS reminders_v3_new;
                 CREATE TABLE reminders_v3_new (
                    endpoint     TEXT    NOT NULL,
                    p256dh       TEXT    NOT NULL,
                    auth         TEXT    NOT NULL,
                    match_id     INTEGER NOT NULL,
                    lead_ms      INTEGER NOT NULL DEFAULT 900000,
                    notify_at_ms INTEGER NOT NULL,
                    title        TEXT    NOT NULL,
                    body         TEXT    NOT NULL,
                    url          TEXT    NOT NULL,
                    sport        TEXT    NOT NULL DEFAULT '',
                    league       TEXT    NOT NULL DEFAULT '',
                    team_a       TEXT    NOT NULL DEFAULT '',
                    team_b       TEXT    NOT NULL DEFAULT '',
                    event        TEXT    NOT NULL DEFAULT '',
                    sent         INTEGER NOT NULL DEFAULT 0,
                    PRIMARY KEY (endpoint, match_id, sport, lead_ms)
                 );
                 INSERT OR IGNORE INTO reminders_v3_new
                    (endpoint, p256dh, auth, match_id, lead_ms, notify_at_ms,
                     title, body, url, sport, league, team_a, team_b, event, sent)
                 SELECT endpoint, p256dh, auth, match_id, 900000, notify_at_ms,
                     title, body, url, sport, league, team_a, team_b, event, sent
                 FROM reminders WHERE excluded = 0;
                 INSERT OR IGNORE INTO exclusions (endpoint, match_id, sport)
                 SELECT endpoint, match_id, sport FROM reminders WHERE excluded = 1;
                 DROP TABLE reminders;
                 ALTER TABLE reminders_v3_new RENAME TO reminders;",
            )?;
        }
        set_meta(&conn, "reminders_pk_v3", "1")?;
    }

    // The per-reminder zone + clock format, so the sender can re-render a
    // rescheduled match's start-time body. Added *after* the v3 rebuild above,
    // which recreates `reminders` from a fixed column list and would otherwise
    // drop columns added by an earlier ALTER; here they land on the final shape
    // (older rows fall back to the server tz / 12-hour). Idempotent.
    let _ = conn.execute(
        "ALTER TABLE reminders ADD COLUMN tz TEXT NOT NULL DEFAULT ''",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE reminders ADD COLUMN hour24 INTEGER NOT NULL DEFAULT 0",
        [],
    );

    purge_unknown_sports(&conn)?;
    Ok(conn)
}

/// Whether the `reminders` table still has the pre-v3 `excluded` column (i.e. it
/// predates the multi-timer PK and needs the recreate migration).
fn reminders_has_excluded_column(conn: &Connection) -> bool {
    conn.prepare("SELECT 1 FROM pragma_table_info('reminders') WHERE name = 'excluded'")
        .and_then(|mut s| s.exists([]))
        .unwrap_or(false)
}

/// Delete match rows whose stored slug isn't a current [`Sport`] — orphans left
/// behind when a sport's slug is renamed (e.g. soccer's "soccer" → "football").
/// Otherwise such rows load as the default sport and surface on the wrong page.
/// Cheap and idempotent, so it runs on every open.
fn purge_unknown_sports(conn: &Connection) -> rusqlite::Result<()> {
    let slugs: Vec<&str> = Sport::ALL.iter().map(|g| g.slug()).collect();
    let placeholders = vec!["?"; slugs.len()].join(",");
    conn.execute(
        &format!("DELETE FROM matches WHERE sport NOT IN ({placeholders})"),
        rusqlite::params_from_iter(slugs),
    )?;
    Ok(())
}

/// The team's full name column, falling back to its short label for rows
/// written before the name columns existed (or any left NULL).
fn team_name(row: &rusqlite::Row, name_col: &str, label_col: &str) -> rusqlite::Result<String> {
    let name: Option<String> = row.get(name_col)?;
    name.filter(|s| !s.is_empty())
        .map_or_else(|| row.get::<_, String>(label_col), Ok)
}

fn row_to_match(row: &rusqlite::Row) -> rusqlite::Result<Option<NormalizedMatch>> {
    // The `sport` column holds the slug, not the enum.
    let slug: String = row.get("sport")?;
    // Drop rows whose slug no longer maps to a `Sport` (e.g. a row written under a
    // since-renamed slug). Defaulting an unknown sport to Cs2 would mislabel it
    // and surface it on the wrong page, so skip it instead.
    let Some(sport) = Sport::from_filter(&slug) else {
        return Ok(None);
    };
    let begin_ms: i64 = row.get("begin_at_ms")?;
    let status: String = row.get("status")?;
    Ok(Some(NormalizedMatch {
        id: row.get("id")?,
        sport,
        league: row.get("league")?,
        league_url: row.get("league_url")?,
        series_name: row
            .get::<_, Option<String>>("series_name")?
            .unwrap_or_default(),
        tier: row.get("tier")?,
        begin_at: DateTime::from_timestamp_millis(begin_ms).unwrap_or_else(Utc::now),
        status: MatchStatus::from_db(&status),
        best_of: row.get("best_of")?,
        team_a: NormalizedTeam {
            label: row.get("team_a_label")?,
            // Older rows predate team_a_name; fall back to the label.
            name: team_name(row, "team_a_name", "team_a_label")?,
            abbrev: row
                .get::<_, Option<String>>("team_a_abbrev")?
                .unwrap_or_default(),
            score: row.get("team_a_score")?,
        },
        team_b: NormalizedTeam {
            label: row.get("team_b_label")?,
            name: team_name(row, "team_b_name", "team_b_label")?,
            abbrev: row
                .get::<_, Option<String>>("team_b_abbrev")?
                .unwrap_or_default(),
            score: row.get("team_b_score")?,
        },
        stream_url: row.get("stream_url")?,
        tournament_id: row.get("tournament_id")?,
        venue_tz: row.get("venue_tz")?,
        venue_name: row
            .get::<_, Option<String>>("venue_name")?
            .unwrap_or_default(),
        venue_location: row
            .get::<_, Option<String>>("venue_location")?
            .unwrap_or_default(),
        team_a_logo: row
            .get::<_, Option<String>>("team_a_logo")?
            .unwrap_or_default(),
        team_b_logo: row
            .get::<_, Option<String>>("team_b_logo")?
            .unwrap_or_default(),
        // Streams aren't persisted (in-memory only); repopulated on next poll.
        streams: Vec::new(),
        // Series refs aren't persisted (MLB, in-memory); repopulated on next poll.
        mlb_series: None,
        // The motorsport result ref IS persisted (the ocblacktop fetch that sets it
        // is too infrequent to rely on re-attaching it each poll — a restart would
        // blank a finished stage's results until the next fetch). Parsed back here.
        motor_result_ref: row
            .get::<_, Option<String>>("motor_ref")?
            .as_deref()
            .and_then(MotorResultRef::from_db),
    }))
}

/// Load all stored matches, ordered by start time. Rows with an unrecognized
/// sport slug are skipped (see [`row_to_match`]).
pub fn load_all(conn: &Connection) -> rusqlite::Result<Vec<NormalizedMatch>> {
    let mut stmt = conn.prepare("SELECT * FROM matches ORDER BY begin_at_ms ASC")?;
    let rows = stmt.query_map([], row_to_match)?;
    rows.filter_map(Result::transpose).collect()
}

/// Read the stored "last fetched" timestamp (unix ms), if any.
pub fn load_fetched_at(conn: &Connection) -> Option<i64> {
    get_meta(conn, FETCHED_AT_KEY).and_then(|s| s.parse().ok())
}

/// Read an arbitrary `meta` value by key. Used for the backfill cursor and the
/// per-day past-refresh timestamps.
#[must_use]
pub fn get_meta(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
        r.get::<_, String>(0)
    })
    .ok()
}

/// Upsert an arbitrary `meta` key/value.
pub fn set_meta(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// Upsert freshly polled matches, prune anything older than `cutoff_ms`, and
/// record `fetched_at_ms` — all in one transaction.
pub fn upsert_and_prune(
    conn: &mut Connection,
    matches: &[NormalizedMatch],
    fetched_at_ms: i64,
    cutoff_ms: i64,
    replace_leagues: &[(&str, Vec<i64>)],
) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    {
        let mut up = tx.prepare(
            "INSERT INTO matches
                (id, sport, league, tier, begin_at_ms, status, best_of,
                 team_a_label, team_a_score, team_b_label, team_b_score, stream_url,
                 league_url, tournament_id, series_name, team_a_name, team_b_name, venue_tz,
                 venue_name, venue_location, team_a_logo, team_b_logo,
                 team_a_abbrev, team_b_abbrev, motor_ref)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)
             ON CONFLICT(id, sport) DO UPDATE SET
                league=excluded.league, tier=excluded.tier,
                begin_at_ms=excluded.begin_at_ms, status=excluded.status,
                best_of=excluded.best_of,
                team_a_label=excluded.team_a_label, team_a_score=excluded.team_a_score,
                team_b_label=excluded.team_b_label, team_b_score=excluded.team_b_score,
                stream_url=excluded.stream_url, league_url=excluded.league_url,
                tournament_id=excluded.tournament_id, series_name=excluded.series_name,
                team_a_name=excluded.team_a_name, team_b_name=excluded.team_b_name,
                venue_tz=excluded.venue_tz, venue_name=excluded.venue_name,
                venue_location=excluded.venue_location,
                team_a_logo=excluded.team_a_logo, team_b_logo=excluded.team_b_logo,
                team_a_abbrev=excluded.team_a_abbrev, team_b_abbrev=excluded.team_b_abbrev,
                motor_ref=excluded.motor_ref",
        )?;
        for m in matches {
            up.execute(params![
                m.id,
                m.sport.slug(),
                m.league,
                m.tier,
                m.begin_at.timestamp_millis(),
                m.status.as_str(),
                m.best_of,
                m.team_a.label,
                m.team_a.score,
                m.team_b.label,
                m.team_b.score,
                m.stream_url,
                m.league_url,
                m.tournament_id,
                m.series_name,
                m.team_a.name,
                m.team_b.name,
                m.venue_tz,
                m.venue_name,
                m.venue_location,
                m.team_a_logo,
                m.team_b_logo,
                m.team_a.abbrev,
                m.team_b.abbrev,
                m.motor_result_ref.as_ref().map(MotorResultRef::to_db),
            ])?;
        }
        // Fully-enumerated feeds (the WRC season feed returns every rally each
        // poll) are league-scoped replaced: drop any row of the league whose id
        // isn't in this poll's set, so a placeholder superseded by its stages — or
        // stale stages of a rally no longer detailed — doesn't linger the way a
        // dropped-from-window esports match is deliberately retained. Skipped when
        // the league brought back nothing this poll (e.g. its fetch errored), so a
        // transient failure never wipes the league.
        for (league, keep) in replace_leagues {
            if keep.is_empty() {
                continue;
            }
            let holes = std::iter::repeat_n("?", keep.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!("DELETE FROM matches WHERE league = ?1 AND id NOT IN ({holes})");
            let mut vals: Vec<rusqlite::types::Value> = Vec::with_capacity(keep.len() + 1);
            vals.push(rusqlite::types::Value::Text((*league).to_string()));
            vals.extend(keep.iter().map(|&id| rusqlite::types::Value::Integer(id)));
            tx.execute(&sql, rusqlite::params_from_iter(vals))?;
        }
        tx.execute(
            "DELETE FROM matches WHERE begin_at_ms < ?1",
            params![cutoff_ms],
        )?;
        tx.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![FETCHED_AT_KEY, fetched_at_ms.to_string()],
        )?;
    }
    tx.commit()
}

/// Load all cached event-link resolutions: `(key, url, checked_at_ms)` where a
/// `None` url means "resolved, no confident match".
pub fn load_event_links(conn: &Connection) -> rusqlite::Result<Vec<(String, Option<String>, i64)>> {
    let mut stmt = conn.prepare("SELECT key, url, checked_at_ms FROM event_links")?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    rows.collect()
}

/// Upsert one event-link resolution.
pub fn set_event_link(
    conn: &Connection,
    key: &str,
    url: Option<&str>,
    checked_at_ms: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO event_links (key, url, checked_at_ms) VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET url=excluded.url, checked_at_ms=excluded.checked_at_ms",
        params![key, url, checked_at_ms],
    )?;
    Ok(())
}

/// One row of the generic `result_cache` (JSON `value` + when it was fetched).
pub struct CacheEntry {
    pub value: String,
    pub fetched_at_ms: i64,
}

/// Read one cached payload by `(ns, key)`. `None` when absent.
pub fn cache_get(conn: &Connection, ns: &str, key: &str) -> rusqlite::Result<Option<CacheEntry>> {
    conn.query_row(
        "SELECT value, fetched_at_ms FROM result_cache WHERE ns = ?1 AND key = ?2",
        rusqlite::params![ns, key],
        |row| {
            Ok(CacheEntry {
                value: row.get(0)?,
                fetched_at_ms: row.get(1)?,
            })
        },
    )
    .optional()
}

/// Every row of a namespace, ordered by key (for Pattern B startup load).
pub fn cache_get_ns(conn: &Connection, ns: &str) -> rusqlite::Result<Vec<(String, CacheEntry)>> {
    let mut stmt = conn
        .prepare("SELECT key, value, fetched_at_ms FROM result_cache WHERE ns = ?1 ORDER BY key")?;
    let rows = stmt
        .query_map(rusqlite::params![ns], |row| {
            Ok((
                row.get::<_, String>(0)?,
                CacheEntry {
                    value: row.get(1)?,
                    fetched_at_ms: row.get(2)?,
                },
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Upsert one payload (replace on the `(ns, key)` primary key).
pub fn cache_put(
    conn: &Connection,
    ns: &str,
    key: &str,
    value: &str,
    fetched_at_ms: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO result_cache (ns, key, value, fetched_at_ms) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(ns, key) DO UPDATE SET value = excluded.value, fetched_at_ms = excluded.fetched_at_ms",
        rusqlite::params![ns, key, value, fetched_at_ms],
    )?;
    Ok(())
}

/// A stored Web Push reminder (one timer for one match).
#[derive(Debug, Clone)]
pub struct Reminder {
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
    pub match_id: i64,
    /// Lead time (ms before start) this reminder fires at; part of the key.
    pub lead_ms: i64,
    pub notify_at_ms: i64,
    pub title: String,
    pub body: String,
    pub url: String,
    /// Sport/league/teams/event the match belongs to (for scope-based cleanup).
    pub sport: String,
    pub league: String,
    pub team_a: String,
    pub team_b: String,
    /// Full event name (e.g. F1 Austrian Grand Prix), for event-scope cleanup.
    pub event: String,
    /// Subscriber's IANA zone + 12h/24h pref, stored so the sender can re-render
    /// this reminder's start-time body when the match is rescheduled (see
    /// [`crate::cache::reschedule_plan`]). Empty tz ⇒ the server default.
    pub tz: String,
    pub hour24: bool,
    /// Whether this timer was already delivered. Not part of the write path (the
    /// insert defaults it to 0 and [`mark_reminder_sent`] flips it); it's read back
    /// by [`all_reminders`] so the reconcile can notify on a cancellation even
    /// after every timer for the match has fired.
    pub sent: bool,
}

const REMINDER_COLS: &str = "endpoint, p256dh, auth, match_id, lead_ms, notify_at_ms, title, \
     body, url, sport, league, team_a, team_b, event, tz, hour24";

fn reminder_params(r: &Reminder) -> [&dyn rusqlite::ToSql; 16] {
    [
        &r.endpoint,
        &r.p256dh,
        &r.auth,
        &r.match_id,
        &r.lead_ms,
        &r.notify_at_ms,
        &r.title,
        &r.body,
        &r.url,
        &r.sport,
        &r.league,
        &r.team_a,
        &r.team_b,
        &r.event,
        &r.tz,
        &r.hour24,
    ]
}

/// `INSERT … ON CONFLICT … DO UPDATE` upsert for a single timer row (matched by
/// the full `(endpoint, match_id, sport, lead_ms)` key), re-arming `sent`.
fn reminder_upsert_sql() -> String {
    format!(
        "INSERT INTO reminders ({REMINDER_COLS}, sent)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, 0)
         ON CONFLICT(endpoint, match_id, sport, lead_ms) DO UPDATE SET
            p256dh=excluded.p256dh, auth=excluded.auth,
            notify_at_ms=excluded.notify_at_ms, title=excluded.title,
            body=excluded.body, url=excluded.url,
            league=excluded.league, team_a=excluded.team_a,
            team_b=excluded.team_b, event=excluded.event,
            tz=excluded.tz, hour24=excluded.hour24, sent=0"
    )
}

/// Insert a timer row only if one doesn't already exist for that key.
fn reminder_insert_if_absent_sql() -> String {
    format!(
        "INSERT INTO reminders ({REMINDER_COLS}, sent)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, 0)
         ON CONFLICT(endpoint, match_id, sport, lead_ms) DO NOTHING"
    )
}

/// Arm or re-arm a single timer row, clearing any opt-out for its match. Used to
/// re-include a match opted out of a scope (and by the store tests);
/// [`set_match_reminders`] is the multi-timer entry point for an explicit star.
pub fn add_reminder(conn: &Connection, r: &Reminder) -> rusqlite::Result<()> {
    conn.execute(&reminder_upsert_sql(), &reminder_params(r)[..])?;
    clear_exclusion(conn, &r.endpoint, r.match_id, &r.sport)?;
    Ok(())
}

/// Replace every timer for one starred match with exactly `reminders` (all
/// sharing endpoint/match/sport, one per lead offset). Clears any opt-out and
/// drops stale *unsent* rows first, so changing a match's timer set leaves no
/// orphans and never re-fires a timer that already went out. Atomic; a no-op on
/// an empty slice.
pub fn set_match_reminders(conn: &Connection, reminders: &[Reminder]) -> rusqlite::Result<()> {
    let Some(first) = reminders.first() else {
        return Ok(());
    };
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM exclusions WHERE endpoint=?1 AND match_id=?2 AND sport=?3",
        params![first.endpoint, first.match_id, first.sport],
    )?;
    tx.execute(
        "DELETE FROM reminders WHERE endpoint=?1 AND match_id=?2 AND sport=?3 AND sent=0",
        params![first.endpoint, first.match_id, first.sport],
    )?;
    // Insert-if-absent so an already-sent timer for one of these offsets stays
    // sent (the stale unsent ones were just cleared, so this re-arms the rest).
    let sql = reminder_insert_if_absent_sql();
    for r in reminders {
        tx.execute(&sql, &reminder_params(r)[..])?;
    }
    tx.commit()
}

/// Insert a timer row only if one doesn't exist (so re-expanding a sport/event
/// subscription never re-arms an already-sent reminder).
pub fn add_reminder_if_absent(conn: &Connection, r: &Reminder) -> rusqlite::Result<()> {
    conn.execute(&reminder_insert_if_absent_sql(), &reminder_params(r)[..])?;
    Ok(())
}

/// Opt a match out of all covering subscriptions: a single row blocks every timer
/// for `(endpoint, match_id, sport)`. Also drops its unsent reminder rows so the
/// opt-out takes effect immediately.
pub fn exclude_match(
    conn: &Connection,
    endpoint: &str,
    match_id: i64,
    sport: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO exclusions (endpoint, match_id, sport) VALUES (?1, ?2, ?3)",
        params![endpoint, match_id, sport],
    )?;
    conn.execute(
        "DELETE FROM reminders WHERE endpoint=?1 AND match_id=?2 AND sport=?3 AND sent=0",
        params![endpoint, match_id, sport],
    )?;
    Ok(())
}

/// Clear a match's opt-out (an explicit star wins over a prior opt-out).
pub fn clear_exclusion(
    conn: &Connection,
    endpoint: &str,
    match_id: i64,
    sport: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM exclusions WHERE endpoint=?1 AND match_id=?2 AND sport=?3",
        params![endpoint, match_id, sport],
    )?;
    Ok(())
}

/// Every opt-out as `(endpoint, match_id, sport)` — the expansion loop loads this
/// once per tick to skip re-arming opted-out matches.
pub fn list_exclusions(conn: &Connection) -> rusqlite::Result<HashSet<(String, i64, String)>> {
    let mut stmt = conn.prepare("SELECT endpoint, match_id, sport FROM exclusions")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    rows.collect()
}

pub fn remove_reminder(
    conn: &Connection,
    endpoint: &str,
    match_id: i64,
    sport: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM reminders WHERE endpoint = ?1 AND match_id = ?2 AND sport = ?3",
        params![endpoint, match_id, sport],
    )?;
    Ok(())
}

/// Unsent reminders whose notify time has arrived and that haven't gone stale. A
/// reminder is "due" only when, with `begin_at = notify_at + lead_ms`:
///   `notify_at <= now < begin_at + grace`   (not past the match start) **and**
///   `now < notify_at + MAX_LATE`            (not delivered far behind schedule).
/// So a reminder that came due while the server was down fires only if the match
/// hasn't started (no pointless late "starts soon") *and* it isn't badly behind
/// (a long-lead timer the server missed by hours is dropped, not fired late).
/// Carries `sport` + `lead_ms` so the sender can mark the exact timer row sent.
pub fn due_reminders(conn: &Connection, now_ms: i64) -> rusqlite::Result<Vec<Reminder>> {
    // The bounds are trusted i64 consts, so inlining them is injection-safe.
    let mut stmt = conn.prepare(&format!(
        "SELECT endpoint, p256dh, auth, match_id, lead_ms, notify_at_ms, title, body, url, sport
         FROM reminders
         WHERE sent = 0 AND notify_at_ms <= ?1
           AND ?1 < notify_at_ms + lead_ms + {DELIVER_GRACE_MS}
           AND ?1 < notify_at_ms + {MAX_LATE_MS}
         ORDER BY notify_at_ms",
    ))?;
    let rows = stmt.query_map([now_ms], |r| {
        Ok(Reminder {
            endpoint: r.get(0)?,
            p256dh: r.get(1)?,
            auth: r.get(2)?,
            match_id: r.get(3)?,
            lead_ms: r.get(4)?,
            notify_at_ms: r.get(5)?,
            title: r.get(6)?,
            body: r.get(7)?,
            url: r.get(8)?,
            sport: r.get(9)?,
            // Not needed for sending.
            league: String::new(),
            team_a: String::new(),
            team_b: String::new(),
            event: String::new(),
            tz: String::new(),
            hour24: false,
            sent: false,
        })
    })?;
    rows.collect()
}

/// Mark exactly one timer row sent (keyed by the full PK, so delivering one
/// timer never silences a match's other timers — or the same id under another
/// sport).
pub fn mark_reminder_sent(
    conn: &Connection,
    endpoint: &str,
    match_id: i64,
    sport: &str,
    lead_ms: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE reminders SET sent = 1
         WHERE endpoint = ?1 AND match_id = ?2 AND sport = ?3 AND lead_ms = ?4",
        params![endpoint, match_id, sport, lead_ms],
    )?;
    Ok(())
}

/// Every reminder in full, sent and unsent — the sender loads these each tick to
/// reconcile against the latest match start times (see
/// [`crate::cache::reschedule_plan`]). Sent rows are included so a cancellation is
/// still announced after every timer for the match has already fired; the plan
/// only *reschedules* the unsent ones.
pub fn all_reminders(conn: &Connection) -> rusqlite::Result<Vec<Reminder>> {
    let mut stmt = conn.prepare(&format!("SELECT {REMINDER_COLS}, sent FROM reminders"))?;
    let rows = stmt.query_map([], |r| {
        Ok(Reminder {
            endpoint: r.get(0)?,
            p256dh: r.get(1)?,
            auth: r.get(2)?,
            match_id: r.get(3)?,
            lead_ms: r.get(4)?,
            notify_at_ms: r.get(5)?,
            title: r.get(6)?,
            body: r.get(7)?,
            url: r.get(8)?,
            sport: r.get(9)?,
            league: r.get(10)?,
            team_a: r.get(11)?,
            team_b: r.get(12)?,
            event: r.get(13)?,
            tz: r.get(14)?,
            hour24: r.get(15)?,
            sent: r.get::<_, i64>(16)? != 0,
        })
    })?;
    rows.collect()
}

/// Re-point an unsent timer row at a rescheduled start: update its notify time
/// and the derived title/body/url, keyed by the full PK. Touches only unsent rows
/// — a delivered reminder is never rewritten, and its `sent` latch is preserved.
pub fn update_reminder_schedule(conn: &Connection, r: &Reminder) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE reminders SET notify_at_ms=?1, title=?2, body=?3, url=?4
         WHERE endpoint=?5 AND match_id=?6 AND sport=?7 AND lead_ms=?8 AND sent=0",
        params![
            r.notify_at_ms,
            r.title,
            r.body,
            r.url,
            r.endpoint,
            r.match_id,
            r.sport,
            r.lead_ms
        ],
    )?;
    Ok(())
}

/// Remove all reminders, subscriptions, and opt-outs for a dead endpoint
/// (404/410 push).
pub fn delete_endpoint(conn: &Connection, endpoint: &str) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM reminders WHERE endpoint = ?1",
        params![endpoint],
    )?;
    conn.execute(
        "DELETE FROM subscriptions WHERE endpoint = ?1",
        params![endpoint],
    )?;
    conn.execute(
        "DELETE FROM exclusions WHERE endpoint = ?1",
        params![endpoint],
    )?;
    Ok(())
}

/// Move every row from a rotated push subscription to its replacement, updating
/// the endpoint (the row key) plus the encryption keys. Backs the service
/// worker's `pushsubscriptionchange` handler, so pending reminders survive the
/// browser rotating the subscription rather than waiting for the next visit to
/// re-arm. A no-op if old == new. `OR IGNORE` so a row that already exists under
/// the new endpoint (e.g. a reconcile already ran) is left as-is — the stale old
/// one is dropped below and would otherwise 410-prune anyway.
pub fn migrate_endpoint(
    conn: &Connection,
    old: &str,
    new: &crate::types::PushSub,
) -> rusqlite::Result<()> {
    if old == new.endpoint {
        return Ok(());
    }
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "UPDATE OR IGNORE reminders SET endpoint=?1, p256dh=?2, auth=?3 WHERE endpoint=?4",
        params![new.endpoint, new.p256dh, new.auth, old],
    )?;
    tx.execute(
        "UPDATE OR IGNORE subscriptions SET endpoint=?1, p256dh=?2, auth=?3 WHERE endpoint=?4",
        params![new.endpoint, new.p256dh, new.auth, old],
    )?;
    tx.execute(
        "UPDATE OR IGNORE exclusions SET endpoint=?1 WHERE endpoint=?2",
        params![new.endpoint, old],
    )?;
    // Drop anything the OR IGNORE skipped (a collision with an existing new-row).
    tx.execute("DELETE FROM reminders WHERE endpoint=?1", params![old])?;
    tx.execute("DELETE FROM subscriptions WHERE endpoint=?1", params![old])?;
    tx.execute("DELETE FROM exclusions WHERE endpoint=?1", params![old])?;
    tx.commit()
}

/// Drop reminders whose match started well in the past (`begin_at = notify_at +
/// lead_ms < cutoff`). Keyed on the start, not the notify time, so a *sent*
/// reminder for a still-upcoming match (a long lead that already fired) is
/// retained — otherwise pruning it would let the subscription-expansion's
/// insert-if-absent re-arm it and fire a duplicate. Cleaned up once the match is
/// safely in the past.
pub fn prune_reminders(conn: &Connection, cutoff_ms: i64) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM reminders WHERE notify_at_ms + lead_ms < ?1",
        params![cutoff_ms],
    )?;
    Ok(())
}

// ----- Sport/event subscriptions --------------------------------------------

/// A subscription to a whole sport or event (expanded into reminders).
#[derive(Debug, Clone)]
pub struct Subscription {
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
    /// "sport" or "league" (also "team"/"event").
    pub scope_kind: String,
    /// "cs2"/"lol" for a sport, else the league name.
    pub scope_value: String,
    /// Legacy single lead, kept for back-compat reads.
    pub lead_ms: i64,
    /// The lead offsets (ms) this scope arms — one reminder per offset, per match.
    pub lead_list: Vec<i64>,
    /// The subscriber's IANA timezone, so expansion bakes each reminder body's
    /// start time in their zone. Empty ⇒ the server's display tz.
    pub tz: String,
    /// The subscriber's 12h/24h preference, so expansion formats each reminder
    /// body's start time to match. `false` ⇒ 12-hour.
    pub hour24: bool,
}

/// Serialize a lead-offset list for the `lead_list` column.
fn join_lead_list(v: &[i64]) -> String {
    v.iter().map(i64::to_string).collect::<Vec<_>>().join(",")
}

/// Parse a `lead_list` column, clamping to `0..=1 week`, sorting + deduping. An
/// empty/blank column (rows written by the pre-list code) falls back to
/// `[fallback]` (the legacy single `lead_ms`).
fn parse_lead_list(s: &str, fallback: i64) -> Vec<i64> {
    let mut v: Vec<i64> = s
        .split(',')
        .filter_map(|p| p.trim().parse::<i64>().ok())
        .filter(|&ms| (0..=WEEK_MS).contains(&ms))
        .collect();
    v.sort_unstable();
    v.dedup();
    if v.is_empty() {
        vec![fallback]
    } else {
        v
    }
}

/// The reminders-table WHERE clause matching a subscription scope (`?2` = value).
fn scope_where(kind: &str) -> &'static str {
    match kind {
        "sport" => "sport=?2",
        "team" => "(team_a=?2 OR team_b=?2)",
        "event" => "event=?2",
        _ => "league=?2",
    }
}

pub fn add_subscription(conn: &Connection, s: &Subscription) -> rusqlite::Result<()> {
    let lead_list = join_lead_list(&s.lead_list);
    conn.execute(
        "INSERT INTO subscriptions
            (endpoint, p256dh, auth, scope_kind, scope_value, lead_ms, lead_list, tz, hour24)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(endpoint, scope_kind, scope_value) DO UPDATE SET
            p256dh=excluded.p256dh, auth=excluded.auth,
            lead_ms=excluded.lead_ms, lead_list=excluded.lead_list, tz=excluded.tz,
            hour24=excluded.hour24",
        params![
            s.endpoint,
            s.p256dh,
            s.auth,
            s.scope_kind,
            s.scope_value,
            s.lead_ms,
            lead_list,
            s.tz,
            s.hour24
        ],
    )?;
    // If the timer set shrank, drop this scope's unsent reminders for offsets no
    // longer wanted, so a removed (e.g. global) timer stops firing immediately;
    // newly-added offsets are armed by the next expansion tick.
    prune_scope_leads(
        conn,
        &s.endpoint,
        &s.scope_kind,
        &s.scope_value,
        &s.lead_list,
    )?;
    Ok(())
}

/// Drop a scope's unsent reminders whose `lead_ms` is no longer in `leads`.
fn prune_scope_leads(
    conn: &Connection,
    endpoint: &str,
    kind: &str,
    value: &str,
    leads: &[i64],
) -> rusqlite::Result<()> {
    if leads.is_empty() {
        return Ok(());
    }
    // `leads` are i64s, so inlining them is injection-safe.
    let keep = join_lead_list(leads);
    let where_scope = scope_where(kind);
    conn.execute(
        &format!(
            "DELETE FROM reminders
             WHERE endpoint=?1 AND {where_scope} AND sent=0 AND lead_ms NOT IN ({keep})"
        ),
        params![endpoint, value],
    )?;
    Ok(())
}

pub fn remove_subscription(
    conn: &Connection,
    endpoint: &str,
    kind: &str,
    value: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM subscriptions WHERE endpoint=?1 AND scope_kind=?2 AND scope_value=?3",
        params![endpoint, kind, value],
    )?;
    Ok(())
}

pub fn list_subscriptions(conn: &Connection) -> rusqlite::Result<Vec<Subscription>> {
    let mut stmt = conn.prepare(
        "SELECT endpoint, p256dh, auth, scope_kind, scope_value, lead_ms, lead_list, tz, hour24
         FROM subscriptions",
    )?;
    let rows = stmt.query_map([], |r| {
        let lead_ms: i64 = r.get(5)?;
        let lead_list: String = r.get(6)?;
        Ok(Subscription {
            endpoint: r.get(0)?,
            p256dh: r.get(1)?,
            auth: r.get(2)?,
            scope_kind: r.get(3)?,
            scope_value: r.get(4)?,
            lead_ms,
            lead_list: parse_lead_list(&lead_list, lead_ms),
            tz: r.get(7)?,
            hour24: r.get(8)?,
        })
    })?;
    rows.collect()
}

/// On unsubscribe, drop unsent reminders this endpoint got from that scope. A
/// team matches either side; sport/league match their single column.
pub fn delete_unsent_reminders_by_scope(
    conn: &Connection,
    endpoint: &str,
    kind: &str,
    value: &str,
) -> rusqlite::Result<()> {
    let where_scope = scope_where(kind);
    conn.execute(
        &format!("DELETE FROM reminders WHERE endpoint=?1 AND sent=0 AND {where_scope}"),
        params![endpoint, value],
    )?;
    Ok(())
}

/// Remove everything an endpoint is signed up for — all subscriptions, all
/// (unsent) reminders, and all opt-outs. Backs "clear all".
pub fn clear_endpoint(conn: &Connection, endpoint: &str) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM subscriptions WHERE endpoint = ?1",
        params![endpoint],
    )?;
    conn.execute(
        "DELETE FROM reminders WHERE endpoint = ?1 AND sent = 0",
        params![endpoint],
    )?;
    conn.execute(
        "DELETE FROM exclusions WHERE endpoint = ?1",
        params![endpoint],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(id: i64, begin_at: DateTime<Utc>) -> NormalizedMatch {
        NormalizedMatch {
            id,
            sport: Sport::Lol,
            league: "LCK".into(),
            league_url: None,
            series_name: String::new(),
            tier: "A".into(),
            begin_at,
            status: MatchStatus::Upcoming,
            best_of: Some(3),
            team_a: NormalizedTeam {
                label: "T1".into(),
                name: "T1".into(),
                score: None,
                abbrev: String::new(),
            },
            team_b: NormalizedTeam {
                label: "GEN".into(),
                name: "Gen.G".into(),
                score: Some(1),
                abbrev: String::new(),
            },
            stream_url: Some("https://twitch.tv/lck".into()),
            tournament_id: Some(42),
            venue_tz: Some("America/New_York".into()),
            venue_name: String::new(),
            venue_location: String::new(),
            team_a_logo: String::new(),
            team_b_logo: String::new(),
            streams: Vec::new(),
            mlb_series: None,
            motor_result_ref: None,
        }
    }

    #[test]
    fn roundtrip_upsert_and_prune() {
        let mut conn = open(":memory:").unwrap();
        let now = Utc::now();
        let recent = sample(1, now);
        let old = sample(2, now - chrono::Duration::days(10));
        let cutoff = (now - chrono::Duration::days(2)).timestamp_millis();

        upsert_and_prune(
            &mut conn,
            &[recent, old],
            now.timestamp_millis(),
            cutoff,
            &[],
        )
        .unwrap();

        // The old match is pruned; the recent one survives and round-trips.
        let all = load_all(&conn).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, 1);
        assert_eq!(all[0].team_b.score, Some(1));
        assert_eq!(all[0].team_a.score, None);
        // Full team names round-trip (keys the team page + subscriptions).
        assert_eq!(all[0].team_a.name, "T1");
        assert_eq!(all[0].team_b.name, "Gen.G");
        // The venue timezone round-trips (drives the local-time toggle).
        assert_eq!(all[0].venue_tz.as_deref(), Some("America/New_York"));
        assert_eq!(load_fetched_at(&conn), Some(now.timestamp_millis()));

        // Upserting the same id updates in place (no duplicate row).
        let mut updated = sample(1, now);
        updated.status = MatchStatus::Finished;
        updated.team_a.score = Some(2);
        upsert_and_prune(&mut conn, &[updated], now.timestamp_millis(), cutoff, &[]).unwrap();
        let all = load_all(&conn).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].status, MatchStatus::Finished);
        assert_eq!(all[0].team_a.score, Some(2));
    }

    #[test]
    fn league_replace_drops_superseded_rows_but_spares_other_leagues() {
        let mut conn = open(":memory:").unwrap();
        let now = Utc::now();
        let cutoff = (now - chrono::Duration::days(30)).timestamp_millis();
        let ms = now.timestamp_millis();
        let motor = |id: i64, league: &str| {
            let mut m = sample(id, now);
            m.sport = Sport::Motorsport;
            m.league = league.into();
            m
        };

        // First poll: a WRC rally placeholder, plus an F1 row under the same sport.
        upsert_and_prune(
            &mut conn,
            &[motor(100, "WRC"), motor(200, "F1")],
            ms,
            cutoff,
            &[],
        )
        .unwrap();
        assert_eq!(load_all(&conn).unwrap().len(), 2);

        // Next poll: the rally is expanded into stage rows with new ids, so the WRC
        // set no longer contains the placeholder. The league-scoped replace drops it.
        upsert_and_prune(
            &mut conn,
            &[motor(101, "WRC"), motor(102, "WRC"), motor(200, "F1")],
            ms,
            cutoff,
            &[("WRC", vec![101, 102])],
        )
        .unwrap();

        let ids: std::collections::HashSet<i64> =
            load_all(&conn).unwrap().iter().map(|m| m.id).collect();
        // Superseded WRC placeholder (100) gone; its stages and the F1 row (a league
        // not being replaced) survive.
        assert_eq!(ids, std::collections::HashSet::from([101, 102, 200]));
    }

    #[test]
    fn motor_result_ref_round_trips() {
        use crate::types::MotorResultRef;
        let mut conn = open(":memory:").unwrap();
        let now = Utc::now();
        let ms = now.timestamp_millis();
        let cutoff = (now - chrono::Duration::days(30)).timestamp_millis();

        // A WRC stage row (per-stage result) and a rally placeholder (overall
        // classification, empty session id).
        let mut stage = sample(7, now);
        stage.sport = Sport::Motorsport;
        stage.league = "WRC".into();
        stage.motor_result_ref = Some(MotorResultRef {
            series: "WRC".into(),
            event_id: "rally-uuid".into(),
            session_id: "stage-uuid".into(),
        });
        let mut placeholder = sample(8, now);
        placeholder.sport = Sport::Motorsport;
        placeholder.league = "WRC".into();
        placeholder.motor_result_ref = Some(MotorResultRef {
            series: "WRC".into(),
            event_id: "rally2-uuid".into(),
            session_id: String::new(),
        });

        upsert_and_prune(&mut conn, &[stage, placeholder], ms, cutoff, &[]).unwrap();

        let all = load_all(&conn).unwrap();
        let by_id = |id: i64| {
            all.iter()
                .find(|m| m.id == id)
                .unwrap()
                .motor_result_ref
                .clone()
        };
        // The per-stage ref survives the DB round-trip, so a restart renders the
        // stage's results immediately instead of waiting for the next ocb fetch.
        assert_eq!(
            by_id(7),
            Some(MotorResultRef {
                series: "WRC".into(),
                event_id: "rally-uuid".into(),
                session_id: "stage-uuid".into(),
            })
        );
        // The placeholder's empty session id round-trips too (overall result).
        assert_eq!(by_id(8).unwrap().session_id, "");
        // A row with no ref stays None.
        let plain = sample(9, now);
        upsert_and_prune(&mut conn, &[plain], ms, cutoff, &[]).unwrap();
        let all = load_all(&conn).unwrap();
        assert!(all
            .iter()
            .find(|m| m.id == 9)
            .unwrap()
            .motor_result_ref
            .is_none());
    }

    #[test]
    fn purges_rows_under_a_renamed_sport_slug() {
        let conn = open(":memory:").unwrap();
        // Simulate a row left behind under a since-renamed slug ("soccer" was
        // renamed to "football"), alongside a valid one. The legacy row would
        // otherwise load as the default sport and surface on the esports page.
        conn.execute(
            "INSERT INTO matches (id, sport, league, tier, begin_at_ms, status,
                team_a_label, team_b_label) VALUES
                (1, 'soccer', 'World Cup', 'S', 100, 'upcoming', 'Brazil', 'France'),
                (2, 'football', 'World Cup', 'S', 200, 'upcoming', 'Spain', 'Japan')",
            [],
        )
        .unwrap();
        // A direct load skips the unparseable row even before the migration runs.
        assert_eq!(load_all(&conn).unwrap().len(), 1);

        // Re-opening the same DB runs the purge, removing the orphan for good.
        purge_unknown_sports(&conn).unwrap();
        let rows = load_all(&conn).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].sport, Sport::Soccer);
        assert_eq!(rows[0].id, 2);
        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM matches", [], |r| r.get(0))
            .unwrap();
        assert_eq!(remaining, 1, "the legacy 'soccer' row is deleted");
    }

    #[test]
    fn rename_column_migration_preserves_data() {
        // Mirrors the in-place upgrade `open()` applies to DBs written before the
        // game->sport / serie_name->series_name rename. The `unwrap`s assert this
        // SQLite build supports RENAME COLUMN and that the row's data survives.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE matches (id INTEGER NOT NULL, game TEXT NOT NULL,
                serie_name TEXT, PRIMARY KEY (id, game));",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO matches (id, game, serie_name) VALUES (1, 'lol', 'Spring')",
            [],
        )
        .unwrap();
        conn.execute("ALTER TABLE matches RENAME COLUMN game TO sport", [])
            .unwrap();
        conn.execute(
            "ALTER TABLE matches RENAME COLUMN serie_name TO series_name",
            [],
        )
        .unwrap();
        let (sport, series): (String, String) = conn
            .query_row(
                "SELECT sport, series_name FROM matches WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(sport, "lol");
        assert_eq!(series, "Spring");
    }

    fn reminder(match_id: i64) -> Reminder {
        Reminder {
            endpoint: "https://push.example/x".into(),
            p256dh: "p".into(),
            auth: "a".into(),
            match_id,
            lead_ms: 900_000,
            notify_at_ms: 100,
            title: "T1 vs GEN".into(),
            body: "LCK".into(),
            url: "u".into(),
            sport: "lol".into(),
            league: "LCK".into(),
            team_a: "T1".into(),
            team_b: "Gen.G".into(),
            event: "LCK Spring".into(),
            tz: String::new(),
            hour24: false,
            sent: false,
        }
    }

    #[test]
    fn v2_to_v3_migration_keeps_the_tz_and_hour24_columns() {
        // The tz/hour24 columns are added by an ALTER placed *after* the v3 table
        // rebuild — which recreates `reminders` from a fixed column list. This
        // guards that ordering: a pre-v3 DB (has the old `game` + `excluded`
        // columns, no reminders_pk_v3 meta) must still end up with the columns.
        let path = std::env::temp_dir().join("pte_v3_tz_migration_test.sqlite");
        let _ = std::fs::remove_file(&path);
        {
            let conn = Connection::open(&path).unwrap();
            // A post-v2, pre-v3 DB: the `reminders_pk_v2` flag is already set (so
            // open() doesn't drop the table), the table carries the pre-v3
            // `excluded` column, and there's no `reminders_pk_v3` flag yet — so the
            // v3 rebuild runs and must preserve the row.
            conn.execute_batch(
                "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 INSERT INTO meta (key, value) VALUES ('reminders_pk_v2', '1');
                 CREATE TABLE reminders (
                    endpoint TEXT NOT NULL, p256dh TEXT NOT NULL, auth TEXT NOT NULL,
                    match_id INTEGER NOT NULL, sport TEXT NOT NULL DEFAULT '',
                    notify_at_ms INTEGER NOT NULL, title TEXT NOT NULL, body TEXT NOT NULL,
                    url TEXT NOT NULL, excluded INTEGER NOT NULL DEFAULT 0,
                    sent INTEGER NOT NULL DEFAULT 0,
                    PRIMARY KEY (endpoint, match_id, sport)
                 );
                 INSERT INTO reminders
                    (endpoint, p256dh, auth, match_id, sport, notify_at_ms, title, body, url, excluded, sent)
                 VALUES ('https://push/x', 'p', 'a', 7, 'lol', 123, 'T', 'B', 'u', 0, 0);",
            )
            .unwrap();
        }
        let conn = open(path.to_str().unwrap()).unwrap();
        let has_col = |name: &str| -> bool {
            conn.query_row(
                "SELECT COUNT(*) FROM pragma_table_info('reminders') WHERE name = ?1",
                [name],
                |r| r.get::<_, i64>(0),
            )
            .unwrap()
                > 0
        };
        assert!(has_col("tz"), "tz column present after the v2→v3 upgrade");
        assert!(
            has_col("hour24"),
            "hour24 column present after the v2→v3 upgrade"
        );
        // The pre-v3 row survived the rebuild, defaulting to the empty (server) tz.
        let (mid, tz): (i64, String) = conn
            .query_row(
                "SELECT match_id, tz FROM reminders WHERE match_id = 7",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(mid, 7);
        assert_eq!(tz, "");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn all_reminders_round_trips_zone_clock_and_sent_flag() {
        let conn = open(":memory:").unwrap();
        let mut r = reminder(5);
        r.tz = "America/New_York".into();
        r.hour24 = true;
        add_reminder(&conn, &r).unwrap();

        let all = all_reminders(&conn).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].tz, "America/New_York");
        assert!(all[0].hour24);
        assert!(!all[0].sent, "freshly armed → unsent");

        // A delivered reminder stays in the set, now flagged sent — so the reconcile
        // can still announce a cancellation after every timer has fired.
        mark_reminder_sent(&conn, &r.endpoint, r.match_id, &r.sport, r.lead_ms).unwrap();
        let all = all_reminders(&conn).unwrap();
        assert_eq!(all.len(), 1);
        assert!(all[0].sent, "delivered → sent");
    }

    #[test]
    fn update_reminder_schedule_moves_notify_and_body_but_only_when_unsent() {
        let conn = open(":memory:").unwrap();
        let r = reminder(6);
        add_reminder(&conn, &r).unwrap();

        // Reschedule earlier: notify + body move.
        let mut moved = r.clone();
        moved.notify_at_ms = r.notify_at_ms - 1_800_000;
        moved.body = "LCK · 7:40 PM PDT".into();
        update_reminder_schedule(&conn, &moved).unwrap();
        let after = all_reminders(&conn).unwrap();
        assert_eq!(after[0].notify_at_ms, r.notify_at_ms - 1_800_000);
        assert_eq!(after[0].body, "LCK · 7:40 PM PDT");

        // Once sent, a further reschedule must not rewrite the row.
        mark_reminder_sent(&conn, &r.endpoint, r.match_id, &r.sport, r.lead_ms).unwrap();
        let mut again = moved.clone();
        again.notify_at_ms = 42;
        again.body = "should not apply".into();
        update_reminder_schedule(&conn, &again).unwrap();
        let stored: (i64, String) = conn
            .query_row(
                "SELECT notify_at_ms, body FROM reminders WHERE match_id = 6",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            stored.0,
            r.notify_at_ms - 1_800_000,
            "notify unchanged once sent"
        );
        assert_eq!(stored.1, "LCK · 7:40 PM PDT", "body unchanged once sent");
    }

    #[test]
    fn reminders_with_the_same_id_across_games_coexist() {
        let conn = open(":memory:").unwrap();
        // Two different matches that happen to share a numeric id, one per sport.
        let mut a = reminder(42);
        a.sport = "cs2".into();
        let mut b = reminder(42);
        b.sport = "mlb".into();
        add_reminder(&conn, &a).unwrap();
        add_reminder(&conn, &b).unwrap();
        // Both survive — the PK is (endpoint, match_id, sport), not (endpoint, id).
        assert_eq!(due_reminders(&conn, 1000).unwrap().len(), 2);
        // Removing one sport leaves the other.
        remove_reminder(&conn, &a.endpoint, 42, "cs2").unwrap();
        let due = due_reminders(&conn, 1000).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].match_id, 42);
    }

    #[test]
    fn exclusion_blocks_a_covered_match_until_re_armed() {
        let conn = open(":memory:").unwrap();
        let r = reminder(1);
        // A subscription expansion arms it → it's due.
        add_reminder_if_absent(&conn, &r).unwrap();
        assert_eq!(due_reminders(&conn, 1000).unwrap().len(), 1);

        // Opt this one match out → the opt-out drops its row, and the expansion
        // loop is expected to skip it (see `list_exclusions`), so it stays gone.
        exclude_match(&conn, &r.endpoint, r.match_id, &r.sport).unwrap();
        assert_eq!(due_reminders(&conn, 1000).unwrap().len(), 0);
        assert!(list_exclusions(&conn).unwrap().contains(&(
            r.endpoint.clone(),
            r.match_id,
            r.sport.clone()
        )));

        // Re-include it (explicit arm) → opt-out cleared and it's due again.
        add_reminder(&conn, &r).unwrap();
        assert_eq!(due_reminders(&conn, 1000).unwrap().len(), 1);
        assert!(list_exclusions(&conn).unwrap().is_empty());
    }

    #[test]
    fn set_match_reminders_replaces_the_timer_set() {
        let conn = open(":memory:").unwrap();
        // Which lead offsets are stored for a match (asserts the row set itself —
        // these timers fire a day/hour apart, so they're never simultaneously due).
        let stored_leads = |conn: &Connection, match_id: i64| -> Vec<i64> {
            conn.prepare("SELECT lead_ms FROM reminders WHERE match_id=?1 ORDER BY lead_ms")
                .unwrap()
                .query_map([match_id], |r| r.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        // Three timers for one match (1d / 1h / 15m before its start).
        let begin = 100_000_000_000_i64;
        let mut rs = Vec::new();
        for &lead in &[86_400_000_i64, 3_600_000, 900_000] {
            let mut r = reminder(7);
            r.lead_ms = lead;
            r.notify_at_ms = begin - lead;
            rs.push(r);
        }
        set_match_reminders(&conn, &rs).unwrap();
        assert_eq!(stored_leads(&conn, 7), vec![900_000, 3_600_000, 86_400_000]);

        // Re-set with a smaller set → the dropped timers' unsent rows are removed.
        set_match_reminders(&conn, &rs[..1]).unwrap();
        assert_eq!(stored_leads(&conn, 7), vec![86_400_000]);
    }

    #[test]
    fn mark_one_timer_sent_leaves_the_others_due() {
        let conn = open(":memory:").unwrap();
        let mut a = reminder(9);
        a.lead_ms = 3_600_000;
        let mut b = reminder(9);
        b.lead_ms = 900_000;
        add_reminder(&conn, &a).unwrap();
        add_reminder(&conn, &b).unwrap();
        assert_eq!(due_reminders(&conn, 1000).unwrap().len(), 2);
        // Marking the 1h timer sent must not silence the 15m timer for the match.
        mark_reminder_sent(&conn, &a.endpoint, a.match_id, &a.sport, a.lead_ms).unwrap();
        let due = due_reminders(&conn, 1000).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].lead_ms, 900_000);
    }

    #[test]
    fn timers_fire_each_at_its_own_time() {
        let conn = open(":memory:").unwrap();
        let begin = 10_000_000_000_i64;
        let day = 86_400_000_i64;
        let hour = 3_600_000_i64;
        let min = 60_000_i64;
        // One match, three timers: 1 day / 1 hour / 15 min before the start.
        for &lead in &[day, hour, 15 * min] {
            let mut r = reminder(7);
            r.lead_ms = lead;
            r.notify_at_ms = begin - lead;
            add_reminder(&conn, &r).unwrap();
        }
        // Simulate one push-sender tick at `now`: deliver everything due, marking
        // each delivered row sent, and report which lead offsets fired.
        let mut tick = |now: i64| -> Vec<i64> {
            let due = due_reminders(&conn, now).unwrap();
            let mut leads: Vec<i64> = due.iter().map(|r| r.lead_ms).collect();
            for r in &due {
                mark_reminder_sent(&conn, &r.endpoint, r.match_id, &r.sport, r.lead_ms).unwrap();
            }
            leads.sort_unstable();
            leads
        };
        // Each timer fires at its own moment, in order, and none before its time.
        assert_eq!(
            tick(begin - 2 * day),
            Vec::<i64>::new(),
            "nothing two days out"
        );
        assert_eq!(tick(begin - day), vec![day], "the 1-day timer");
        assert_eq!(tick(begin - hour), vec![hour], "then the 1-hour timer");
        assert_eq!(
            tick(begin - 15 * min),
            vec![15 * min],
            "then the 15-min timer"
        );
        // Each fired exactly once — no timer re-delivers at the start.
        assert_eq!(tick(begin), Vec::<i64>::new(), "no timer re-fires");
    }

    #[test]
    fn behind_reminder_after_start_is_suppressed() {
        let conn = open(":memory:").unwrap();
        // A 15-min-lead reminder for a match starting at t = 1_000_000.
        let mut r = reminder(1);
        r.lead_ms = 900_000;
        r.notify_at_ms = 1_000_000 - r.lead_ms;
        add_reminder(&conn, &r).unwrap();
        let begin_at = r.notify_at_ms + r.lead_ms; // 1_000_000

        // Delivered on time (just past the notify instant): due.
        assert_eq!(due_reminders(&conn, r.notify_at_ms).unwrap().len(), 1);
        // A little late but the match hasn't started yet: still due.
        assert_eq!(due_reminders(&conn, begin_at - 1).unwrap().len(), 1);
        // Just past the start, inside the grace (covers a brief restart): due.
        assert_eq!(due_reminders(&conn, begin_at + 60_000).unwrap().len(), 1);
        // Well past the start — the server was down across the whole window, so
        // a "starts soon" ping is pointless now: suppressed.
        assert_eq!(
            due_reminders(&conn, begin_at + DELIVER_GRACE_MS + 1)
                .unwrap()
                .len(),
            0
        );
    }

    #[test]
    fn far_future_reminder_behind_schedule_still_fires() {
        let conn = open(":memory:").unwrap();
        // A 1-week-lead reminder whose notify time slipped a minute into the past
        // (the server restarted just after it came due). The match is still ~a
        // week out, so the reminder is still useful and must fire.
        let now = 5_000_000_000_i64;
        let mut r = reminder(2);
        r.lead_ms = WEEK_MS;
        r.notify_at_ms = now - 60_000;
        add_reminder(&conn, &r).unwrap();
        assert_eq!(due_reminders(&conn, now).unwrap().len(), 1);
    }

    #[test]
    fn long_lead_reminder_far_behind_schedule_is_dropped() {
        let conn = open(":memory:").unwrap();
        // A 1-week-lead reminder the server missed by three hours (it was off that
        // long). The match is still days away, so the begin_at grace wouldn't catch
        // it — but it's well past its notify time, so the staleness cap drops it
        // rather than firing a "race in a week" ping three hours late.
        let now = 5_000_000_000_i64;
        let mut r = reminder(3);
        r.lead_ms = WEEK_MS;
        r.notify_at_ms = now - 3 * 60 * 60 * 1000; // 3h late, > MAX_LATE_MS
        add_reminder(&conn, &r).unwrap();
        assert_eq!(due_reminders(&conn, now).unwrap().len(), 0);
        // …but the same timer only a few minutes late still fires (brief restart).
        let mut fresh = reminder(4);
        fresh.lead_ms = WEEK_MS;
        fresh.notify_at_ms = now - 5 * 60 * 1000;
        add_reminder(&conn, &fresh).unwrap();
        assert_eq!(
            due_reminders(&conn, now)
                .unwrap()
                .iter()
                .filter(|r| r.match_id == 4)
                .count(),
            1
        );
    }

    #[test]
    fn prune_keeps_upcoming_reminders_but_drops_started_ones() {
        let conn = open(":memory:").unwrap();
        let now = 10_000_000_000_i64;
        let day_ms = 24 * 60 * 60 * 1000;
        // A long-lead reminder that already fired (sent) but whose match is still
        // a week away. Pruning it would let expansion re-arm and double-fire it.
        let mut upcoming = reminder(1);
        upcoming.lead_ms = WEEK_MS;
        upcoming.notify_at_ms = now - 1000;
        add_reminder(&conn, &upcoming).unwrap();
        mark_reminder_sent(
            &conn,
            &upcoming.endpoint,
            upcoming.match_id,
            &upcoming.sport,
            upcoming.lead_ms,
        )
        .unwrap();
        // A reminder whose match started two days ago — safe to reap.
        let mut started = reminder(2);
        started.lead_ms = 0;
        started.notify_at_ms = now - 2 * day_ms;
        add_reminder(&conn, &started).unwrap();

        // Prune everything whose match started more than a day ago.
        prune_reminders(&conn, now - day_ms).unwrap();

        let ids: Vec<i64> = conn
            .prepare("SELECT match_id FROM reminders ORDER BY match_id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(ids, vec![1], "the upcoming match's reminder is retained");
    }

    #[test]
    fn delete_by_event_scope_removes_only_that_event() {
        let conn = open(":memory:").unwrap();
        let mut a = reminder(1);
        a.event = "F1 Austrian Grand Prix".into();
        let mut b = reminder(2);
        b.event = "F1 British Grand Prix".into();
        add_reminder(&conn, &a).unwrap();
        add_reminder(&conn, &b).unwrap();
        assert_eq!(due_reminders(&conn, 1000).unwrap().len(), 2);

        // Unsubscribing from one GP drops only its reminders.
        delete_unsent_reminders_by_scope(
            &conn,
            "https://push.example/x",
            "event",
            "F1 Austrian Grand Prix",
        )
        .unwrap();
        let due = due_reminders(&conn, 1000).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].match_id, 2);
    }

    #[test]
    fn subscription_round_trips_tz_and_leads() {
        let conn = open(":memory:").unwrap();
        add_subscription(
            &conn,
            &Subscription {
                endpoint: "https://push.example/x".into(),
                p256dh: "p".into(),
                auth: "a".into(),
                scope_kind: "league".into(),
                scope_value: "LCK".into(),
                lead_ms: 900_000,
                lead_list: vec![900_000, 3_600_000],
                tz: "America/New_York".into(),
                hour24: true,
            },
        )
        .unwrap();
        let subs = list_subscriptions(&conn).unwrap();
        assert_eq!(subs.len(), 1);
        // The subscriber's tz + 12h/24h pref persist, so expansion bakes bodies in
        // their zone and chosen clock format.
        assert_eq!(subs[0].tz, "America/New_York");
        assert!(subs[0].hour24);
        assert_eq!(subs[0].lead_list, vec![900_000, 3_600_000]);
    }

    #[test]
    fn migrate_endpoint_moves_rows_to_the_new_subscription() {
        let conn = open(":memory:").unwrap();
        // A reminder + a subscription under the old endpoint.
        add_reminder(&conn, &reminder(1)).unwrap();
        add_subscription(
            &conn,
            &Subscription {
                endpoint: "https://push.example/x".into(),
                p256dh: "old-p".into(),
                auth: "old-a".into(),
                scope_kind: "league".into(),
                scope_value: "LCK".into(),
                lead_ms: 900_000,
                lead_list: vec![900_000],
                tz: "America/New_York".into(),
                hour24: false,
            },
        )
        .unwrap();

        let new = crate::types::PushSub {
            endpoint: "https://push.example/NEW".into(),
            p256dh: "new-p".into(),
            auth: "new-a".into(),
        };
        migrate_endpoint(&conn, "https://push.example/x", &new).unwrap();

        // Nothing left under the old endpoint…
        let old_count: i64 = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM reminders WHERE endpoint=?1)
                      + (SELECT COUNT(*) FROM subscriptions WHERE endpoint=?1)",
                params!["https://push.example/x"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(old_count, 0);
        // …and the rows now live under the new endpoint with its fresh keys.
        let subs = list_subscriptions(&conn).unwrap();
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].endpoint, "https://push.example/NEW");
        assert_eq!(subs[0].p256dh, "new-p");
        assert_eq!(subs[0].tz, "America/New_York"); // preserved across the move
        let due = due_reminders(&conn, 1000).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].endpoint, "https://push.example/NEW");
        assert_eq!(due[0].p256dh, "new-p");
    }

    #[test]
    fn clear_endpoint_removes_subs_and_reminders() {
        let conn = open(":memory:").unwrap();
        add_reminder(&conn, &reminder(1)).unwrap();
        add_subscription(
            &conn,
            &Subscription {
                endpoint: "https://push.example/x".into(),
                p256dh: "p".into(),
                auth: "a".into(),
                scope_kind: "team".into(),
                scope_value: "T1".into(),
                lead_ms: 0,
                lead_list: vec![0],
                tz: String::new(),
                hour24: false,
            },
        )
        .unwrap();
        clear_endpoint(&conn, "https://push.example/x").unwrap();
        assert_eq!(due_reminders(&conn, 1000).unwrap().len(), 0);
        assert_eq!(list_subscriptions(&conn).unwrap().len(), 0);
    }

    #[test]
    fn meta_get_set_roundtrip() {
        let conn = open(":memory:").unwrap();
        assert_eq!(get_meta(&conn, "backfill_cursor_ms"), None);
        set_meta(&conn, "backfill_cursor_ms", "1700000000000").unwrap();
        assert_eq!(
            get_meta(&conn, "backfill_cursor_ms").as_deref(),
            Some("1700000000000")
        );
        // Upsert overwrites in place.
        set_meta(&conn, "backfill_cursor_ms", "1690000000000").unwrap();
        assert_eq!(
            get_meta(&conn, "backfill_cursor_ms").as_deref(),
            Some("1690000000000")
        );
    }

    #[test]
    fn event_links_roundtrip_and_upsert() {
        let conn = open(":memory:").unwrap();
        set_event_link(&conn, "lol|MSI|2026", Some("https://liquipedia.net/x"), 100).unwrap();
        set_event_link(&conn, "cs2|XSE|2026", None, 200).unwrap();
        let mut rows = load_event_links(&conn).unwrap();
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], ("cs2|XSE|2026".to_string(), None, 200));
        assert_eq!(
            rows[1],
            (
                "lol|MSI|2026".to_string(),
                Some("https://liquipedia.net/x".to_string()),
                100
            )
        );
        // Upsert updates in place (no duplicate).
        set_event_link(&conn, "cs2|XSE|2026", Some("https://liquipedia.net/y"), 300).unwrap();
        assert_eq!(load_event_links(&conn).unwrap().len(), 2);
    }

    #[test]
    fn result_cache_roundtrips_and_upserts() {
        let conn = open(":memory:").unwrap();
        cache_put(&conn, "motor_result", "WRC|a|b", "[1,2,3]", 1000).unwrap();
        let got = cache_get(&conn, "motor_result", "WRC|a|b")
            .unwrap()
            .unwrap();
        assert_eq!(got.value, "[1,2,3]");
        assert_eq!(got.fetched_at_ms, 1000);
        // Upsert replaces value + timestamp on the same (ns, key).
        cache_put(&conn, "motor_result", "WRC|a|b", "[9]", 2000).unwrap();
        let got = cache_get(&conn, "motor_result", "WRC|a|b")
            .unwrap()
            .unwrap();
        assert_eq!(got.value, "[9]");
        assert_eq!(got.fetched_at_ms, 2000);
        // Absent key → None.
        assert!(cache_get(&conn, "motor_result", "missing")
            .unwrap()
            .is_none());
    }

    #[test]
    fn result_cache_get_ns_returns_namespace_rows_in_key_order() {
        let conn = open(":memory:").unwrap();
        cache_put(&conn, "soccer_standings", "World Cup", "[]", 1).unwrap();
        cache_put(&conn, "soccer_standings", "Premier League", "[]", 2).unwrap();
        cache_put(&conn, "mlb_standings", "", "[]", 3).unwrap();
        let rows = cache_get_ns(&conn, "soccer_standings").unwrap();
        let keys: Vec<&str> = rows.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, vec!["Premier League", "World Cup"]); // ordered by key
    }
}
