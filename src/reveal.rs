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

/// The reveal key for a whole event, from its canonical path (what
/// [`crate::app::helpers::event_path`] builds — and what a match page already
/// links to). Keying on the path rather than a bespoke derivation is what lets the
/// event page and the match pages under it share one record without two
/// derivations that could drift apart.
#[must_use]
pub fn page_reveal_key(event_path: &str) -> String {
    format!("pg:{event_path}")
}

/// Record (or refresh) a reveal: keep the later end and stamp `touched = now`.
/// Replaces the existing entry for `key` in place, else appends — so the store
/// stays one record per key.
///
/// The end is a max, not an overwrite, because one record can be touched from
/// places that know different ends: an event-scoped reveal gets the event's last
/// match from the event page, but only that match's own time from a match page
/// under it. Taking the later keeps the end from regressing (which would expose
/// the record to early pruning). The cost is that an event rescheduled *earlier*
/// keeps its stale later end — that only delays pruning, which is the safe way to
/// be wrong.
pub fn upsert(recs: &mut Vec<Record>, key: &str, end_ms: i64, now: i64) {
    if let Some(r) = recs.iter_mut().find(|r| r.key == key) {
        r.end_ms = r.end_ms.max(end_ms);
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

/// Drop every reveal whose key is in `keys` or starts with one of `prefixes`.
///
/// This is the cascade behind an event's switch going off: it takes the localized
/// reveals under it with it, so "off" means off for the whole page rather than
/// leaving individually-revealed items still showing. `prefixes` covers the
/// brackets, whose per-round keys (`bn:<tid>:…`) are generated inside the grid.
/// One pass over the store, rather than a read-modify-write per key.
pub fn remove_many(recs: &mut Vec<Record>, keys: &HashSet<String>, prefixes: &[String]) {
    recs.retain(|r| !(keys.contains(&r.key) || prefixes.iter().any(|p| r.key.starts_with(p))));
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
                touched_ms: now - DAY,
            },
            // Untouched but ended recently → kept (just finished).
            Record {
                key: "recent_end".into(),
                end_ms: now - DAY,
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
    fn upsert_keeps_the_later_end() {
        // An event-scoped record is written from two places that know different
        // ends: the event page knows the event's last match, a match page under it
        // only knows its own (earlier) match. Touching from the match page must not
        // drag the end backwards and expose the record to early pruning.
        let mut recs = Vec::new();
        upsert(&mut recs, "pg:/event/cs2/Katowice", 100 * DAY, 10 * DAY);
        upsert(&mut recs, "pg:/event/cs2/Katowice", 20 * DAY, 11 * DAY);
        assert_eq!(
            recs[0],
            Record {
                key: "pg:/event/cs2/Katowice".into(),
                // The event's end stands; only the touch advances.
                end_ms: 100 * DAY,
                touched_ms: 11 * DAY,
            }
        );
    }

    #[test]
    fn remove_many_drops_exact_keys_and_prefix_matches_only() {
        // An event switch going off takes the localized reveals under it with it:
        // the exact keys its gates announced, plus whole brackets by prefix. What it
        // must not touch is anything belonging to another event.
        let mut recs = Vec::new();
        for k in [
            "cs2-1",          // a row on the page
            "cs2-2",          // a row on another page
            "st:77",          // this event's standings
            "st:99",          // another event's standings
            "bn:77:0:0",      // this event's bracket
            "bs:77:1:2",      // this event's bracket
            "bn:99:0:0",      // another event's bracket
            "boxscore:cs2-1", // a section on the page
        ] {
            upsert(&mut recs, k, 0, 0);
        }
        let keys: HashSet<String> = ["cs2-1", "st:77", "boxscore:cs2-1"]
            .into_iter()
            .map(String::from)
            .collect();
        let prefixes = vec!["bn:77:".to_string(), "bs:77:".to_string()];
        remove_many(&mut recs, &keys, &prefixes);
        let left = keys_vec(&recs);
        // Everything this page announced is gone…
        for gone in ["cs2-1", "st:77", "boxscore:cs2-1", "bn:77:0:0", "bs:77:1:2"] {
            assert!(
                !left.contains(&gone.to_string()),
                "{gone} should be cleared"
            );
        }
        // …and nothing else is.
        for kept in ["cs2-2", "st:99", "bn:99:0:0"] {
            assert!(left.contains(&kept.to_string()), "{kept} should survive");
        }
    }

    #[test]
    fn remove_many_with_nothing_registered_is_a_no_op() {
        let mut recs = Vec::new();
        upsert(&mut recs, "cs2-1", 0, 0);
        remove_many(&mut recs, &HashSet::new(), &[]);
        assert_eq!(keys_vec(&recs), vec!["cs2-1".to_string()]);
    }

    fn keys_vec(recs: &[Record]) -> Vec<String> {
        recs.iter().map(|r| r.key.clone()).collect()
    }

    #[test]
    fn page_key_is_prefixed_and_scoped_per_event() {
        // The key is the event's canonical path (what `event_path` builds and the
        // match page already links to), so it can't drift from the event it names.
        assert_eq!(
            page_reveal_key("/event/cs2/IEM%20Katowice%202026"),
            "pg:/event/cs2/IEM%20Katowice%202026"
        );
        // Different editions, and the same edition in different sports, don't share
        // a reveal.
        assert_ne!(
            page_reveal_key("/event/cs2/IEM%20Katowice%202026"),
            page_reveal_key("/event/cs2/BLAST%20Bounty%202026")
        );
        assert_ne!(
            page_reveal_key("/event/cs2/Masters"),
            page_reveal_key("/event/lol/Masters")
        );
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
