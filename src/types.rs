//! Wire/view types shared between the server and the browser (WASM).
//!
//! These are plain `serde` structs with no server-only dependencies (no
//! `chrono`, no `reqwest`) so they compile cheaply into the WASM bundle. All
//! display formatting (times, day labels) is done server-side, so the client
//! only ever renders ready-to-display strings.

use serde::{Deserialize, Serialize};

/// Default forward window (days from today) for a traditional-sports schedule.
/// They play daily, so the homepage shows a short window by default and a
/// "show later days" control extends it up to [`TRAD_FORWARD_MAX`].
pub const TRAD_FORWARD_DAYS: i64 = 2;
/// Never show traditional-sports games more than this many days ahead.
pub const TRAD_FORWARD_MAX: i64 = 7;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Sport {
    #[default]
    Cs2,
    Lol,
    /// Major League Baseball — a traditional sport, fetched from the MLB Stats
    /// API rather than PandaScore. Lives behind the esports/traditional toggle.
    Mlb,
    /// National Hockey League — a traditional sport, fetched from the keyless
    /// NHL Web API (`api-web.nhle.com`). Like MLB: two teams per sport.
    Nhl,
    /// National Basketball Association — a traditional sport, fetched from the
    /// keyless ESPN API. Two teams per sport.
    Nba,
    /// National Football League — a traditional sport, fetched from the keyless
    /// ESPN API. Two teams per sport.
    Nfl,
    /// Soccer — a traditional sport, fetched from the keyless ESPN API. Covers
    /// more than one competition (the Premier League and the World Cup), each its
    /// own `league`. Two teams per sport; matches can draw.
    Soccer,
    /// Motorsport — one category covering several series via the `league` field:
    /// F1 (Jolpica/Ergast), WRC + WEC (Orange Cat Blacktop). A "match" is a single
    /// session/round (an F1 session, a WEC session, or a WRC rally); the event is
    /// the Grand Prix / race meeting / rally. Single-entity (no opponent). The
    /// series are the sub-league chips, like soccer's competitions.
    Motorsport,
}

