//! Persistent SQLite cache for normalized matches (server-only).
//!
//! The in-memory snapshot stays the hot path for serving requests; this layer
//! only persists across restarts. It's touched at boot (load) and after each
//! poll (upsert + prune). Matches are keyed by `(id, game)` and upserted, so a
//! match that finishes and drops out of the API's window is retained until it
//! ages past the retention cutoff.

use crate::pandascore::{NormTeam, NormalizedMatch};
use crate::types::{Game, MatchStatus};
use chrono::{DateTime, Utc};
use once_cell::sync::OnceCell;
use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::{Mutex, PoisonError};

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
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS matches (
            id           INTEGER NOT NULL,
            game         TEXT    NOT NULL,
            league       TEXT    NOT NULL,
            league_url   TEXT,
            serie_name   TEXT,
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
            venue_tz     TEXT,
            stream_url   TEXT,
            tournament_id INTEGER,
            PRIMARY KEY (id, game)
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
            notify_at_ms INTEGER NOT NULL,
            title        TEXT    NOT NULL,
            body         TEXT    NOT NULL,
            url          TEXT    NOT NULL,
            game         TEXT    NOT NULL DEFAULT '',
            league       TEXT    NOT NULL DEFAULT '',
            team_a       TEXT    NOT NULL DEFAULT '',
            team_b       TEXT    NOT NULL DEFAULT '',
            -- Full event name (league + edition, e.g. F1 Austrian Grand Prix), so
            -- an event subscription's reminders can be cleaned up on unsub.
            event        TEXT    NOT NULL DEFAULT '',
            sent         INTEGER NOT NULL DEFAULT 0,
            -- A tombstone row (excluded=1) opts this match out of a covering
            -- subscription: expansion's insert-if-absent leaves it, and the
            -- sender skips it, so the reminder never fires.
            excluded     INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (endpoint, match_id)
        );
        CREATE TABLE IF NOT EXISTS subscriptions (
            endpoint    TEXT    NOT NULL,
            p256dh      TEXT    NOT NULL,
            auth        TEXT    NOT NULL,
            scope_kind  TEXT    NOT NULL,
            scope_value TEXT    NOT NULL,
            lead_ms     INTEGER NOT NULL,
            PRIMARY KEY (endpoint, scope_kind, scope_value)
        );",
    )?;
    // Migrate older DBs (ignored if the column already exists).
    let _ = conn.execute("ALTER TABLE matches ADD COLUMN league_url TEXT", []);
    let _ = conn.execute("ALTER TABLE matches ADD COLUMN tournament_id INTEGER", []);
    let _ = conn.execute("ALTER TABLE matches ADD COLUMN serie_name TEXT", []);
    let _ = conn.execute("ALTER TABLE matches ADD COLUMN team_a_name TEXT", []);
    let _ = conn.execute("ALTER TABLE matches ADD COLUMN team_b_name TEXT", []);
    let _ = conn.execute("ALTER TABLE matches ADD COLUMN venue_tz TEXT", []);
    let _ = conn.execute("ALTER TABLE reminders ADD COLUMN game TEXT NOT NULL DEFAULT ''", []);
    let _ = conn.execute("ALTER TABLE reminders ADD COLUMN league TEXT NOT NULL DEFAULT ''", []);
    let _ = conn.execute("ALTER TABLE reminders ADD COLUMN team_a TEXT NOT NULL DEFAULT ''", []);
    let _ = conn.execute("ALTER TABLE reminders ADD COLUMN team_b TEXT NOT NULL DEFAULT ''", []);
    let _ = conn.execute("ALTER TABLE reminders ADD COLUMN event TEXT NOT NULL DEFAULT ''", []);
    let _ = conn.execute("ALTER TABLE reminders ADD COLUMN excluded INTEGER NOT NULL DEFAULT 0", []);
    purge_unknown_games(&conn)?;
    Ok(conn)
}

