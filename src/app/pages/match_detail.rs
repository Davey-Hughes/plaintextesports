//! The match-detail page: the detail view, streams, per-game series rows, and
//! the F1 / motorsport result views.
use crate::app::pages::home::motorsport_watch;
use crate::app::*;

#[derive(Params, PartialEq, Clone)]
pub(crate) struct DetailParams {
    /// The sport slug segment, e.g. `"mlb"` (see [`crate::types::match_path`]).
    sport: String,
    /// The source match id segment; joined with `sport` into a `match_uid`.
    id: String,
}

/// A Grand Prix's results — each finished session's full finishing order, each
/// spoiler-gated (hidden until revealed, like the rest of the site's scores).
/// Stable in-page anchor for an F1 session/section (e.g. "Practice 1" →
/// "f1sec-practice-1"), shared by the section ids and the jump-to nav.
pub(crate) fn f1_anchor(name: &str) -> String {
    format!("f1sec-{}", name.to_lowercase().replace(' ', "-"))
}

#[component]
pub(crate) fn F1Results(results: Vec<F1Result>, season: i64, round: i64) -> impl IntoView {
    let sections = results
        .into_iter()
        .map(|r| {
            let session = r.session;
            let anchor = f1_anchor(&session);
            let (revealed, toggle) = section_reveal(format!("f1res:{season}:{round}:{session}"));
            let count = r.rows.len();
            let rows = StoredValue::new(r.rows);
            // Always render every row so revealing doesn't shift the page; the
            // position column (just a row number) stays, and the driver /
            // constructor / time blank out until revealed.
            let order = move || {
                let show = revealed.get();
                rows.get_value()
                    .into_iter()
                    .map(|row| {
                        // Flags/logos are gated with the names — showing a flag while
                        // the driver is blank would leak who finished where.
                        let (driver, con, cabbr, detail, flag, clogo) = if show {
                            (
                                row.driver,
                                row.constructor,
                                row.constructor_abbrev,
                                row.detail,
                                row.flag,
                                row.constructor_logo,
                            )
                        } else {
                            (
                                String::new(),
                                String::new(),
                                String::new(),
                                String::new(),
                                String::new(),
                                String::new(),
                            )
                        };
                        view! {
                            <li class="f1-row">
                                <span class="f1-pos">{row.pos}</span>
                                <span class="f1-driver">
                                    {team_logo(&flag, "f1-flag")}{driver}
                                </span>
                                <span class="f1-con">
                                    {clogo_icon(&clogo)}{team_cell(con, cabbr)}
                                </span>
                                <span class="f1-detail">{detail}</span>
                            </li>
                        }
                    })
                    .collect_view()
            };
            view! {
                <div class="f1-session" id=anchor>
                    <button class="f1-session-head" on:click=toggle>
                        <span class="f1-session-name">{session}</span>
                        <span class="f1-session-toggle">
                            {move || {
                                if revealed.get() {
                                    "hide results".to_string()
                                } else {
                                    format!("show results ({count})")
                                }
                            }}
                        </span>
                    </button>
                    <ol class="f1-order">{order}</ol>
                </div>
            }
        })
        .collect_view();
    view! {
        <section class="detail-section">
            <h2 class="section-title f1-results-title">"Results"</h2>
            {sections}
        </section>
    }
}

/// Stable in-page anchor for a motorsport result section (e.g. "Overall
/// classification" → "motorsec-overall-classification"), shared by the section
/// ids and the event page's jump-to nav.
pub(crate) fn motor_anchor(name: &str) -> String {
    format!("motorsec-{}", name.to_lowercase().replace(' ', "-"))
}

