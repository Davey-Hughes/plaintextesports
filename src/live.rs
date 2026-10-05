//! The live fast lane's pure half (server-only): which rows to poll, how they
//! group into requests, and how a poll's updates land on the snapshot. The task
//! that drives it is `cache::spawn_live_poller`; the fetchers are each source's
//! `fetch_live` (`espn`, `nhl`, `mlb`).

use crate::feed::{LiveUpdate, NormalizedMatch};
use crate::types::{MatchStatus, Sport};
use chrono::{DateTime, Duration, NaiveDate, Utc};
use std::collections::{BTreeSet, HashMap};

/// The sports with a live scoreboard the fast lane polls.
pub const LIVE_SPORTS: [Sport; 4] = [Sport::Nfl, Sport::Nba, Sport::Nhl, Sport::Mlb];

/// Start polling a game this long before it starts, so the opening minutes are
/// caught by the fast lane rather than the next main cycle.
pub const LIVE_LEAD: Duration = Duration::minutes(5);

/// How long a recorded update survives without a refresh. A targeted game is
/// refreshed every tick, so only a finished game (or a source that keeps
/// failing) ages out — after which the main poller's data stands.
pub const LIVE_KEEP: Duration = Duration::minutes(3);

/// One recorded update and when it was fetched.
#[derive(Debug, Clone)]
pub struct LiveEntry {
    pub update: LiveUpdate,
    pub fetched_at: DateTime<Utc>,
}

/// The latest update per game.
pub type LiveMap = HashMap<(Sport, i64), LiveEntry>;

/// One fast-lane request: an ESPN scoreboard day (NFL/NBA), an NHL score day, or
/// one MLB lookup by game id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveRequest {
    Espn(Sport, NaiveDate),
    Nhl(NaiveDate),
    Mlb(Vec<i64>),
}

/// The US-Eastern calendar day of `t` — how ESPN and the NHL file a game: the
/// 8:20 PM ET Sunday night game is Sunday's, though it starts Monday UTC.
#[must_use]
pub fn et_day(t: DateTime<Utc>) -> NaiveDate {
    t.with_timezone(&chrono_tz::America::New_York).date_naive()
}

/// The rows worth a fast poll: a live-lane sport that is live, or has started
/// (within `grace`) without being marked finished, or starts within
/// [`LIVE_LEAD`].
#[must_use]
pub fn live_targets(
    matches: &[NormalizedMatch],
    now: DateTime<Utc>,
    grace: Duration,
) -> Vec<&NormalizedMatch> {
    matches
        .iter()
        .filter(|m| LIVE_SPORTS.contains(&m.sport))
        .filter(|m| match m.status {
            MatchStatus::Live => true,
            MatchStatus::Upcoming => m.begin_at <= now + LIVE_LEAD && now < m.begin_at + grace,
            MatchStatus::Finished | MatchStatus::Canceled => false,
        })
        .collect()
}

/// Group targets into the fewest requests: one per (ESPN league, ET day), one per
/// NHL ET day, and every MLB game in one lookup by id — ESPN first, then NHL,
/// then MLB.
#[must_use]
pub fn plan_requests(targets: &[&NormalizedMatch]) -> Vec<LiveRequest> {
    let mut espn: Vec<(Sport, NaiveDate)> = Vec::new();
    let mut nhl = BTreeSet::new();
    let mut mlb = BTreeSet::new();
    for m in targets {
        match m.sport {
            Sport::Nfl | Sport::Nba => {
                let key = (m.sport, et_day(m.begin_at));
                if !espn.contains(&key) {
                    espn.push(key);
                }
            }
            Sport::Nhl => {
                nhl.insert(et_day(m.begin_at));
            }
            Sport::Mlb => {
                mlb.insert(m.id);
            }
            _ => {}
        }
    }
    let mut out: Vec<LiveRequest> = espn
        .into_iter()
        .map(|(s, d)| LiveRequest::Espn(s, d))
        .collect();
    out.extend(nhl.into_iter().map(LiveRequest::Nhl));
    if !mlb.is_empty() {
        out.push(LiveRequest::Mlb(mlb.into_iter().collect()));
    }
    out
}