impl Sport {
    /// Every sport, in canonical (display) order. The single source of truth for
    /// "which sport slugs are valid" — drives the DB cleanup migration (purging
    /// rows written under a since-renamed slug) and exhaustiveness tests.
    pub const ALL: &'static [Sport] = &[
        Self::Cs2,
        Self::Lol,
        Self::Mlb,
        Self::Nhl,
        Self::Nba,
        Self::Nfl,
        Self::Soccer,
        Self::Motorsport,
    ];

    /// Display name. Traditional sports use the sport (not the league), so the
    /// top-level filter reads "Baseball/Hockey/…" rather than "MLB/NHL/…"; the
    /// league name itself comes from the match data. American football is
    /// "Football"; the other code uses "Soccer" for the international sport.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Cs2 => "CS2",
            Self::Lol => "LoL",
            Self::Mlb => "Baseball",
            Self::Nhl => "Hockey",
            Self::Nba => "Basketball",
            Self::Nfl => "Football",
            Self::Soccer => "Soccer",
            // The category is motorsport; the series (F1, …) is its sub-filter.
            Self::Motorsport => "Motorsport",
        }
    }

    #[must_use]
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Cs2 => "cs2",
            Self::Lol => "lol",
            Self::Mlb => "mlb",
            Self::Nhl => "nhl",
            Self::Nba => "nba",
            Self::Nfl => "nfl",
            Self::Soccer => "football",
            Self::Motorsport => "motorsport",
        }
    }

    /// A traditional sport (vs an esports title) — drives the top-bar mode toggle.
    #[must_use]
    pub const fn traditional(self) -> bool {
        matches!(
            self,
            Self::Mlb | Self::Nhl | Self::Nba | Self::Nfl | Self::Soccer | Self::Motorsport
        )
    }

    /// A single-entity sport (a race/session, not two opposing teams) — the row
    /// shows one competitor label rather than "A vs B".
    #[must_use]
    pub const fn single_entity(self) -> bool {
        matches!(self, Self::Motorsport)
    }

    /// Whether this sport's leagues are a genuine sub-categorisation worth a
    /// filter chip even when only one is present — soccer's competitions (Premier
    /// League / World Cup) and motorsport's series (F1, …). For the other sports
    /// the sole league just repeats the tab, so no chip.
    #[must_use]
    pub const fn has_sub_leagues(self) -> bool {
        matches!(self, Self::Soccer | Self::Motorsport)
    }

    /// The traditional sports, in display order — drives the in-mode sport tabs.
    #[must_use]
    pub fn traditional_sports() -> &'static [Sport] {
        &[
            Self::Mlb,
            Self::Nhl,
            Self::Nba,
            Self::Nfl,
            Self::Soccer,
            Self::Motorsport,
        ]
    }

    /// The label for the standings table's last column: a map/game record for
    /// esports, or the team sports' single ranking figure (games-back / points /
    /// win%). Empty for F1 (it has no standings table).
    #[must_use]
    pub const fn standings_last_label(self) -> &'static str {
        match self {
            Self::Lol => "Games",
            Self::Cs2 => "Maps",
            Self::Mlb | Self::Nba => "GB",
            Self::Nhl | Self::Soccer => "PTS",
            Self::Nfl => "PCT",
            Self::Motorsport => "",
        }
    }

    /// Whether the standings' last column holds a single ranking value (games-back
    /// / points / win%) rather than a map/game win-loss record. True for the team
    /// sports, false for esports.
    #[must_use]
    pub const fn standings_single_value(self) -> bool {
        matches!(
            self,
            Self::Mlb | Self::Nhl | Self::Nba | Self::Nfl | Self::Soccer
        )
    }

    /// Parse a UI filter slug. Unknown values (incl. "all") map to `None`.
    #[must_use]
    pub fn from_filter(s: &str) -> Option<Self> {
        match s {
            "cs2" => Some(Self::Cs2),
            "lol" => Some(Self::Lol),
            "mlb" => Some(Self::Mlb),
            "nhl" => Some(Self::Nhl),
            "nba" => Some(Self::Nba),
            "nfl" => Some(Self::Nfl),
            "football" => Some(Self::Soccer),
            // "f1" kept for back-compat with saved filters/links from when the
            // motorsport sport's slug was "f1" (now "motorsport").
            "motorsport" | "f1" => Some(Self::Motorsport),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatchStatus {
    /// Default — also the `from_db` fallback for an unknown value.
    #[default]
    Upcoming,
    Live,
    Finished,
    Canceled,
}

impl MatchStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Upcoming => "upcoming",
            Self::Live => "live",
            Self::Finished => "finished",
            Self::Canceled => "canceled",
        }
    }

    /// Short status badge shown on a schedule/series row ("" while upcoming, so
    /// no badge is drawn).
    #[must_use]
    pub const fn badge(self) -> &'static str {
        match self {
            Self::Live => "LIVE",
            Self::Finished => "Final",
            Self::Canceled => "Canc.",
            Self::Upcoming => "",
        }
    }

    /// CSS class for a schedule/series row in this status (styles the badge + row
    /// emphasis). Matches the `MatchStatus` variants by intent.
    #[must_use]
    pub const fn row_class(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Finished => "final",
            Self::Canceled => "canceled",
            Self::Upcoming => "upcoming",
        }
    }

    /// Parse the string written by [`as_str`](Self::as_str) back (used when
    /// loading from the cache DB). Unknown values default to `Upcoming`. Named
    /// `from_db` rather than `from_str` to avoid shadowing the `FromStr` trait.
    #[must_use]
    pub fn from_db(s: &str) -> Self {
        match s {
            "live" => Self::Live,
            "finished" => Self::Finished,
            "canceled" => Self::Canceled,
            _ => Self::Upcoming,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamView {
    /// Short label shown in the box (acronym if available, else name).
    pub label: String,
    /// Full team name — keys the team page + per-team subscription (empty/"TBD"
    /// when the opponent isn't decided). `#[serde(default)]` so older cached
    /// payloads without it still deserialize.
    #[serde(default)]
    pub name: String,
    pub score: Option<i64>,
    /// True when this team won a finished match (for emphasis).
    pub winner: bool,
    /// Vector (SVG) logo URL, when a vector source exists (NHL/MLB); empty
    /// otherwise. Shown on the match/team pages. `#[serde(default)]` for older
    /// cached payloads.
    #[serde(default)]
    pub logo: String,
    /// Official 2-3 letter abbreviation, shown in place of the label in compact
    /// contexts (narrow schedule rows). Empty when the source has none.
    #[serde(default)]
    pub abbrev: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchView {
    pub id: i64,
    pub sport: Sport,
    /// Short league name, e.g. "LCK" or "IEM".
    pub league: String,
    /// The series/edition name within the league (e.g. "Cologne Major"), empty
    /// when the league name is already the full event name. Combined with
    /// `league` to title the event/match pages (e.g. "IEM Cologne Major").
    #[serde(default)]
    pub series_name: String,
    pub status: MatchStatus,
    /// Start time in the display tz, e.g. "6:00 PM" (always the clock time; the
    /// live/final state is conveyed separately by `status`).
    pub clock_label: String,
    /// Start date in the display tz, e.g. "Wednesday, June 24" — shown on the
    /// detail page, where there's no day heading to carry it. `#[serde(default)]`
    /// so older cached payloads still load.
    #[serde(default)]
    pub date_label: String,
    /// The same start time at the event's venue (e.g. "6:00 PM EDT"), shown when
    /// the user clicks the time. Empty when the venue timezone is unknown
    /// (esports). `#[serde(default)]` so older cached payloads still load.
    #[serde(default)]
    pub venue_label: String,
    /// The venue's name (e.g. "loanDepot park") and its city/country (e.g.
    /// "Miami, FL"), shown on the match page. Empty for esports (no venue).
    #[serde(default)]
    pub venue_name: String,
    #[serde(default)]
    pub venue_location: String,
    /// e.g. "Bo3"; empty when unknown.
    pub best_of: String,
    pub team_a: TeamView,
    pub team_b: TeamView,
    /// The source's official league/event URL (often empty). Carried so
    /// [`group_days`] can resolve each `LeagueGroup`'s event link once from its
    /// first row, instead of every row paying for the resolve. Never read per-row
    /// on the client, so it's skipped on the wire.
    #[serde(skip)]
    pub league_url: String,
    pub begin_at_ms: i64,
    /// When set, the whole row links here instead of its own `/match` page. The
    /// WRC schedule's collapsed "Day N" rows use it to jump to that day on the
    /// rally's event page, where the day's individual stages are listed.
    /// `#[serde(default)]` so older cached payloads still load.
    #[serde(default)]
    pub row_href: Option<String>,
}

impl MatchView {
    /// This match's globally-unique id (see [`match_uid`]).
    #[must_use]
    pub fn uid(&self) -> String {
        match_uid(self.sport, self.id)
    }
}

/// A globally-unique match identifier, `"{slug}-{id}"` (e.g. `"cs2-1494586"`,
/// `"mlb-822869"`). Source match ids aren't unique across games/sources (an ESPN
/// event id can equal an MLB gamePk), so identity is keyed by `(sport, id)`. Sport
/// slugs contain no `-` and ids are numeric, so the uid splits cleanly. Used in
/// the `/match/{uid}` route, the client reveal/star sets + DOM ids, the
/// notifications round-trip, and reminder keys.
#[must_use]
pub fn match_uid(sport: Sport, id: i64) -> String {
    format!("{}-{}", sport.slug(), id)
}

/// Parse a [`match_uid`] back into its sport + source id; `None` if malformed or
/// the slug is unknown.
#[must_use]
pub fn parse_match_uid(uid: &str) -> Option<(Sport, i64)> {
    let (slug, id) = uid.split_once('-')?;
    Some((Sport::from_filter(slug)?, id.parse().ok()?))
}

/// The detail-page path for a match, `"/match/{slug}/{id}"` (e.g.
/// `"/match/mlb/824255"`). The route scopes the sport into its own segment; the
/// internal cross-page key is still the `-`-joined [`match_uid`].
#[must_use]
pub fn match_path(sport: Sport, id: i64) -> String {
    format!("/match/{}/{}", sport.slug(), id)
}

/// The display name for an event: the league plus its series/edition, e.g.
/// ("IEM", "Cologne Major") -> "IEM Cologne Major". An empty series returns the
/// league alone (league seasons already name themselves). Also the key for the
/// `/event` page, so each edition is addressed distinctly.
#[must_use]
pub fn full_event_name(league: &str, series: &str) -> String {
    if series.is_empty() {
        league.to_string()
    } else {
        format!("{league} {series}")
    }
}

/// Whether `full_event_name(league, series) == target`, without allocating — used
/// to filter the snapshot by edition on every event request.
#[must_use]
pub fn event_name_eq(league: &str, series: &str, target: &str) -> bool {
    if series.is_empty() {
        target == league
    } else {
        target
            .strip_prefix(league)
            .and_then(|rest| rest.strip_prefix(' '))
            .is_some_and(|rest| rest == series)
    }
}

/// The best-fit noun for a tier-2 competition, for display (the subscription tag).
/// The `league` field is a mix of true leagues (MLB, LCK, "… Pro League"),
/// knockout tournaments (World Cup, IEM, the invitationals/majors), and racing
/// championships (F1, WRC, WEC) — "league" alone mislabels the latter two. We only
/// have the name here (not the `Sport`), so this is name-based: exact for the
/// fixed sports/motorsport names, keyword heuristics for the open-ended esports
/// ones (where an occasional miss is acceptable). Returns "league" by default.
#[must_use]
pub fn competition_kind(league: &str) -> &'static str {
    // Racing championships go by "series" (and "WEC"/"WRC" must match before the
    // "championship" keyword below, which their full names would otherwise hit).
    if matches!(league, "F1" | "MotoGP" | "WRC" | "WEC") {
        return "series";
    }
    // Knockout/standalone competitions: cups, invitationals, majors, masters,
    // championships, and Intel Extreme Masters ("IEM"). A domestic "… League"
    // (Premier League, XSE Pro League) has none of these, so it stays a league.
    const TOURNAMENT_WORDS: &[&str] = &[
        "cup",
        "invitational",
        "major",
        "masters",
        "championship",
        "iem",
    ];
    let lower = league.to_ascii_lowercase();
    if TOURNAMENT_WORDS.iter().any(|w| lower.contains(w)) {
        return "tournament";
    }
    "league"
}

/// Matches for one league/event within a day.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeagueGroup {
    pub league: String,
    /// The series/edition name within the league (e.g. "Cologne Major"); empty
    /// for league seasons. Combined with `league` for the header + event link.
    #[serde(default)]
    pub series_name: String,
    /// Event-page link shown on the league header (see `MatchView::event_url`).
    pub event_url: String,
    /// Best-of label shown in the header when uniform across the group (e.g.
    /// "Bo3"); `None` when matches mix formats (then shown per-row).
    pub bo: Option<String>,
    pub matches: Vec<MatchView>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DayGroup {
    /// ISO date in the display timezone, e.g. "2026-06-21".
    pub day_key: String,
    /// Human label, e.g. "Sunday, June 21".
    pub day_label: String,
    pub leagues: Vec<LeagueGroup>,
}

/// A footer link (e.g. GitHub, social media).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteLink {
    pub label: String,
    pub url: String,
}

/// Footer/site info pulled from config.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteInfo {
    pub copyright: Option<String>,
    /// When set, the copyright line links to this URL.
    #[serde(default)]
    pub copyright_url: Option<String>,
    pub links: Vec<SiteLink>,
}