/// A set of finished motorsport classifications (the WRC overall + Power Stage,
/// or each WEC/MotoGP session) rendered like [`F1Results`]: a "Results" section
/// with one spoiler-gated sub-section each, reusing the F1 row layout (so the
/// long-name hover popover on `.f1-driver`/`.f1-con` applies — WEC crews can be
/// long). `key_prefix` scopes the per-section reveal keys: the event page passes
/// the edition, a single match page passes its id, so each page's reveals stay
/// distinct.
#[component]
pub(crate) fn MotorResultsView(results: Vec<MotorResult>, key_prefix: String) -> impl IntoView {
    let sections = results
        .into_iter()
        .map(|result| {
            let MotorResult { title, rows } = result;
            let anchor = motor_anchor(&title);
            let (revealed, toggle) = section_reveal(format!("{key_prefix}:{title}"));
            let count = rows.len();
            let rows = StoredValue::new(rows);
            // Always render every row so revealing doesn't shift the page; the
            // position stays, names/team/time blank until revealed.
            let order = move || {
                let show = revealed.get();
                rows.get_value()
                    .into_iter()
                    .map(|row| {
                        let (name, codriver, team, time, flag) = if show {
                            (row.name, row.codriver, row.team, row.time, row.flag)
                        } else {
                            (
                                String::new(),
                                String::new(),
                                String::new(),
                                String::new(),
                                String::new(),
                            )
                        };
                        view! {
                            <li class="f1-row">
                                <span class="f1-pos">{row.pos}</span>
                                <span class="f1-driver">
                                    {team_logo(&flag, "f1-flag")}{name}
                                    {(!codriver.is_empty())
                                        .then(|| view! { <span class="motor-codriver">{codriver}</span> })}
                                </span>
                                <span class="f1-con">{team}</span>
                                <span class="f1-detail">{time}</span>
                            </li>
                        }
                    })
                    .collect_view()
            };
            view! {
                <div class="f1-session" id=anchor>
                    <button class="f1-session-head" on:click=toggle>
                        <span class="f1-session-name">{title}</span>
                        <span class="f1-session-toggle">
                            {move || {
                                if revealed.get() {
                                    "hide results".to_string()
                                } else {
                                    format!("show results ({count})")
                                }
                            }}
                        </span>
                    </button>
                    <ol class="f1-order">{order}</ol>
                </div>
            }
        })
        .collect_view();
    view! {
        <section class="detail-section">
            <h2 class="section-title f1-results-title">"Results"</h2>
            {sections}
        </section>
    }
}

#[component]
pub(crate) fn MatchDetailPage() -> impl IntoView {
    let params = use_params::<DetailParams>();
    // The route splits the uid into `/match/{sport}/{id}`; rejoin into the
    // internal `"{sport}-{id}"` key the server (and reveal sets) expect.
    let uid = move || {
        params
            .get()
            .ok()
            .map(|p| format!("{}-{}", p.sport, p.id))
            .unwrap_or_default()
    };
    let hour24 = use_context::<RwSignal<bool>>().expect("hour24 context");
    let tz = use_context::<RwSignal<String>>().expect("tz context");
    let detail = Resource::new(
        move || (uid(), tz.get(), hour24.get()),
        |(uid, tz, h)| async move { get_match_detail(uid, tz, h).await },
    );
    // The on-demand results load on their own resource so a slow (or 500ing)
    // upstream never delays the header — see `detail_view`.
    let results = Resource::new(
        move || (uid(), tz.get(), hour24.get()),
        |(uid, tz, h)| async move { get_match_results(uid, tz, h).await.unwrap_or_default() },
    );
    view! {
        // Transition, not Suspense: the resource is keyed on tz/hour24, so toggling
        // the timezone or 12/24h clock while reading a match refetches in place —
        // Transition keeps the match on screen instead of flashing "loading…".
        <Transition fallback=|| view! { <p class="loading">"loading…"</p> }>
            {move || {
                detail
                    .get()
                    .map(|res| match res {
                        Ok(d) if d.found => detail_view(d, results).into_any(),
                        _ => view! { <p class="empty">"Match not found."</p> }.into_any(),
                    })
            }}
        </Transition>
    }
}

