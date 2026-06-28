//! Pure record logic for the spoiler-reveal store. The browser keeps two
//! localStorage values — `revealed` (per-match score reveals) and `sections`
//! (standings/bracket/results reveals) — and historically each was just a set of
//! keys that grew without bound. Now each reveal is a record carrying *when the
//! thing it reveals ends* and *when the user last interacted with it*, so a reveal
//! can be pruned (reverting to hidden) once it is BOTH over a week past that end
//! AND over a week untouched.
//!
//! This module is the pure, side-effect-free core — parse / serialize / prune /
//! upsert — so it compiles on every target and is unit-tested directly. The
//! localStorage glue (and "now") lives in the hydrate-only `app::storage`.

use std::collections::HashSet;

/// A reveal lasts this long past both its entity's end and the last interaction
/// before it's pruned back to hidden (one week, in ms).
pub const REVEAL_TTL_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// One stored reveal: the key (a match uid like "cs2-123" or a section key like
/// "st:678"), the end time of the thing it reveals, and the last-interaction time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub key: String,
    /// When the revealed entity is finished and won't change (ms since epoch).
    /// May be in the future for an in-progress event (which keeps it un-prunable).
    pub end_ms: i64,
    /// When the user last toggled or viewed this reveal (ms since epoch).
    pub touched_ms: i64,
}

/// Parse the stored value (newline-separated) into records. A well-formed line is
/// `key\tend_ms\ttouched_ms`. A legacy line (no tabs — the old key-only format) is
/// grandfathered as `end = 0` (treated as long-ended) and `touched = now`, giving
/// it a fresh one-week grace that ages out unless it's viewed (which re-captures a
/// real end). Blank lines are skipped; an unparseable timestamp falls back the
/// same way a legacy line would.
#[must_use]
pub fn parse_records(raw: &str, now: i64) -> Vec<Record> {
    raw.split('\n')
        .filter(|l| !l.is_empty())
        .map(|line| {
            let mut parts = line.split('\t');
            let key = parts.next().unwrap_or_default().to_string();
            match (parts.next(), parts.next()) {
                (Some(e), Some(t)) => Record {
                    key,
                    end_ms: e.parse().unwrap_or(0),
                    touched_ms: t.parse().unwrap_or(now),
                },
                // Legacy key-only line.
                _ => Record {
                    key,
                    end_ms: 0,
                    touched_ms: now,
                },
            }
        })
        .filter(|r| !r.key.is_empty())
        .collect()
}

/// Serialize records back to the stored value.
#[must_use]
pub fn serialize_records(recs: &[Record]) -> String {
    recs.iter()
        .map(|r| format!("{}\t{}\t{}", r.key, r.end_ms, r.touched_ms))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Drop every record that is BOTH over `ttl` past its end AND over `ttl`
/// untouched. A future or recent end (an in-progress / just-finished event) keeps
/// the record regardless of touch; a recent touch keeps it regardless of end.
#[must_use]
pub fn prune_records(recs: Vec<Record>, now: i64, ttl: i64) -> Vec<Record> {
    recs.into_iter()
        .filter(|r| !(now - r.end_ms > ttl && now - r.touched_ms > ttl))
        .collect()
}

/// Record (or refresh) a reveal: set its end and stamp `touched = now`. Replaces
/// the existing entry for `key` in place, else appends — so the store stays one
/// record per key.
pub fn upsert(recs: &mut Vec<Record>, key: &str, end_ms: i64, now: i64) {
    if let Some(r) = recs.iter_mut().find(|r| r.key == key) {
        r.end_ms = end_ms;
        r.touched_ms = now;
    } else {
        recs.push(Record {
            key: key.to_string(),
            end_ms,
            touched_ms: now,
        });
    }
}

/// Drop a reveal entirely (used when the user hides it).
pub fn remove(recs: &mut Vec<Record>, key: &str) {
    recs.retain(|r| r.key != key);
}

/// The set of keys, for seeding the in-memory reveal context.
#[must_use]
pub fn keys(recs: &[Record]) -> HashSet<String> {
    recs.iter().map(|r| r.key.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TTL: i64 = REVEAL_TTL_MS;
    const DAY: i64 = 24 * 60 * 60 * 1000;

    #[test]
    fn prune_drops_only_when_both_end_and_touch_are_stale() {
        let now = 100 * DAY;
        let recs = vec![
            // Both stale (>1wk past end AND >1wk untouched) → dropped.
            Record {
                key: "stale".into(),
                end_ms: now - 30 * DAY,
                touched_ms: now - 30 * DAY,
            },
            // Past-end but touched recently → kept (you still look at it).
            Record {
                key: "touched".into(),
                end_ms: now - 30 * DAY,
                touched_ms: now - 1 * DAY,
            },
            // Untouched but ended recently → kept (just finished).
            Record {
                key: "recent_end".into(),
                end_ms: now - 1 * DAY,
                touched_ms: now - 30 * DAY,
            },
            // Event still in the future (in progress) → kept regardless of touch.
            Record {
                key: "ongoing".into(),
                end_ms: now + 5 * DAY,
                touched_ms: now - 30 * DAY,
            },
        ];
        let kept = keys(&prune_records(recs, now, TTL));
        assert!(!kept.contains("stale"));
        assert!(kept.contains("touched"));
        assert!(kept.contains("recent_end"));
        assert!(kept.contains("ongoing"));
    }

    #[test]
    fn legacy_key_only_line_is_grandfathered() {
        let now = 100 * DAY;
        // The old format was just keys, newline-joined.
        let recs = parse_records("cs2-1\nst:42", now);
        assert_eq!(recs.len(), 2);
        // end=0 (long-ended) but touched=now → survives this load's prune, and
        // will only age out after a week with no interaction.
        for r in &recs {
            assert_eq!(r.end_ms, 0);
            assert_eq!(r.touched_ms, now);
        }
        let kept = keys(&prune_records(recs, now, TTL));
        assert!(kept.contains("cs2-1") && kept.contains("st:42"));
    }

    #[test]
    fn roundtrip_serialize_then_parse_is_stable() {
        let now = 50 * DAY;
        let mut recs = Vec::new();
        upsert(&mut recs, "cs2-1", now - 2 * DAY, now);
        upsert(&mut recs, "st:42", now + DAY, now);
        let reparsed = parse_records(&serialize_records(&recs), now);
        assert_eq!(reparsed, recs);
    }

    #[test]
    fn upsert_refreshes_and_remove_deletes() {
        let mut recs = Vec::new();
        upsert(&mut recs, "k", 10, 20);
        // Re-touch updates end + touched in place (no duplicate row).
        upsert(&mut recs, "k", 99, 200);
        assert_eq!(recs.len(), 1);
        assert_eq!(
            recs[0],
            Record {
                key: "k".into(),
                end_ms: 99,
                touched_ms: 200
            }
        );
        remove(&mut recs, "k");
        assert!(recs.is_empty());
    }

    #[test]
    fn parse_skips_blanks_and_tolerates_bad_timestamps() {
        let now = 7 * DAY;
        let recs = parse_records("\nk1\t5\t6\n\nk2\tbad\t9\n", now);
        assert_eq!(recs.len(), 2);
        assert_eq!(
            recs[0],
            Record {
                key: "k1".into(),
                end_ms: 5,
                touched_ms: 6
            }
        );
        // A non-numeric end falls back to 0 (and touched parses fine here).
        assert_eq!(
            recs[1],
            Record {
                key: "k2".into(),
                end_ms: 0,
                touched_ms: 9
            }
        );
    }
}