/// A browser Web Push subscription (from `PushSubscription.toJSON()`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushSub {
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
}

/// A request to be reminded about a match (sent from the browser when starred).
/// Only the push subscription + match id are sent; the server derives the
/// notification's time/title/body/url from its snapshot (it doesn't trust the
/// client for those).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReminderReq {
    pub sub: PushSub,
    pub match_id: i64,
    /// The match's sport slug, so the server resolves the right match (ids aren't
    /// unique across games) and keys the reminder by `(match_id, game)`.
    #[serde(default)]
    pub sport: String,
    /// Effective lead offsets (ms before start), resolved client-side: the match's
    /// own override else the global timer list. One reminder is armed per offset.
    /// Empty (old client / serde default) ⇒ the server applies the config single
    /// lead, preserving the prior single-reminder behaviour.
    #[serde(default)]
    pub leads: Vec<i64>,
    /// The viewer's IANA timezone (e.g. "America/New_York"), so the server bakes
    /// the notification body's start time in the viewer's zone rather than its own
    /// configured display tz. Empty (old client / serde default) ⇒ the server tz.
    #[serde(default)]
    pub tz: String,
    /// The viewer's 12h/24h preference, so the body's start time matches the rest
    /// of the UI. `false` (old client / serde default) ⇒ 12-hour.
    #[serde(default)]
    pub hour24: bool,
}

/// Subscribe/unsubscribe to a whole sport or event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubscribeReq {
    pub sub: PushSub,
    /// "sport" or "league".
    pub kind: String,
    /// "cs2"/"lol" for a sport, else the league name.
    pub value: String,
    /// Effective lead offsets (ms) for this scope: its override else the global
    /// list. Empty ⇒ the server falls back to the config single lead.
    #[serde(default)]
    pub leads: Vec<i64>,
    /// The viewer's IANA timezone, stored with the subscription so the poller's
    /// expansion bakes each match's reminder body in the viewer's zone. Empty ⇒
    /// the server's display tz.
    #[serde(default)]
    pub tz: String,
    /// The viewer's 12h/24h preference, stored with the subscription so the
    /// poller's expansion formats each body's time to match. `false` ⇒ 12-hour.
    #[serde(default)]
    pub hour24: bool,
}

/// The notifications page's starred-match data: the still-upcoming ones to show,
/// plus the uids the server knows are finished/started so the client can silently
/// drop them (so a finished match neither shows nor lingers in the export).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationsView {
    pub upcoming: Vec<MatchView>,
    pub stale: Vec<String>,
}