pub(crate) fn detail_view(d: MatchDetail, results: Resource<MatchResults>) -> impl IntoView {
    let MatchDetail {
        match_view,
        streams,
        event,
        stages,
        series,
        source_link,
        ..
    } = d;
    let m = match_view.expect("found implies match_view");
    // This match's time is the reveal "end" for every section reveal on the page
    // (its results, the series rows), so they prune ~a week past the match.
    provide_context(RevealEnd(m.begin_at_ms));
    // Per-page reveal key for a motorsport (WRC/MotoGP) result section.
    let motor_reveal_key = format!("motorres:{}", m.id);
    // For an F1 session page, derive (season, round) from the id encoding
    // (season*100_000 + round*100 + session_ord) so the results section shares
    // the GP event page's per-session reveal key.
    let (f1_season, f1_round) = (m.id / 100_000, (m.id / 100) % 1000);
    let status_label = match m.status {
        MatchStatus::Live => "LIVE",
        MatchStatus::Finished => "Final",
        MatchStatus::Canceled => "Canceled",
        MatchStatus::Upcoming => "",
    };
    // The meta line is split so the date+time can swap to the venue-local time on
    // click (blue), like the schedule rows. Lead = event name; the middle is the
    // clickable when; trail = best-of + status.
    let event_name = full_event_name(&m.league, &m.series_name);
    // Motorsport series have no per-event broadcast data; show the stable
    // per-series service link(s) (F1 TV, Rally.TV, WEC on YouTube, MotoGP
    // VideoPass) — the same links the event page carries. Empty for other sports.
    let motor_watch: Vec<(&'static str, &'static str)> = motorsport_watch(&m.league).to_vec();
    let when_local = [m.date_label.clone(), m.clock_label.clone()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    let venue_when = m.venue_label.clone();
    let trail = [m.best_of.clone(), status_label.to_string()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    // "Venue · City, Country" (whichever the source provides); empty for esports.
    let venue_line = [m.venue_name.trim(), m.venue_location.trim()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");

    // Shared venue-time toggle (same one the schedule rows use), so the date+time
    // swaps to the venue's local date+time when clicked.
    let show_venue = use_context::<ShowVenue>().map(|s| s.0);
    let has_venue = !venue_when.is_empty();
    let venue_shown = move || show_venue.is_some_and(|sv| sv.get());
    let toggle_venue = move |ev: leptos::ev::MouseEvent| {
        ev.prevent_default();
        ev.stop_propagation();
        if let Some(sv) = show_venue {
            sv.update(|v| *v = !*v);
        }
    };
    let venue_title = move || {
        if venue_shown() {
            "Click to hide the local time at the venue"
        } else {
            "Click to show the local time at the venue"
        }
    };

    let muid = m.uid();
    let (sa, sb) = (m.team_a.score, m.team_b.score);
    let has_score = sa.is_some() && sb.is_some();
    // "Played" = under way or done (an upcoming match can still carry a 0-0
    // placeholder score, so a score alone isn't enough).
    let played = matches!(m.status, MatchStatus::Live | MatchStatus::Finished) && has_score;
    let (win_a, win_b) = (m.team_a.winner, m.team_b.winner);
    let sep = versus_sep(m.sport);
    // F1 (and any single-entity sport) has no opponent: the title is the one
    // session label, with no "vs"/"at" separator and no empty second team.
    let is_solo = m.sport.single_entity();
    let sport = m.sport;
    let (team_a, team_b) = (m.team_a.label, m.team_b.label);
    let (name_a, name_b) = (m.team_a.name, m.team_b.name);
    let (logo_a, logo_b) = (m.team_a.logo, m.team_b.logo);

    // Per-match "remind me" ★ beside the title — the same control the schedule
    // rows carry, so a match can be subscribed to straight from its own page. Only
    // for an upcoming match (a past/live/finished one can't be reminded about) and
    // only once Web Push is available (read reactively, so it appears after the key
    // resolves). Captured here before the title view consumes the names.
    let star_upcoming = matches!(m.status, MatchStatus::Upcoming);
    let (star_id, star_game, star_league) = (m.id, m.sport, m.league.clone());
    let (star_a, star_b) = (name_a.clone(), name_b.clone());
    let vapid_ctx = use_context::<RwSignal<Option<String>>>();
    let match_star = move || {
        let push = vapid_ctx.is_some_and(|v| v.with(|x| x.is_some()));
        (star_upcoming && push).then(|| {
            view! {
                <StarButton
                    id=star_id
                    sport=star_game
                    league=star_league.clone()
                    team_a=star_a.clone()
                    team_b=star_b.clone()
                />
            }
        })
    };

    // Scores/standings/bracket are spoilers: reveal when the global toggle is on
    // or this match was individually revealed (persisted, shared with the list).
    let global = use_context::<ShowScores>().map(|s| s.0);
    let revealed = use_context::<RevealedMatches>().map(|r| r.0);
    let reveal = {
        let muid = muid.clone();
        Memo::new(move |_| {
            global.is_some_and(|g| g.get())
                || revealed.is_some_and(|r| r.with(|set| set.contains(&muid)))
        })
    };
    // Per-match reveal button (hidden when the global toggle already shows all).
    let toggle = reveal_toggler(revealed, muid, m.begin_at_ms);
    // Hide the reveal toggle when the global toggle already shows all, or when
    // the match hasn't been played yet (nothing to reveal).
    let toggle_hidden = move || global.is_some_and(|g| g.get()) || !played;

    view! {
        <article class="detail">
            <A href="/">"← schedule"</A>
            <div class="detail-title-row">
                {match_star}
                <h1 class="detail-title match-title" class:detail-title-solo=is_solo>
                {if is_solo {
                    view! {
                        <span class="detail-team detail-solo">
                            {team_logo(&logo_a, "team-logo-lg")}
                            {team_a}
                        </span>
                    }
                    .into_any()
                } else {
                    view! {
                        <span
                            class="detail-team"
                            class:winner=move || reveal.get() && win_a
                            class:loser=move || reveal.get() && win_b
                        >
                            {team_logo(&logo_a, "team-logo-lg")}
                            {team_link(sport, team_a, name_a)}
                        </span>
                        <span class="detail-score">
                            {move || {
                                if reveal.get() && has_score {
                                    format!("{} – {}", sa.unwrap_or(0), sb.unwrap_or(0))
                                } else {
                                    sep.to_string()
                                }
                            }}
                        </span>
                        <span
                            class="detail-team"
                            class:winner=move || reveal.get() && win_b
                            class:loser=move || reveal.get() && win_a
                        >
                            {team_link(sport, team_b, name_b)}
                            {team_logo(&logo_b, "team-logo-lg")}
                        </span>
                    }
                    .into_any()
                }}
                </h1>
            </div>
            <div class="detail-meta">
                <span class="detail-meta-line">
                    <span class="detail-event">
                        <A href=event_path(sport, &event_name)>{event_name.clone()}</A>
                        " · "
                    </span>
                    {if has_venue {
                        let local = when_local.clone();
                        let venue = venue_when.clone();
                        view! {
                            <span
                                class="detail-when row-time-tz"
                                class:on=venue_shown
                                title=venue_title
                                on:click=toggle_venue
                            >
                                {move || if venue_shown() { venue.clone() } else { local.clone() }}
                            </span>
                        }
                        .into_any()
                    } else {
                        view! { <span>{when_local.clone()}</span> }.into_any()
                    }}
                    {(!trail.is_empty()).then(|| format!(" · {trail}"))}
                </span>
                {move || {
                    let toggle = toggle.clone();
                    (!toggle_hidden())
                        .then(move || {
                            view! {
                                <button class="toggle" on:click=toggle>
                                    {move || if reveal.get() { "hide scores" } else { "show scores" }}
                                </button>
                            }
                        })
                }}
            </div>
            {(!venue_line.is_empty())
                .then(|| view! { <div class="detail-venue">{venue_line}</div> })}
            {source_link.map(|s| view! {
                <p class="event-link">
                    <a href=s.url target="_blank" rel="noreferrer">
                        {format!("view on {} ↗", s.label)}
                    </a>
                </p>
            })}
            {(!motor_watch.is_empty()).then(|| {
                let links = motor_watch
                    .iter()
                    .enumerate()
                    .map(|(i, (name, url))| view! {
                        {(i > 0).then(|| view! { <span class="sep">" · "</span> })}
                        <a href=*url target="_blank" rel="noreferrer">
                            {format!("{name} ↗")}
                        </a>
                    })
                    .collect_view();
                view! { <p class="event-link event-watch">"watch · "{links}</p> }
            })}
            <StreamsList streams=streams />
            // Results load on their own resource (not with the header), so a slow or
            // 500ing upstream never delays the title/meta. F1 → that session's
            // finishing order; WRC/WEC/MotoGP → its classification; a finished
            // result-bearing session with nothing to show → "results not available".
            // Spoiler-gated; absent for upcoming / non-result sessions.
            {move || {
                let motor_key = motor_reveal_key.clone();
                view! {
                    // Transition, not Suspense: `results` is keyed on tz/hour24 too,
                    // so a clock/timezone toggle refetches it in place — keep the
                    // results section visible rather than flashing its loading state.
                    <Transition fallback=|| {
                        view! {
                            <section class="detail-section">
                                <p class="loading">"loading…"</p>
                            </section>
                        }
                    }>
                        {move || {
                            let motor_key = motor_key.clone();
                            results
                                .get()
                                .map(move |r| {
                                    if let Some(fr) = r.f1_result {
                                        view! { <F1Results results=vec![fr] season=f1_season round=f1_round /> }
                                            .into_any()
                                    } else if let Some(mr) = r.motor_result {
                                        view! { <MotorResultsView results=vec![mr] key_prefix=motor_key.clone() /> }
                                            .into_any()
                                    } else if r.unavailable {
                                        view! {
                                            <section class="detail-section">
                                                <h2 class="section-title f1-results-title">"Results"</h2>
                                                <p class="empty">"results not available"</p>
                                            </section>
                                        }
                                            .into_any()
                                    } else {
                                        ().into_any()
                                    }
                                })
                        }}
                    </Transition>
                }
            }}
            // MLB series between the two teams. Each sport row reveals on its own
            // (shared with the schedule); the record line waits until every played
            // sport is revealed, so it never leaks the leader ahead of the scores.
            {(!series.games.is_empty())
                .then(|| {
                    // Series games are MLB; key by uid like the per-match set.
                    let played_ids: Vec<String> = series
                        .games
                        .iter()
                        .filter(|g| {
                            matches!(g.status, MatchStatus::Live | MatchStatus::Finished)
                                && g.score_a.is_some()
                                && g.score_b.is_some()
                        })
                        .map(|g| crate::types::match_uid(Sport::Mlb, g.match_id))
                        .collect();
                    let global = use_context::<ShowScores>().map(|s| s.0);
                    let revealed = use_context::<RevealedMatches>().map(|r| r.0);
                    let record_reveal = Memo::new(move |_| {
                        global.is_some_and(|g| g.get())
                            || (!played_ids.is_empty()
                                && revealed.is_some_and(|r| {
                                    r.with(|set| played_ids.iter().all(|id| set.contains(id)))
                                }))
                    });
                    view! { <SeriesSection series=series record_reveal=record_reveal /> }
                })}
            // The event's stage combo (Swiss grid/list + playoff bracket), same as
            // the event page; each section self-gates its own reveal (shared,
            // persisted). Empty sections render nothing.
            {(!event.is_empty())
                .then(|| view! { <EventStageCombo event=event swiss_grid=RwSignal::new(true) /> })}
            // MLB has no per-event standings; show the two teams' division tables.
            {(!stages.is_empty()).then(|| view! { <EventStages stages=stages /> })}
        </article>
    }
}

/// Split a stream URL into a friendly (site, channel), e.g.
/// "https://www.twitch.tv/esl_csgo" → ("Twitch", "esl_csgo").
pub(crate) fn stream_parts(url: &str) -> (String, String) {
    let after = url
        .split_once("//")
        .map_or(url, |(_, rest)| rest)
        .trim_end_matches('/');
    let (domain, path) = after.split_once('/').unwrap_or((after, ""));
    let domain = domain.trim_start_matches("www.");
    let site = if domain.contains("twitch") {
        "Twitch"
    } else if domain.contains("youtube") || domain.contains("youtu.be") {
        "YouTube"
    } else if domain.contains("kick") {
        "Kick"
    } else if domain.contains("afreeca") || domain.contains("sooplive") || domain.contains("soop") {
        "SOOP"
    } else if domain.contains("bilibili") {
        "Bilibili"
    } else {
        domain
    };
    // The channel/handle is the path (drop any query string).
    let channel = path.split('?').next().unwrap_or("").trim_matches('/');
    (site.to_string(), channel.to_string())
}

/// Language + main/official tags for a stream, e.g. "EN · main".
pub(crate) fn stream_tags(s: &StreamView) -> String {
    let mut tags = Vec::new();
    if !s.language.is_empty() {
        tags.push(s.language.to_uppercase());
    }
    if s.main {
        tags.push("main".to_string());
    } else if s.official {
        tags.push("official".to_string());
    }
    tags.join(" · ")
}

#[component]
pub(crate) fn StreamsList(streams: Vec<StreamView>) -> impl IntoView {
    if streams.is_empty() {
        return ().into_any();
    }
    // MLB broadcasts carry a network `name` (and sometimes a watch link); esports
    // streams carry a clickable `url`. Title the section to match what it lists.
    let broadcast = streams.iter().any(|s| !s.name.is_empty());
    let title = if broadcast { "Broadcasts" } else { "Streams" };
    // A small gap leads each new group (after the first), separating official
    // streams from co-streams, and TV / streaming / radio for broadcasts.
    let gaps: Vec<bool> = streams
        .iter()
        .enumerate()
        .map(|(i, s)| i > 0 && streams[i - 1].group != s.group)
        .collect();
    let items = streams
        .into_iter()
        .zip(gaps)
        .map(|(s, gap)| {
            // A broadcast's label is its network name; an esports stream's is
            // derived from its url (site + channel).
            let (label, channel) = if s.name.is_empty() {
                stream_parts(&s.url)
            } else {
                (s.name.clone(), String::new())
            };
            let tags = if s.name.is_empty() {
                stream_tags(&s)
            } else {
                s.tag.clone()
            };
            let mut cls = String::from("stream");
            // Highlight an esports official broadcast (not the per-sport MLB flag).
            if s.official && s.name.is_empty() {
                cls.push_str(" official");
            }
            if gap {
                cls.push_str(" stream-group-start");
            }
            let label_view = view! {
                <span class="stream-site">{label}</span>
                {(!channel.is_empty())
                    .then(|| view! { <span class="stream-chan">{format!("/{channel}")}</span> })}
            };
            let body = if s.url.is_empty() {
                label_view.into_any()
            } else {
                view! { <a href=s.url target="_blank" rel="noreferrer">{label_view}</a> }.into_any()
            };
            view! {
                <li class=cls>
                    {body}
                    {(!tags.is_empty()).then(|| view! { <span class="stream-tags">{tags}</span> })}
                </li>
            }
        })
        .collect_view();
    view! {
        <section class="detail-section">
            <h2 class="section-title">{title}</h2>
            <ul class="streams">{items}</ul>
        </section>
    }
    .into_any()
}

/// The MLB "Series" section: each game of the series between the two teams as a
/// schedule-style row — date, status badge ("Final"/"LIVE"), the matchup, and a
/// score you reveal by clicking the badge (no need to open the game first). Each
/// row gates on its OWN game's reveal (the same per-match set the schedule uses),
/// so revealing here reveals it on the homepage too, and vice versa. The series
/// record only shows once every played game in the series is revealed, so it
/// never leaks the leader ahead of the scores it's derived from.
#[component]
pub(crate) fn SeriesSection(series: Series, record_reveal: Memo<bool>) -> impl IntoView {
    let Series {
        games,
        game_label,
        record_label,
    } = series;
    // Whether any game can swap to a ballpark-local time (so the "venue time"
    // hint by the title only appears when there's actually a venue toggle here,
    // not just because some other page flipped the shared ShowVenue).
    let has_venue = games.iter().any(|g| !g.venue_label.is_empty());
    let rows = games
        .into_iter()
        .map(|g| view! { <SeriesRow game=g /> })
        .collect_view();
    // The record line is a spoiler (it names the leader). Show it (when present)
    // only once every played game in the series is revealed — `record_reveal`,
    // computed by the parent from the shared per-game reveal set. The line always
    // occupies its height (a blank placeholder when hidden) so revealing it never
    // shifts the page layout.
    let record = (!record_label.is_empty()).then(|| {
        view! {
            <p class="series-record" class:series-hidden=move || !record_reveal.get()>
                {move || if record_reveal.get() { record_label.clone() } else { "\u{00a0}".to_string() }}
            </p>
        }
    });
    let game_label =
        (!game_label.is_empty()).then(|| view! { <span class="series-meta">{game_label}</span> });
    // When the ballpark time is being shown (and this series has a venue toggle),
    // a small blue "venue time" hint by the title makes the swapped state obvious.
    let venue_hint = has_venue.then(|| {
        let show_venue = use_context::<ShowVenue>().map(|s| s.0);
        let shown = move || show_venue.is_some_and(|sv| sv.get());
        view! {
            {move || shown().then(|| view! { <span class="series-venue-hint">"venue time"</span> })}
        }
    });
    view! {
        <section class="detail-section">
            <h2 class="section-title">"Series" {game_label} {venue_hint}</h2>
            <div class="series-games">{rows}</div>
            {record}
        </section>
    }
}

/// One schedule-style row in the Series section. Mirrors `MatchRow`'s per-game
/// score reveal: the status badge is clickable to toggle just this game's score,
/// keyed on its own `match_id` in the shared `RevealedMatches` set (so it stays
/// in sync with the homepage). The whole row links to that game's detail page.
#[component]
pub(crate) fn SeriesRow(game: SeriesGame) -> impl IntoView {
    let SeriesGame {
        day_label,
        clock_label,
        venue_label,
        team_a,
        team_b,
        score_a,
        score_b,
        winner,
        status,
        current,
        match_id,
    } = game;
    let status_class = status.row_class();
    let badge = status.badge();
    let has = score_a.is_some() && score_b.is_some();
    let played = matches!(status, MatchStatus::Live | MatchStatus::Finished) && has;
    let (win_a, win_b) = (winner == "a", winner == "b");

    // This game's own reveal (global toggle OR this game individually revealed),
    // shared with the schedule via the per-match set keyed on its uid. Series
    // games are always MLB. A played game's badge is the reveal control.
    let muid = crate::types::match_uid(Sport::Mlb, match_id);
    // A series game has no timestamp of its own, so use the match page's reveal end
    // (the headline game's time) for all of the series' rows.
    let (reveal, toggle_reveal) = row_reveal(&muid, current_reveal_end());
    let meta_view = reveal_meta(
        reveal,
        toggle_reveal,
        status,
        status_class,
        badge,
        None,
        played,
    );

    // Each row leads with its date and start time on one line, both in the
    // viewer's tz so they always agree. When the venue tz is known it's clickable
    // to *swap* the whole label to the ballpark's local date+time (rather than
    // appending it) — so the row width and the team highlighting stay put. The
    // toggle is the shared `ShowVenue` one the schedule uses.
    let when_text = if clock_label.is_empty() {
        day_label.clone()
    } else {
        format!("{day_label} · {clock_label}")
    };
    let when_view = venue_time_cell(
        "row-time series-when",
        when_text,
        venue_label,
        "Click to show the local time at the ballpark",
        "Showing the time at the ballpark — click for your local time",
    );
    let inner = view! {
        {when_view}
        {versus_row(
            reveal,
            win_a,
            win_b,
            has,
            view! { {team_a} }.into_any(),
            view! { {team_b} }.into_any(),
            score_a,
            score_b,
            "at".to_string(),
        )}
        {meta_view}
    };

    // The row links to this game's detail page; the current game (this page)
    // isn't a link — it's marked instead. `display: contents` lets the body's
    // children sit in the row grid.
    let body = if current {
        view! { <span class="row-body">{inner}</span> }.into_any()
    } else {
        view! {
            <a class="row-body" href=crate::types::match_path(Sport::Mlb, match_id)>{inner}</a>
        }
        .into_any()
    };
    let mut row_cls = format!("row {status_class}");
    if current {
        row_cls.push_str(" series-current");
    }
    view! {
        <div class=format!("{row_cls} has-star")>
            {status_bar(status)}
            {body}
        </div>
    }
}