/// Put one update on its row. Scores move only when the update carries both, so
/// a source that drops them for a tick can't blank the row; the detail clears
/// once the game isn't live.
pub fn patch(m: &mut NormalizedMatch, u: &LiveUpdate) {
    m.status = u.status;
    if let (Some(a), Some(b)) = (u.score_a, u.score_b) {
        m.team_a.score = Some(a);
        m.team_b.score = Some(b);
    }
    m.live_detail = if u.status == MatchStatus::Live {
        u.detail.clone()
    } else {
        String::new()
    };
}

/// Remember a poll's updates, stamped with when they were fetched.
pub fn record(live: &mut LiveMap, updates: &[LiveUpdate], fetched_at: DateTime<Utc>) {
    for u in updates {
        live.insert(
            (u.sport, u.id),
            LiveEntry {
                update: u.clone(),
                fetched_at,
            },
        );
    }
}

/// Forget updates not refreshed within [`LIVE_KEEP`].
pub fn prune(live: &mut LiveMap, now: DateTime<Utc>) {
    live.retain(|_, e| now - e.fetched_at < LIVE_KEEP);
}

/// Re-apply every update fetched at or after `since` — the main poller's cycle
/// start — onto freshly reloaded rows, so the newest data wins either way: a live
/// score the fast lane saw after the main fetch survives the reload, and a Final
/// the main fetch saw after the last fast poll isn't overwritten by it.
pub fn apply_live(matches: &mut [NormalizedMatch], live: &LiveMap, since: DateTime<Utc>) {
    for m in matches {
        if let Some(e) = live.get(&(m.sport, m.id))
            && e.fetched_at >= since
        {
            patch(m, &e.update);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed::NormalizedTeam;

    const GRACE: Duration = Duration::hours(5);

    fn row(sport: Sport, id: i64, begin: DateTime<Utc>, status: MatchStatus) -> NormalizedMatch {
        let t = |s: &str| NormalizedTeam {
            label: s.into(),
            name: s.into(),
            abbrev: String::new(),
            score: Some(0),
        };
        NormalizedMatch::team_sport(id, sport, "L", begin, status, t("A"), t("B"))
    }

    fn up(sport: Sport, id: i64, status: MatchStatus, a: i64, b: i64, d: &str) -> LiveUpdate {
        LiveUpdate {
            sport,
            id,
            status,
            score_a: Some(a),
            score_b: Some(b),
            detail: d.into(),
        }
    }

    fn at(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }

    #[test]
    fn targets_live_started_and_imminent_rows_of_live_sports_only() {
        let now = Utc::now();
        let rows = [
            row(Sport::Nfl, 1, now - Duration::hours(1), MatchStatus::Live),
            // Started, but the main poller hasn't flipped it to Live yet.
            row(
                Sport::Nhl,
                2,
                now - Duration::minutes(5),
                MatchStatus::Upcoming,
            ),
            row(
                Sport::Mlb,
                3,
                now + Duration::minutes(4),
                MatchStatus::Upcoming,
            ),
            row(
                Sport::Nba,
                4,
                now + Duration::minutes(30),
                MatchStatus::Upcoming,
            ),
            row(
                Sport::Nfl,
                5,
                now - Duration::hours(1),
                MatchStatus::Finished,
            ),
            row(
                Sport::Soccer,
                6,
                now - Duration::minutes(20),
                MatchStatus::Live,
            ),
            row(
                Sport::Nhl,
                7,
                now - Duration::hours(6),
                MatchStatus::Upcoming,
            ),
            row(
                Sport::Mlb,
                8,
                now - Duration::hours(1),
                MatchStatus::Canceled,
            ),
        ];
        let ids: Vec<i64> = live_targets(&rows, now, GRACE)
            .iter()
            .map(|m| m.id)
            .collect();
        assert_eq!(ids, [1, 2, 3]);
    }

    #[test]
    fn et_day_files_a_late_game_under_its_eastern_date() {
        let oct4 = NaiveDate::from_ymd_opt(2026, 10, 4).unwrap();
        // SNF: 8:20 PM EDT on Sunday is 00:20Z Monday.
        assert_eq!(et_day(at("2026-10-05T00:20:00Z")), oct4);
        assert_eq!(et_day(at("2026-10-04T17:00:00Z")), oct4);
    }

    #[test]
    fn plans_one_request_per_espn_day_one_per_nhl_day_and_one_mlb_batch() {
        let (early, late) = (at("2026-10-04T17:00:00Z"), at("2026-10-05T00:20:00Z"));
        let rows = [
            row(Sport::Nfl, 1, early, MatchStatus::Live),
            row(Sport::Nfl, 2, late, MatchStatus::Live),
            row(Sport::Nhl, 3, late, MatchStatus::Live),
            row(Sport::Mlb, 9, late, MatchStatus::Live),
            row(
                Sport::Nba,
                4,
                at("2026-10-06T00:00:00Z"),
                MatchStatus::Upcoming,
            ),
            row(Sport::Mlb, 8, early, MatchStatus::Live),
        ];
        let refs: Vec<&NormalizedMatch> = rows.iter().collect();
        let oct4 = NaiveDate::from_ymd_opt(2026, 10, 4).unwrap();
        let oct5 = NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();
        assert_eq!(
            plan_requests(&refs),
            [
                LiveRequest::Espn(Sport::Nfl, oct4),
                LiveRequest::Espn(Sport::Nba, oct5),
                LiveRequest::Nhl(oct4),
                LiveRequest::Mlb(vec![8, 9]),
            ]
        );
        assert_eq!(plan_requests(&[]), Vec::new());
    }

    #[test]
    fn patch_sets_scores_status_and_detail_then_clears_detail_once_final() {
        let mut m = row(Sport::Nfl, 1, Utc::now(), MatchStatus::Upcoming);
        patch(
            &mut m,
            &up(Sport::Nfl, 1, MatchStatus::Live, 7, 3, "Q2 4:08"),
        );
        assert_eq!(
            (
                m.status,
                m.team_a.score,
                m.team_b.score,
                m.live_detail.as_str()
            ),
            (MatchStatus::Live, Some(7), Some(3), "Q2 4:08")
        );
        patch(
            &mut m,
            &up(Sport::Nfl, 1, MatchStatus::Finished, 14, 10, ""),
        );
        assert_eq!(
            (m.status, m.team_a.score, m.live_detail.as_str()),
            (MatchStatus::Finished, Some(14), "")
        );
    }

    #[test]
    fn patch_keeps_scores_when_the_update_has_none() {
        let mut m = row(Sport::Nhl, 1, Utc::now(), MatchStatus::Live);
        m.team_a.score = Some(2);
        let mut u = up(Sport::Nhl, 1, MatchStatus::Live, 0, 0, "P2 INT");
        u.score_a = None;
        patch(&mut m, &u);
        assert_eq!(
            (m.team_a.score, m.live_detail.as_str()),
            (Some(2), "P2 INT")
        );
    }

    #[test]
    fn apply_live_applies_only_entries_fetched_since_the_cycle_began() {
        let t0 = Utc::now();
        let mut live = LiveMap::new();
        let fresh = up(Sport::Nhl, 1, MatchStatus::Live, 2, 1, "P2 9:14");
        let stale = up(Sport::Nhl, 2, MatchStatus::Live, 0, 0, "P3 1:00");
        record(&mut live, &[fresh], t0 + Duration::seconds(2));
        record(&mut live, &[stale], t0 - Duration::seconds(5));
        let mut rows = [
            row(Sport::Nhl, 1, t0, MatchStatus::Live),
            // The main fetch saw the final after the fast lane's last poll.
            row(Sport::Nhl, 2, t0, MatchStatus::Finished),
            // Same id, different sport: never touched.
            row(Sport::Mlb, 1, t0, MatchStatus::Live),
        ];
        apply_live(&mut rows, &live, t0);
        assert_eq!(
            (rows[0].team_a.score, rows[0].live_detail.as_str()),
            (Some(2), "P2 9:14")
        );
        assert_eq!(rows[1].status, MatchStatus::Finished);
        assert_eq!(
            (rows[2].team_a.score, rows[2].live_detail.as_str()),
            (Some(0), "")
        );
    }

    #[test]
    fn prune_drops_entries_older_than_live_keep() {
        let now = Utc::now();
        let mut live = LiveMap::new();
        let u = |id| up(Sport::Mlb, id, MatchStatus::Live, 1, 1, "Top 4th");
        record(&mut live, &[u(1)], now - LIVE_KEEP - Duration::seconds(1));
        record(&mut live, &[u(2)], now);
        prune(&mut live, now);
        assert_eq!(live.keys().map(|k| k.1).collect::<Vec<_>>(), [2]);
    }
}