/// One broadcast/stream for a match. Esports entries (from PandaScore
/// `streams_list`) carry a clickable `url`; MLB TV/radio broadcasts carry no
/// link, just a network `name` and a pre-formatted `tag` (e.g. "national · TV").
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamView {
    pub url: String,
    /// ISO-639-1 language code (e.g. "en"), or empty if unknown.
    pub language: String,
    pub official: bool,
    pub main: bool,
    /// Display label for a link-less broadcast (the network name). Empty for
    /// esports streams, whose label is derived from the `url`.
    #[serde(default)]
    pub name: String,
    /// Pre-formatted tag for a link-less broadcast (e.g. "national · TV"). Empty
    /// for esports streams, whose tags are derived from language/main/official.
    #[serde(default)]
    pub tag: String,
    /// Grouping key for the broadcast list, so kinds are visually separated:
    /// "official"/"costream" for esports, "tv"/"streaming"/"radio" for MLB. The
    /// producers emit entries already ordered by group.
    #[serde(default)]
    pub group: String,
    /// True when this stream's Twitch channel is live right now (esports only;
    /// set request-time by the Twitch enrichment). `#[serde(default)]` for
    /// back-compat with cached payloads.
    #[serde(default)]
    pub live: bool,
    /// Current viewer count when `live`. `None` otherwise.
    #[serde(default)]
    pub viewers: Option<u64>,
}

/// The best external "view this game" link for a match: a precise per-game page
/// (ESPN gamecast, MLB Gameday) when we can build one from the match id, else the
/// event/series page. `label` is the human host name shown in the anchor (e.g.
/// "ESPN", "Liquipedia").
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceLink {
    pub url: String,
    pub label: String,
}

/// One row of a group-stage / Swiss standings table.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StandingRow {
    pub rank: i32,
    pub team: String,
    pub wins: i32,
    pub losses: i32,
    pub ties: i32,
    /// Maps/games won and lost (the "map diff"); zeroed when unavailable.
    pub game_wins: i32,
    pub game_losses: i32,
    /// Games back from the division leader (MLB), e.g. "3.5" or "-" for the
    /// leader. Empty for esports standings, which don't use it.
    #[serde(default)]
    pub gb: String,
}

/// One match within a bracket round.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BracketMatch {
    pub team_a: String,
    pub team_b: String,
    pub score_a: Option<i64>,
    pub score_b: Option<i64>,
    /// Which side won, if decided: "a", "b", or empty.
    pub winner: String,
    /// PandaScore match id, for linking to the match detail page. 0 when unknown
    /// (e.g. demo data); the bracket suppresses the link in that case.
    #[serde(default)]
    pub match_id: i64,
    /// The `(round_index, match_index)` of the matches whose winners/losers feed
    /// this one (0 entries = a first-round match). Used to auto-reveal a series'
    /// lineup once its feeders' scores are revealed — works for single- and
    /// double-elimination alike. Each feeder is in a strictly earlier round.
    #[serde(default)]
    pub feeders: Vec<(usize, usize)>,
}

/// One match within a Swiss-stage bracket (a "first to N" group stage rendered
/// as a buchholz grid rather than a tree).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwissMatch {
    pub team_a: String,
    pub team_b: String,
    pub score_a: Option<i64>,
    pub score_b: Option<i64>,
    /// Which side won, if decided: "a", "b", or empty.
    pub winner: String,
    pub match_id: i64,
    /// The final stage record (e.g. "3-0" advanced, "0-3" eliminated) for a side
    /// that finished the Swiss in this match; empty while the team is still in.
    #[serde(default)]
    pub a_record: String,
    #[serde(default)]
    pub b_record: String,
    /// The `match_id` of each side's previous-round match (the "feeder" that put
    /// it at this record), or `None` in round 1. Progressive reveal gates a
    /// side's name on its feeder's score being shown — exactly like the playoff
    /// bracket — so you can't peek ahead without revealing the earlier rounds.
    #[serde(default)]
    pub a_feeder: Option<i64>,
    #[serde(default)]
    pub b_feeder: Option<i64>,
}

/// A record bucket within a Swiss round — the teams that entered with the same
/// W-L (e.g. all the 2-0 teams), playing each other.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwissBucket {
    /// The shared entering record, e.g. "2-0".
    pub record: String,
    pub matches: Vec<SwissMatch>,
}

/// One round (column) of a Swiss stage, its buckets ordered most-wins first.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwissRound {
    pub number: u32,
    pub buckets: Vec<SwissBucket>,
}

/// A column of the elimination bracket (e.g. "Quarterfinals").
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BracketRound {
    pub title: String,
    pub matches: Vec<BracketMatch>,
    /// Which half of a double-elimination bracket this column belongs to:
    /// "upper", "lower", "final" (grand final), or "" for a single-elimination
    /// bracket. Drives the upper/lower visual grouping.
    #[serde(default)]
    pub section: String,
}

/// Standings + bracket for one event/tournament.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventInfo {
    pub event: String,
    /// PandaScore tournament id — the stable key for sharing per-round reveal
    /// state across the pages that show this same bracket.
    pub tournament_id: i64,
    /// The stage/tournament name within the event (e.g. "Stage 1", "Playoffs"),
    /// so a multi-stage event can label each stage. Empty for single-stage events.
    #[serde(default)]
    pub stage: String,
    /// Which sport this event is, so standings can use sport-appropriate labels.
    pub sport: Sport,
    pub standings: Vec<StandingRow>,
    pub rounds: Vec<BracketRound>,
    /// Swiss-stage rounds (buchholz grid), when this stage is a Swiss/group
    /// stage; empty otherwise. Paired with `standings` (the same stage), so the
    /// UI can toggle between the grid and the ranking table.
    #[serde(default)]
    pub swiss: Vec<SwissRound>,
}

impl EventInfo {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.standings.is_empty() && self.rounds.is_empty() && self.swiss.is_empty()
    }
}

/// MLB-only handle for fetching the whole series between the two teams: the two
/// teams' MLB-Stats-API ids and this game's 1-based position within its series.
/// Kept on the in-memory match (not serialized to the client, not persisted);
/// the detail server fn uses it to fetch the series at request time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MlbSeriesRef {
    /// MLB Stats API team ids for the home/away sides (order doesn't matter — the
    /// fetch uses one as `teamId` and the other as `opponentId`).
    pub team_id_a: i64,
    pub team_id_b: i64,
    /// This game's 1-based number within its series, and the series length, so the
    /// fetch can bound a tight date window around just this one series.
    pub game_number: i32,
    pub games_in_series: i32,
}