/// Delete match rows whose `game` slug isn't a current [`Game`] — orphans left
/// behind when a game's slug is renamed (e.g. soccer's "soccer" → "football").
/// Otherwise such rows load as the default game and surface on the wrong page.
/// Cheap and idempotent, so it runs on every open.
fn purge_unknown_games(conn: &Connection) -> rusqlite::Result<()> {
    let slugs: Vec<&str> = Game::ALL.iter().map(|g| g.slug()).collect();
    let placeholders = vec!["?"; slugs.len()].join(",");
    conn.execute(
        &format!("DELETE FROM matches WHERE game NOT IN ({placeholders})"),
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
    let game: String = row.get("game")?;
    // Drop rows whose game slug no longer maps to a `Game` (e.g. a row written
    // under a since-renamed slug). Defaulting an unknown game to Cs2 would
    // mislabel it and surface it on the wrong page, so skip it instead.
    let Some(game) = Game::from_filter(&game) else {
        return Ok(None);
    };
    let begin_ms: i64 = row.get("begin_at_ms")?;
    let status: String = row.get("status")?;
    Ok(Some(NormalizedMatch {
        id: row.get("id")?,
        game,
        league: row.get("league")?,
        league_url: row.get("league_url")?,
        serie_name: row.get::<_, Option<String>>("serie_name")?.unwrap_or_default(),
        tier: row.get("tier")?,
        begin_at: DateTime::from_timestamp_millis(begin_ms).unwrap_or_else(Utc::now),
        status: MatchStatus::from_db(&status),
        best_of: row.get("best_of")?,
        team_a: NormTeam {
            label: row.get("team_a_label")?,
            // Older rows predate team_a_name; fall back to the label.
            name: team_name(row, "team_a_name", "team_a_label")?,
            score: row.get("team_a_score")?,
        },
        team_b: NormTeam {
            label: row.get("team_b_label")?,
            name: team_name(row, "team_b_name", "team_b_label")?,
            score: row.get("team_b_score")?,
        },
        stream_url: row.get("stream_url")?,
        tournament_id: row.get("tournament_id")?,
        venue_tz: row.get("venue_tz")?,
        // Streams aren't persisted (in-memory only); repopulated on next poll.
        streams: Vec::new(),
        // Series refs aren't persisted (MLB, in-memory); repopulated on next poll.
        mlb_series: None,
    }))
}

/// Load all stored matches, ordered by start time. Rows with an unrecognized
/// game slug are skipped (see [`row_to_match`]).
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
    conn.query_row(
        "SELECT value FROM meta WHERE key = ?1",
        params![key],
        |r| r.get::<_, String>(0),
    )
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
) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    {
        let mut up = tx.prepare(
            "INSERT INTO matches
                (id, game, league, tier, begin_at_ms, status, best_of,
                 team_a_label, team_a_score, team_b_label, team_b_score, stream_url,
                 league_url, tournament_id, serie_name, team_a_name, team_b_name, venue_tz)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
             ON CONFLICT(id, game) DO UPDATE SET
                league=excluded.league, tier=excluded.tier,
                begin_at_ms=excluded.begin_at_ms, status=excluded.status,
                best_of=excluded.best_of,
                team_a_label=excluded.team_a_label, team_a_score=excluded.team_a_score,
                team_b_label=excluded.team_b_label, team_b_score=excluded.team_b_score,
                stream_url=excluded.stream_url, league_url=excluded.league_url,
                tournament_id=excluded.tournament_id, serie_name=excluded.serie_name,
                team_a_name=excluded.team_a_name, team_b_name=excluded.team_b_name,
                venue_tz=excluded.venue_tz",
        )?;
        for m in matches {
            up.execute(params![
                m.id,
                m.game.slug(),
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
                m.serie_name,
                m.team_a.name,
                m.team_b.name,
                m.venue_tz,
            ])?;
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

/// A stored Web Push reminder.
#[derive(Debug, Clone)]
pub struct Reminder {
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
    pub match_id: i64,
    pub notify_at_ms: i64,
    pub title: String,
    pub body: String,
    pub url: String,
    /// Game/league/teams/event the match belongs to (for scope-based cleanup).
    pub game: String,
    pub league: String,
    pub team_a: String,
    pub team_b: String,
    /// Full event name (e.g. F1 Austrian Grand Prix), for event-scope cleanup.
    pub event: String,
}

const REMINDER_COLS: &str = "endpoint, p256dh, auth, match_id, notify_at_ms, title, body, url, \
     game, league, team_a, team_b, event";

fn reminder_params(r: &Reminder) -> [&dyn rusqlite::ToSql; 13] {
    [
        &r.endpoint,
        &r.p256dh,
        &r.auth,
        &r.match_id,
        &r.notify_at_ms,
        &r.title,
        &r.body,
        &r.url,
        &r.game,
        &r.league,
        &r.team_a,
        &r.team_b,
        &r.event,
    ]
}

/// Add or update a reminder (re-arms `sent`, and clears any exclusion tombstone).
/// Used for an explicit star, or to re-include a match opted out of a scope.
pub fn add_reminder(conn: &Connection, r: &Reminder) -> rusqlite::Result<()> {
    conn.execute(
        &format!(
            "INSERT INTO reminders ({REMINDER_COLS}, sent, excluded)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 0, 0)
             ON CONFLICT(endpoint, match_id) DO UPDATE SET
                p256dh=excluded.p256dh, auth=excluded.auth,
                notify_at_ms=excluded.notify_at_ms, title=excluded.title,
                body=excluded.body, url=excluded.url, game=excluded.game,
                league=excluded.league, team_a=excluded.team_a,
                team_b=excluded.team_b, event=excluded.event, sent=0, excluded=0"
        ),
        &reminder_params(r)[..],
    )?;
    Ok(())
}

/// Tombstone a match so a covering subscription won't notify about it: an
/// `excluded=1` row that expansion's insert-if-absent leaves untouched and the
/// sender skips. Re-include it by [`add_reminder`] (which clears the flag).
pub fn exclude_reminder(conn: &Connection, r: &Reminder) -> rusqlite::Result<()> {
    conn.execute(
        &format!(
            "INSERT INTO reminders ({REMINDER_COLS}, sent, excluded)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 0, 1)
             ON CONFLICT(endpoint, match_id) DO UPDATE SET excluded=1, sent=0"
        ),
        &reminder_params(r)[..],
    )?;
    Ok(())
}

/// Insert a reminder only if one doesn't exist (so re-expanding a game/event
/// subscription never re-arms an already-sent reminder).
pub fn add_reminder_if_absent(conn: &Connection, r: &Reminder) -> rusqlite::Result<()> {
    conn.execute(
        &format!(
            "INSERT INTO reminders ({REMINDER_COLS}, sent)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 0)
             ON CONFLICT(endpoint, match_id) DO NOTHING"
        ),
        &reminder_params(r)[..],
    )?;
    Ok(())
}

pub fn remove_reminder(conn: &Connection, endpoint: &str, match_id: i64) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM reminders WHERE endpoint = ?1 AND match_id = ?2",
        params![endpoint, match_id],
    )?;
    Ok(())
}

