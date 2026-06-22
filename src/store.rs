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
            tier         TEXT    NOT NULL,
            begin_at_ms  INTEGER NOT NULL,
            status       TEXT    NOT NULL,
            best_of      INTEGER,
            team_a_label TEXT    NOT NULL,
            team_a_score INTEGER,
            team_b_label TEXT    NOT NULL,
            team_b_score INTEGER,
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
            sent         INTEGER NOT NULL DEFAULT 0,
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
    let _ = conn.execute("ALTER TABLE reminders ADD COLUMN game TEXT NOT NULL DEFAULT ''", []);
    let _ = conn.execute("ALTER TABLE reminders ADD COLUMN league TEXT NOT NULL DEFAULT ''", []);
    Ok(conn)
}

fn row_to_match(row: &rusqlite::Row) -> rusqlite::Result<NormalizedMatch> {
    let game: String = row.get("game")?;
    let begin_ms: i64 = row.get("begin_at_ms")?;
    let status: String = row.get("status")?;
    Ok(NormalizedMatch {
        id: row.get("id")?,
        game: Game::from_filter(&game).unwrap_or(Game::Cs2),
        league: row.get("league")?,
        league_url: row.get("league_url")?,
        tier: row.get("tier")?,
        begin_at: DateTime::from_timestamp_millis(begin_ms).unwrap_or_else(Utc::now),
        status: MatchStatus::from_db(&status),
        best_of: row.get("best_of")?,
        team_a: NormTeam {
            label: row.get("team_a_label")?,
            score: row.get("team_a_score")?,
        },
        team_b: NormTeam {
            label: row.get("team_b_label")?,
            score: row.get("team_b_score")?,
        },
        stream_url: row.get("stream_url")?,
        tournament_id: row.get("tournament_id")?,
        // Streams aren't persisted (in-memory only); repopulated on next poll.
        streams: Vec::new(),
    })
}

/// Load all stored matches, ordered by start time.
pub fn load_all(conn: &Connection) -> rusqlite::Result<Vec<NormalizedMatch>> {
    let mut stmt = conn.prepare("SELECT * FROM matches ORDER BY begin_at_ms ASC")?;
    let rows = stmt.query_map([], row_to_match)?;
    rows.collect()
}

/// Read the stored "last fetched" timestamp (unix ms), if any.
pub fn load_fetched_at(conn: &Connection) -> Option<i64> {
    conn.query_row(
        "SELECT value FROM meta WHERE key = ?1",
        params![FETCHED_AT_KEY],
        |r| r.get::<_, String>(0),
    )
    .ok()
    .and_then(|s| s.parse().ok())
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
                 league_url, tournament_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
             ON CONFLICT(id, game) DO UPDATE SET
                league=excluded.league, tier=excluded.tier,
                begin_at_ms=excluded.begin_at_ms, status=excluded.status,
                best_of=excluded.best_of,
                team_a_label=excluded.team_a_label, team_a_score=excluded.team_a_score,
                team_b_label=excluded.team_b_label, team_b_score=excluded.team_b_score,
                stream_url=excluded.stream_url, league_url=excluded.league_url,
                tournament_id=excluded.tournament_id",
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
    /// Game/league the match belongs to (for scope-based cleanup).
    pub game: String,
    pub league: String,
}

const REMINDER_COLS: &str =
    "endpoint, p256dh, auth, match_id, notify_at_ms, title, body, url, game, league";

fn reminder_params(r: &Reminder) -> [&dyn rusqlite::ToSql; 10] {
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
    ]
}

/// Add or update a reminder (re-arms `sent`). Used for an explicit star.
pub fn add_reminder(conn: &Connection, r: &Reminder) -> rusqlite::Result<()> {
    conn.execute(
        &format!(
            "INSERT INTO reminders ({REMINDER_COLS}, sent)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0)
             ON CONFLICT(endpoint, match_id) DO UPDATE SET
                p256dh=excluded.p256dh, auth=excluded.auth,
                notify_at_ms=excluded.notify_at_ms, title=excluded.title,
                body=excluded.body, url=excluded.url, game=excluded.game,
                league=excluded.league, sent=0"
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
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0)
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
         FROM reminders WHERE sent = 0 AND notify_at_ms <= ?1 ORDER BY notify_at_ms",
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

/// On unsubscribe, drop unsent reminders this endpoint got from that scope.
pub fn delete_unsent_reminders_by_scope(
    conn: &Connection,
    endpoint: &str,
    kind: &str,
    value: &str,
) -> rusqlite::Result<()> {
    let col = if kind == "game" { "game" } else { "league" };
    conn.execute(
        &format!("DELETE FROM reminders WHERE endpoint=?1 AND sent=0 AND {col}=?2"),
        params![endpoint, value],
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
            tier: "A".into(),
            begin_at,
            status: MatchStatus::Upcoming,
            best_of: Some(3),
            team_a: NormTeam {
                label: "T1".into(),
                score: None,
            },
            team_b: NormTeam {
                label: "GEN".into(),
                score: Some(1),
            },
            stream_url: Some("https://twitch.tv/lck".into()),
            tournament_id: Some(42),
            streams: Vec::new(),
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