/// One game of an MLB series between the two teams. Scores are raw (the same as
/// the headline match score); the UI hides them and the leader until the page's
/// reveal toggle is on, so nothing leaks past the spoiler gate.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeriesGame {
    /// Date label in the viewer's display tz, e.g. "Mon, Jun 23". Derived from the
    /// same tz as `clock_label`, so the date and time always agree (a late game can
    /// fall on a different calendar day in the viewer's tz than at the ballpark).
    pub day_label: String,
    /// Start time in the display tz, e.g. "6:40 PM" (formatted server-side, like
    /// the schedule rows). Empty when the start time can't be parsed.
    #[serde(default)]
    pub clock_label: String,
    /// The same start as `day_label`/`clock_label` but in the ballpark's local tz,
    /// as a full "date · time tz" (e.g. "Mon, Jun 23 · 7:10 PM EDT") — shown when
    /// the viewer clicks the time, like the schedule's venue toggle. Empty when the
    /// venue tz is unknown or matches the display tz (nothing to add).
    #[serde(default)]
    pub venue_label: String,
    /// The two sides' short labels, in the same order as the headline match
    /// (`team_a` is the headline's left team), so each row reads consistently.
    pub team_a: String,
    pub team_b: String,
    pub score_a: Option<i64>,
    pub score_b: Option<i64>,
    /// Which headline side won this game once final: "a", "b", or "" (not final
    /// or tied). Only surfaced by the UI when scores are revealed.
    #[serde(default)]
    pub winner: String,
    pub status: MatchStatus,
    /// True for the game the detail page is about (the "current" game).
    pub current: bool,
    /// gamePk, so each row links to that game's own detail page (0 = this game).
    #[serde(default)]
    pub match_id: i64,
}

/// The multi-game series between the two teams of an MLB match: every game and
/// the overall series standing. Empty `games` ⇒ no series section.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Series {
    pub games: Vec<SeriesGame>,
    /// e.g. "Game 2 of 3" — the current game's place in the series (always safe
    /// to show; reveals nothing about scores).
    #[serde(default)]
    pub game_label: String,
    /// Spoiler-bearing record line computed from the revealed games, e.g.
    /// "Blue Jays lead 2–1", "Series tied 1–1", or "Astros win series 2–1". The
    /// UI shows it only when scores are revealed. Empty when no game is final yet.
    #[serde(default)]
    pub record_label: String,
}

/// One finished F1 session's full classification, for the GP event page.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct F1Result {
    /// The session this classifies: "Race", "Sprint", or "Qualifying".
    pub session: String,
    pub rows: Vec<F1ResultRow>,
}

/// One driver's line in an F1 session result.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct F1ResultRow {
    /// Finishing/grid position, e.g. "1". May be non-numeric for the classified.
    pub pos: String,
    pub driver: String,
    pub constructor: String,
    /// Race/Sprint: finishing time or gap ("+2.974"), else the status ("+1 Lap",
    /// "DNF"). Qualifying: the best lap time (pole time for P1). A spoiler.
    pub detail: String,
    /// The driver's nationality flag (SVG) + their constructor's logo URLs; empty
    /// when unmapped. `#[serde(default)]` for older cached payloads.
    #[serde(default)]
    pub flag: String,
    #[serde(default)]
    pub constructor_logo: String,
    /// The constructor's 2-3 letter abbreviation (e.g. "FER"), shown in place of
    /// the full name where the column is too narrow. Empty when unmapped.
    #[serde(default)]
    pub constructor_abbrev: String,
}

/// The F1 championship standings as of a round, for the GP event page. Both
/// tables are spoilers — they encode every prior result.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct F1Standings {
    /// The round these standings are current as of (e.g. 7).
    pub round: i64,
    pub drivers: Vec<F1StandingRow>,
    pub constructors: Vec<F1StandingRow>,
}

/// One line in an F1 championship table. `detail` carries the driver's
/// constructor in the drivers' table; it's empty in the constructors' table.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct F1StandingRow {
    pub pos: String,
    pub name: String,
    pub detail: String,
    pub points: String,
    pub wins: String,
    /// The driver's nationality flag (drivers' table only) + the constructor's
    /// logo (the driver's team, or the constructor itself). Empty when unmapped.
    #[serde(default)]
    pub flag: String,
    #[serde(default)]
    pub constructor_logo: String,
    /// The constructor's 2-3 letter abbreviation, shown where the name won't fit.
    #[serde(default)]
    pub constructor_abbrev: String,
}

/// Championship standings for WRC/WEC (Orange Cat Blacktop) — a list of titled
/// tables, since each series has several (WRC: drivers / co-drivers /
/// manufacturers; WEC: per class × drivers / teams / manufacturers). Generic
/// enough for both, unlike the F1-specific [`F1Standings`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MotorStandings {
    pub tables: Vec<MotorStandingTable>,
}

/// One championship table within a series' standings.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MotorStandingTable {
    /// The class these tables belong to (WEC: "Hypercar"/"LMGT3"); empty for WRC.
    /// Tables sharing a `group` render together; a new group starts a new row.
    #[serde(default)]
    pub group: String,
    /// e.g. "Drivers", "Teams", "Manufacturers".
    pub title: String,
    pub rows: Vec<MotorStandingRow>,
}

/// One line in a WRC/WEC championship table.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MotorStandingRow {
    pub pos: String,
    pub name: String,
    pub points: String,
    /// Nationality flag (from the API's 2-letter country code); empty when none.
    #[serde(default)]
    pub flag: String,
}

/// A finished motorsport classification for the match page — a WRC stage's times,
/// a WRC rally's overall result, or a MotoGP session's finishing order. Generic
/// across the Orange Cat Blacktop series (F1 keeps its richer [`F1Result`]); the
/// `title` names what it classifies ("Overall classification", "SS4 Stiri 1",
/// "Race"). Spoiler-bearing — the UI gates it behind the reveal toggle.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MotorResult {
    pub title: String,
    pub rows: Vec<MotorResultRow>,
}