/// Unsent reminders whose notify time has arrived.
pub fn due_reminders(conn: &Connection, now_ms: i64) -> rusqlite::Result<Vec<Reminder>> {
    let mut stmt = conn.prepare(
        "SELECT endpoint, p256dh, auth, match_id, notify_at_ms, title, body, url
         FROM reminders
         WHERE sent = 0 AND excluded = 0 AND notify_at_ms <= ?1 ORDER BY notify_at_ms",
    )?;
    let rows = stmt.query_map([now_ms], |r| {
        Ok(Reminder {
            endpoint: r.get(0)?,
            p256dh: r.get(1)?,
            auth: r.get(2)?,
            match_id: r.get(3)?,
            notify_at_ms: r.get(4)?,
            title: r.get(5)?,
            body: r.get(6)?,
            url: r.get(7)?,
            // Not needed for sending.
            game: String::new(),
            league: String::new(),
            team_a: String::new(),
            team_b: String::new(),
            event: String::new(),
        })
    })?;
    rows.collect()
}

pub fn mark_reminder_sent(conn: &Connection, endpoint: &str, match_id: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE reminders SET sent = 1 WHERE endpoint = ?1 AND match_id = ?2",
        params![endpoint, match_id],
    )?;
    Ok(())
}

/// Remove all reminders and subscriptions for a dead endpoint (404/410 push).
pub fn delete_endpoint(conn: &Connection, endpoint: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM reminders WHERE endpoint = ?1", params![endpoint])?;
    conn.execute("DELETE FROM subscriptions WHERE endpoint = ?1", params![endpoint])?;
    Ok(())
}

/// Drop reminders whose notify time is well in the past.
pub fn prune_reminders(conn: &Connection, cutoff_ms: i64) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM reminders WHERE notify_at_ms < ?1",
        params![cutoff_ms],
    )?;
    Ok(())
}

// ----- Game/event subscriptions --------------------------------------------

/// A subscription to a whole game or event (expanded into reminders).
#[derive(Debug, Clone)]
pub struct Subscription {
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
    /// "game" or "league".
    pub scope_kind: String,
    /// "cs2"/"lol" for a game, else the league name.
    pub scope_value: String,
    pub lead_ms: i64,
}

pub fn add_subscription(conn: &Connection, s: &Subscription) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO subscriptions (endpoint, p256dh, auth, scope_kind, scope_value, lead_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(endpoint, scope_kind, scope_value) DO UPDATE SET
            p256dh=excluded.p256dh, auth=excluded.auth, lead_ms=excluded.lead_ms",
        params![s.endpoint, s.p256dh, s.auth, s.scope_kind, s.scope_value, s.lead_ms],
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
        "SELECT endpoint, p256dh, auth, scope_kind, scope_value, lead_ms FROM subscriptions",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(Subscription {
            endpoint: r.get(0)?,
            p256dh: r.get(1)?,
            auth: r.get(2)?,
            scope_kind: r.get(3)?,
            scope_value: r.get(4)?,
            lead_ms: r.get(5)?,
        })
    })?;
    rows.collect()
}

