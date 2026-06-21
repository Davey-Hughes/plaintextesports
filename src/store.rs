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
use rusqlite::{params, Connection};
use std::path::Path;

const FETCHED_AT_KEY: &str = "fetched_at_ms";

/// Open (creating if needed) the cache DB and ensure the schema exists.
pub fn open(path: &str) -> rusqlite::Result<Connection> {
    if let Some(parent) = Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(parent);
        }
    }
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS matches (
            id           INTEGER NOT NULL,
            game         TEXT    NOT NULL,
            league       TEXT    NOT NULL,
            tier         TEXT    NOT NULL,
            begin_at_ms  INTEGER NOT NULL,
            status       TEXT    NOT NULL,
            best_of      INTEGER,
            team_a_label TEXT    NOT NULL,
            team_a_score INTEGER,
            team_b_label TEXT    NOT NULL,
            team_b_score INTEGER,
            stream_url   TEXT,
            PRIMARY KEY (id, game)
        );
        CREATE INDEX IF NOT EXISTS idx_matches_begin ON matches (begin_at_ms);
        CREATE TABLE IF NOT EXISTS meta (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );",
    )?;
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
        tier: row.get("tier")?,
        begin_at: DateTime::from_timestamp_millis(begin_ms).unwrap_or_else(Utc::now),
        status: MatchStatus::from_str(&status),
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
                 team_a_label, team_a_score, team_b_label, team_b_score, stream_url)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT(id, game) DO UPDATE SET
                league=excluded.league, tier=excluded.tier,
                begin_at_ms=excluded.begin_at_ms, status=excluded.status,
                best_of=excluded.best_of,
                team_a_label=excluded.team_a_label, team_a_score=excluded.team_a_score,
                team_b_label=excluded.team_b_label, team_b_score=excluded.team_b_score,
                stream_url=excluded.stream_url",
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(id: i64, begin_at: DateTime<Utc>) -> NormalizedMatch {
        NormalizedMatch {
            id,
            game: Game::Lol,
            league: "LCK".into(),
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
}