/// One competitor's line in a [`MotorResult`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MotorResultRow {
    /// Finishing/classification position, e.g. "1". May be non-numeric (DNF).
    pub pos: String,
    /// The primary competitor: the driver (rally) or rider (MotoGP).
    pub name: String,
    /// A second name line — the co-driver (rally); empty for MotoGP.
    #[serde(default)]
    pub codriver: String,
    /// Team / manufacturer / constructor.
    #[serde(default)]
    pub team: String,
    /// The spoiler figure: the leader's total/stage time, a gap ("+51.800") for
    /// the rest. Empty when the source gives none.
    #[serde(default)]
    pub time: String,
    /// The competitor's nationality flag (SVG); empty when unmapped.
    #[serde(default)]
    pub flag: String,
}

/// A reference for fetching a motorsport row's result on the detail page — a WRC
/// rally's overall classification (`session_id` empty), a single WRC stage's
/// times, or a MotoGP session's finishing order. In-memory only, repopulated
/// each poll like [`crate::types::MlbSeriesRef`]; `None` for everything else.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MotorResultRef {
    /// The series league, e.g. "WRC" / "MotoGP" — selects the API + endpoint.
    pub series: String,
    /// The event UUID (the rally / the MotoGP event).
    pub event_id: String,
    /// The stage/session UUID; empty means the event's overall classification.
    #[serde(default)]
    pub session_id: String,
}

impl MotorResultRef {
    /// Serialize for the DB as `series|event_id|session_id`. The series is a short
    /// token ("WRC"/"WEC"/"MotoGP") and the ids are UUIDs, so none contain `|`;
    /// `session_id` may be empty (an overall-classification placeholder).
    #[must_use]
    pub fn to_db(&self) -> String {
        format!("{}|{}|{}", self.series, self.event_id, self.session_id)
    }

    /// Parse a [`Self::to_db`] string back; `None` if it's empty or malformed
    /// (missing the series or event id).
    #[must_use]
    pub fn from_db(s: &str) -> Option<Self> {
        let mut parts = s.splitn(3, '|');
        let series = parts.next().unwrap_or_default().to_string();
        let event_id = parts.next().unwrap_or_default().to_string();
        let session_id = parts.next().unwrap_or_default().to_string();
        (!series.is_empty() && !event_id.is_empty()).then_some(Self {
            series,
            event_id,
            session_id,
        })
    }
}

/// Everything the per-match detail page shows.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchDetail {
    pub found: bool,
    pub match_view: Option<MatchView>,
    pub streams: Vec<StreamView>,
    pub event: EventInfo,
    /// Extra standings tables shown below `event` — used by MLB to show each
    /// team's division table. Empty for esports (which use `event`).
    #[serde(default)]
    pub stages: Vec<EventInfo>,
    /// The MLB series between the two teams (every game + the series standing).
    /// Empty for esports and for MLB games with no resolvable series.
    /// `#[serde(default)]` so older cached payloads still load.
    #[serde(default)]
    pub series: Series,
    /// The best external "view this game" link (ESPN gamecast / MLB Gameday /
    /// event page), resolved server-side. `None` when no labelled link fits.
    /// `#[serde(default)]` so older cached payloads still load.
    #[serde(default)]
    pub source_link: Option<SourceLink>,
}

/// The match's on-demand results, fetched separately from the page basics (via
/// `get_match_results`) so the detail page can render its header without waiting on
/// a slow — or 500ing — upstream. `unavailable` is true when a finished,
/// result-bearing session returned nothing to show (so the page can say so rather
/// than render an empty gap).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchResults {
    /// For an F1 session: that session's full finishing order (`None` for non-F1,
    /// upcoming, or a session type the source doesn't classify).
    #[serde(default)]
    pub f1_result: Option<F1Result>,
    /// For a finished WRC stage/rally or MotoGP/WEC session: its classification.
    #[serde(default)]
    pub motor_result: Option<MotorResult>,
    /// A finished, result-bearing session whose results came back empty (upstream
    /// 500 / not yet posted) — the page shows a "results not available" note.
    #[serde(default)]
    pub unavailable: bool,
}

/// A sport-agnostic post-game box score, rendered by the shared components in
/// `src/app/components/boxscore.rs`. Every sport's normalizer produces this; the
/// UI only ever sees this, so all sports render identically. All fields are
/// emptyable — a sport fills what it has. Integer shares (not floats) keep the
/// whole tree `Eq` so it can nest in `MatchResults`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoxScore {
    /// Per-game breakdown for multi-game series (esports maps/games). Empty for
    /// the single-game traditional sports — present from day one so a future
    /// esports integration needs no model change.
    #[serde(default)]
    pub games: Vec<GameTab>,
    #[serde(default)]
    pub line: Option<LineScore>,
    #[serde(default)]
    pub leaders: Vec<LeaderCard>,
    #[serde(default)]
    pub team_stats: Vec<StatPair>,
    #[serde(default)]
    pub player_tables: Vec<PlayerTable>,
    #[serde(default)]
    pub timeline: Vec<ScoreEvent>,
    /// Finished game whose upstream returned nothing usable → show the existing
    /// "not available" placeholder rather than an empty section.
    #[serde(default)]
    pub unavailable: bool,
}

/// One game/map of a multi-game series (esports). A nested `BoxScore` reuses the
/// whole render path. Unused by the single-game traditional sports.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameTab {
    pub label: String,
    pub box_score: BoxScore,
}

/// Teams as rows, game segments as columns, plus totals. Segments are innings
/// (MLB), periods (NHL), quarters (NBA/NFL), halves (Soccer), maps (esports).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineScore {
    pub segments: Vec<String>,
    pub away: LineRow,
    pub home: LineRow,
    /// Aggregate columns to the right of the grid (e.g. R/H/E); each carries the
    /// away+home values so it renders in the same two-column body.
    pub totals: Vec<StatPair>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineRow {
    pub team: String,
    pub abbrev: String,
    /// Per-segment score, aligned to `LineScore::segments`; "" for a segment not
    /// played (e.g. bottom of the 9th when the home team didn't bat).
    pub segment_values: Vec<String>,
    pub total: String,
}

/// One comparison row: away vs home for a labeled stat, with an optional
/// proportional lead bar. `away_share` is away's share of the two values as a
/// whole percent (0..=100); home's share is `100 - away_share`. `None` ⇒ no bar.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatPair {
    pub label: String,
    pub away: String,
    pub home: String,
    #[serde(default)]
    pub away_share: Option<u8>,
}