/// On unsubscribe, drop unsent reminders this endpoint got from that scope. A
/// team matches either side; game/league match their single column.
pub fn delete_unsent_reminders_by_scope(
    conn: &Connection,
    endpoint: &str,
    kind: &str,
    value: &str,
) -> rusqlite::Result<()> {
    let where_scope = match kind {
        "game" => "game=?2",
        "team" => "(team_a=?2 OR team_b=?2)",
        "event" => "event=?2",
        _ => "league=?2",
    };
    conn.execute(
        &format!("DELETE FROM reminders WHERE endpoint=?1 AND sent=0 AND {where_scope}"),
        params![endpoint, value],
    )?;
    Ok(())
}

/// Remove everything an endpoint is signed up for — all subscriptions and all
/// (unsent) reminders, including exclusion tombstones. Backs "clear all".
pub fn clear_endpoint(conn: &Connection, endpoint: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM subscriptions WHERE endpoint = ?1", params![endpoint])?;
    conn.execute(
        "DELETE FROM reminders WHERE endpoint = ?1 AND sent = 0",
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
            game: Game::Lol,
            league: "LCK".into(),
            league_url: None,
            serie_name: String::new(),
            tier: "A".into(),
            begin_at,
            status: MatchStatus::Upcoming,
            best_of: Some(3),
            team_a: NormTeam {
                label: "T1".into(),
                name: "T1".into(),
                score: None,
            },
            team_b: NormTeam {
                label: "GEN".into(),
                name: "Gen.G".into(),
                score: Some(1),
            },
            stream_url: Some("https://twitch.tv/lck".into()),
            tournament_id: Some(42),
            venue_tz: Some("America/New_York".into()),
            streams: Vec::new(),
            mlb_series: None,
        }
    }

    #[test]
    fn roundtrip_upsert_and_prune() {
        let mut conn = open(":memory:").unwrap();
        let now = Utc::now();
        let recent = sample(1, now);
        let old = sample(2, now - chrono::Duration::days(10));
        let cutoff = (now - chrono::Duration::days(2)).timestamp_millis();

        upsert_and_prune(&mut conn, &[recent, old], now.timestamp_millis(), cutoff).unwrap();

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
        upsert_and_prune(&mut conn, &[updated], now.timestamp_millis(), cutoff).unwrap();
        let all = load_all(&conn).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].status, MatchStatus::Finished);
        assert_eq!(all[0].team_a.score, Some(2));
    }

    #[test]
    fn purges_rows_under_a_renamed_game_slug() {
        let conn = open(":memory:").unwrap();
        // Simulate a row left behind under a since-renamed slug ("soccer" was
        // renamed to "football"), alongside a valid one. The legacy row would
        // otherwise load as the default game and surface on the esports page.
        conn.execute(
            "INSERT INTO matches (id, game, league, tier, begin_at_ms, status,
                team_a_label, team_b_label) VALUES
                (1, 'soccer', 'World Cup', 'S', 100, 'upcoming', 'Brazil', 'France'),
                (2, 'football', 'World Cup', 'S', 200, 'upcoming', 'Spain', 'Japan')",
            [],
        )
        .unwrap();
        // A direct load skips the unparseable row even before the migration runs.
        assert_eq!(load_all(&conn).unwrap().len(), 1);

        // Re-opening the same DB runs the purge, removing the orphan for good.
        purge_unknown_games(&conn).unwrap();
        let rows = load_all(&conn).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].game, Game::Soccer);
        assert_eq!(rows[0].id, 2);
        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM matches", [], |r| r.get(0))
            .unwrap();
        assert_eq!(remaining, 1, "the legacy 'soccer' row is deleted");
    }

    fn reminder(match_id: i64) -> Reminder {
        Reminder {
            endpoint: "https://push.example/x".into(),
            p256dh: "p".into(),
            auth: "a".into(),
            match_id,
            notify_at_ms: 100,
            title: "T1 vs GEN".into(),
            body: "LCK".into(),
            url: "u".into(),
            game: "lol".into(),
            league: "LCK".into(),
            team_a: "T1".into(),
            team_b: "Gen.G".into(),
            event: "LCK Spring".into(),
        }
    }

    #[test]
    fn exclusion_tombstone_blocks_a_covered_match() {
        let conn = open(":memory:").unwrap();
        let r = reminder(1);
        // A subscription expansion arms it → it's due.
        add_reminder_if_absent(&conn, &r).unwrap();
        assert_eq!(due_reminders(&conn, 1000).unwrap().len(), 1);

        // Opt this one match out → not due, and re-expansion can't re-arm it
        // (insert-if-absent leaves the tombstone).
        exclude_reminder(&conn, &r).unwrap();
        assert_eq!(due_reminders(&conn, 1000).unwrap().len(), 0);
        add_reminder_if_absent(&conn, &r).unwrap();
        assert_eq!(due_reminders(&conn, 1000).unwrap().len(), 0);

        // Re-include it (explicit arm) → due again.
        add_reminder(&conn, &r).unwrap();
        assert_eq!(due_reminders(&conn, 1000).unwrap().len(), 1);
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
}