/// A headline performer card (top performers / three stars / stat leaders / MVP).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaderCard {
    pub name: String,
    pub team: String,
    pub category: String,
    pub line: String,
}

/// A collapsible player stat table (one per team per stat group).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerTable {
    pub title: String,
    pub team: String,
    pub columns: Vec<String>,
    pub rows: Vec<PlayerRow>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerRow {
    pub name: String,
    pub note: String,
    pub values: Vec<String>,
}

/// A chronological scoring/key event for the timeline.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScoreEvent {
    pub segment: String,
    pub clock: String,
    pub team: String,
    pub kind: String,
    pub description: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleView {
    pub days: Vec<DayGroup>,
    /// Today's date key (`%Y-%m-%d`) in the display tz, computed server-side so a
    /// day heading can be greyed when past or highlighted when it's today (the
    /// keys sort chronologically as strings). Empty in tests.
    #[serde(default)]
    pub today_key: String,
    /// When the data was last fetched (unix ms).
    pub fetched_at_ms: i64,
    /// Server-formatted "last loaded" time in the display tz, e.g. "10:57 AM".
    pub fetched_label: String,
    /// True when the most recent poll failed and we're serving older data.
    pub stale: bool,
    /// True when no API token is configured and demo data is shown.
    pub using_fixture: bool,
    /// True when demo data was explicitly forced via `DEMO=1`.
    pub demo_forced: bool,
    /// Populated only for the single-day view: ISO dates for prev/next nav.
    pub date_label: Option<String>,
    pub prev_date: Option<String>,
    pub next_date: Option<String>,
}

/// Parse a stat display value ("312", "53.8%", "24/52", "1,024") to a number:
/// take the part before any "/", drop a trailing "%" and any commas.
fn stat_value(s: &str) -> Option<f64> {
    let head = s.trim().split('/').next().unwrap_or("").trim();
    let cleaned = head.trim_end_matches('%').replace(',', "");
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        return None;
    }
    cleaned.parse::<f64>().ok()
}

/// Away's whole-percent share of the two values, for the comparison lead bar.
/// `None` when either value isn't numeric or the two sum to zero.
#[must_use]
pub fn stat_share(away: &str, home: &str) -> Option<u8> {
    let (a, h) = (stat_value(away)?, stat_value(home)?);
    let sum = a + h;
    if sum <= 0.0 {
        return None;
    }
    Some(((a / sum) * 100.0).round().clamp(0.0, 100.0) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    #[test]
    fn event_name_eq_agrees_with_full_event_name() {
        for (league, series) in [("IEM", "Cologne Major"), ("LCK", ""), ("A B", "C")] {
            let full = full_event_name(league, series);
            assert!(event_name_eq(league, series, &full));
        }
        assert!(!event_name_eq("IEM", "Cologne Major", "IEM Katowice"));
        assert!(!event_name_eq("IEM", "Cologne", "IEM Cologne Major")); // series is a prefix
        assert!(!event_name_eq("LCK", "", "LCK Spring"));
        assert!(!event_name_eq("IEM", "Cologne", "IEMCologne")); // missing the space
    }

    #[test]
    fn competition_kind_labels_the_real_competitions() {
        // Racing championships → "series".
        for s in ["F1", "WRC", "WEC"] {
            assert_eq!(competition_kind(s), "series", "{s}");
        }
        // Knockout/standalone competitions → "tournament".
        for t in ["World Cup", "IEM", "Mid-Season Invitational"] {
            assert_eq!(competition_kind(t), "tournament", "{t}");
        }
        // Domestic/regional leagues (incl. a "… Pro League") → "league".
        for l in ["MLB", "LCK", "LCS", "LEC", "LPL", "CBLOL", "XSE Pro League"] {
            assert_eq!(competition_kind(l), "league", "{l}");
        }
    }

    #[test]
    fn game_from_filter_maps_known_and_rejects_other() {
        assert_eq!(Sport::from_filter("cs2"), Some(Sport::Cs2));
        assert_eq!(Sport::from_filter("lol"), Some(Sport::Lol));
        assert_eq!(Sport::from_filter("all"), None);
        assert_eq!(Sport::from_filter(""), None);
    }

    #[test]
    fn game_slug_and_label() {
        assert_eq!(Sport::Cs2.slug(), "cs2");
        assert_eq!(Sport::Lol.slug(), "lol");
        assert_eq!(Sport::Cs2.label(), "CS2");
        assert_eq!(Sport::Lol.label(), "LoL");
        // Traditional sports label by the sport, not the league.
        assert_eq!(Sport::Mlb.label(), "Baseball");
        assert_eq!(Sport::Nhl.label(), "Hockey");
        assert_eq!(Sport::Nba.label(), "Basketball");
        // American football is "Football"; the international sport is "Soccer"
        // (its slug stays "football", unchanged).
        assert_eq!(Sport::Nfl.label(), "Football");
        assert_eq!(Sport::Soccer.label(), "Soccer");
        assert_eq!(Sport::Soccer.slug(), "football");
        assert_eq!(Sport::Motorsport.label(), "Motorsport");
    }

    #[test]
    fn match_status_str_roundtrips() {
        for s in [
            MatchStatus::Upcoming,
            MatchStatus::Live,
            MatchStatus::Finished,
            MatchStatus::Canceled,
        ] {
            assert_eq!(MatchStatus::from_db(s.as_str()), s);
        }
        assert_eq!(MatchStatus::from_db("nonsense"), MatchStatus::Upcoming);
    }

    #[test]
    fn every_game_slug_round_trips_through_from_filter() {
        // `slug` and `from_filter` must be inverse over every sport — store.rs
        // persists `slug()` and reads it back with `from_filter`, so a mismatch
        // would make a row load as a different (default) sport. Also asserts `ALL`
        // is exhaustive (a new variant trips the count).
        assert_eq!(Sport::ALL.len(), 8);
        for &g in Sport::ALL {
            assert_eq!(Sport::from_filter(g.slug()), Some(g), "slug {:?}", g.slug());
        }
    }

    #[test]
    fn soccer_slug_is_football_not_legacy_soccer() {
        // Regression: soccer's slug was renamed "soccer" → "football". The old
        // slug must not parse, so legacy rows are dropped rather than loaded as
        // the default sport (which surfaced the World Cup on the esports page).
        assert_eq!(Sport::Soccer.slug(), "football");
        assert_eq!(Sport::from_filter("football"), Some(Sport::Soccer));
        assert_eq!(Sport::from_filter("soccer"), None);
    }

    #[test]
    fn standings_columns_match_the_sport() {
        // Esports show a map/game record (not a single value); the team sports a
        // single ranking figure with a sport-specific label.
        assert_eq!(Sport::Cs2.standings_last_label(), "Maps");
        assert_eq!(Sport::Lol.standings_last_label(), "Games");
        assert!(!Sport::Cs2.standings_single_value());
        assert!(!Sport::Lol.standings_single_value());
        for (g, label) in [
            (Sport::Mlb, "GB"),
            (Sport::Nhl, "PTS"),
            (Sport::Nba, "GB"),
            (Sport::Nfl, "PCT"),
            (Sport::Soccer, "PTS"),
        ] {
            assert_eq!(g.standings_last_label(), label, "{g:?}");
            assert!(g.standings_single_value(), "{g:?}");
        }
        // F1 has no standings table.
        assert_eq!(Sport::Motorsport.standings_last_label(), "");
        assert!(!Sport::Motorsport.standings_single_value());
    }

    #[test]
    fn match_uid_round_trips_for_every_game() {
        for &g in Sport::ALL {
            let uid = match_uid(g, 1494586);
            assert_eq!(parse_match_uid(&uid), Some((g, 1494586)), "uid {uid}");
            // The slug has no '-', so the split is unambiguous.
            assert_eq!(uid, format!("{}-1494586", g.slug()));
        }
        assert_eq!(parse_match_uid("cs2-1494586"), Some((Sport::Cs2, 1494586)));
        assert_eq!(parse_match_uid("mlb-822869"), Some((Sport::Mlb, 822869)));
        // Malformed / unknown slug → None.
        assert_eq!(parse_match_uid("1494586"), None); // no separator
        assert_eq!(parse_match_uid("bogus-1"), None); // unknown sport
        assert_eq!(parse_match_uid("cs2-abc"), None); // non-numeric id
    }

    #[test]
    fn match_path_scopes_sport_and_id_into_segments() {
        assert_eq!(match_path(Sport::Mlb, 824255), "/match/mlb/824255");
        assert_eq!(match_path(Sport::Cs2, 1494586), "/match/cs2/1494586");
        // The path segments are exactly the uid split on its single '-'.
        for &g in Sport::ALL {
            let (slug, id) = parse_match_uid(&match_uid(g, 7)).unwrap();
            assert_eq!(match_path(g, 7), format!("/match/{}/{}", slug.slug(), id));
        }
    }

    #[test]
    fn match_status_badge_and_row_class() {
        assert_eq!(MatchStatus::Live.badge(), "LIVE");
        assert_eq!(MatchStatus::Finished.badge(), "Final");
        assert_eq!(MatchStatus::Canceled.badge(), "Canc.");
        assert_eq!(MatchStatus::Upcoming.badge(), ""); // no badge while upcoming
                                                       // Row classes the SCSS keys off (note "final", not the wire "finished").
        assert_eq!(MatchStatus::Live.row_class(), "live");
        assert_eq!(MatchStatus::Finished.row_class(), "final");
        assert_eq!(MatchStatus::Canceled.row_class(), "canceled");
        assert_eq!(MatchStatus::Upcoming.row_class(), "upcoming");
    }

    #[test]
    fn box_score_serde_round_trips() {
        let bs = BoxScore {
            line: Some(LineScore {
                segments: vec!["1".into(), "2".into()],
                away: LineRow {
                    team: "White Sox".into(),
                    abbrev: "CWS".into(),
                    segment_values: vec!["1".into(), "0".into()],
                    total: "1".into(),
                },
                home: LineRow {
                    team: "Orioles".into(),
                    abbrev: "BAL".into(),
                    segment_values: vec!["1".into(), "4".into()],
                    total: "5".into(),
                },
                totals: vec![StatPair {
                    label: "R".into(),
                    away: "1".into(),
                    home: "5".into(),
                    away_share: None,
                }],
            }),
            team_stats: vec![StatPair {
                label: "Hits".into(),
                away: "9".into(),
                home: "8".into(),
                away_share: Some(53),
            }],
            leaders: vec![LeaderCard {
                name: "A. Ruiz".into(),
                team: "BAL".into(),
                category: "Top performer".into(),
                line: "3-4, 2 HR".into(),
            }],
            player_tables: vec![PlayerTable {
                title: "Batting — BAL".into(),
                team: "BAL".into(),
                columns: vec!["AB".into(), "H".into()],
                rows: vec![PlayerRow {
                    name: "A. Ruiz".into(),
                    note: "SS".into(),
                    values: vec!["4".into(), "3".into()],
                }],
            }],
            timeline: vec![ScoreEvent {
                segment: "2nd".into(),
                clock: "".into(),
                team: "BAL".into(),
                kind: "hr".into(),
                description: "Ruiz HR (2)".into(),
            }],
            games: Vec::new(),
            unavailable: false,
        };
        let json = serde_json::to_string(&bs).unwrap();
        let back: BoxScore = serde_json::from_str(&json).unwrap();
        assert_eq!(bs, back);
        // Defaults: an empty object deserializes to an all-empty BoxScore.
        let empty: BoxScore = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, BoxScore::default());
    }

    #[test]
    fn stat_share_parses_display_values() {
        assert_eq!(stat_share("312", "289"), Some(52)); // 312/601 = 51.9 → 52
        assert_eq!(stat_share("53.8%", "46.2%"), Some(54)); // strips %
        assert_eq!(stat_share("24/52", "28/52"), Some(46)); // ratio → numerator
        assert_eq!(stat_share("1,024", "976"), Some(51)); // strips commas
        assert_eq!(stat_share("bad", "5"), None); // non-numeric
        assert_eq!(stat_share("0", "0"), None); // both zero
    }
}
