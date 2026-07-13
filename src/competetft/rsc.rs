//! Extract competetft.com's server-rendered data. The Next.js app embeds its
//! GraphQL results as RSC payload chunks (`self.__next_f.push([n,"…"])`); we
//! concatenate the decoded chunk strings and pull the typed objects we need out
//! of the resulting blob.

use chrono::{DateTime, Utc};

/// Concatenate the decoded `self.__next_f.push([n,"…"])` payload chunks into one
/// blob. Each chunk's payload is a JSON string literal (escaped); we decode it
/// with `serde_json` so `\uXXXX` / `\n` / `\"` come through correctly.
#[must_use]
pub fn rsc_blob(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    let marker = "self.__next_f.push([";
    while let Some(i) = rest.find(marker) {
        rest = &rest[i + marker.len()..];
        // Skip the leading `<n>,` and take the following JSON string literal.
        let Some(q) = rest.find('"') else { break };
        let after = &rest[q..];
        let bytes = after.as_bytes();
        // Find the matching closing quote, honoring backslash escapes. `"` and
        // `\` are ASCII, and UTF-8 continuation bytes are all >= 0x80, so this
        // byte walk never splits a multibyte char and `after[..=j]` stays on a
        // char boundary.
        let mut j = 1;
        while j < bytes.len() {
            match bytes[j] {
                b'\\' => j += 2,
                b'"' => break,
                _ => j += 1,
            }
        }
        if j >= bytes.len() {
            break;
        }
        let literal = &after[..=j];
        if let Ok(s) = serde_json::from_str::<String>(literal) {
            out.push_str(&s);
        }
        rest = &after[j + 1..];
    }
    out
}

/// One broadcast day of a tournament, from competetft's schedule feed. The
/// `stage_id` joins to a tournament-overview stage tab (which carries the sheet
/// URL); the `date` is the precise broadcast start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompeteEvent {
    pub tournament_id: String,
    pub stage_id: String,
    pub date: DateTime<Utc>,
    pub event_type: String,
}

/// competetft `eventType`s that are actual broadcasts (as opposed to
/// registration windows and ladder snapshots, which are separate `__typename`s).
const BROADCAST_TYPES: &[&str] = &["TACTICIANS_CROWN", "REGIONAL_FINALS"];

/// Pull the `"key":"value"` string field out of a flat JSON object slice.
fn str_field(obj: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\":\"");
    let s = obj.find(&pat)? + pat.len();
    let e = obj[s..].find('"')? + s;
    Some(obj[s..e].to_string())
}

/// Parse the `/en-US/schedule` page into TFT broadcast events. Non-broadcast
/// milestones (registration, ladder/cup snapshots) are different `__typename`s
/// and are ignored; so are non-`tft` sports.
#[must_use]
pub fn parse_schedule(html: &str) -> Vec<CompeteEvent> {
    let blob = rsc_blob(html);
    let mut out = Vec::new();
    for chunk in blob
        .split("\"__typename\":\"CompeteEventScheduleItem\"")
        .skip(1)
    {
        // The item is a flat object (arrays use `]`, not `}`), so the first `}`
        // closes it.
        let obj = &chunk[..chunk.find('}').unwrap_or(chunk.len())];
        if str_field(obj, "sport").as_deref() != Some("tft") {
            continue;
        }
        let (Some(tournament_id), Some(stage_id), Some(date), Some(event_type)) = (
            str_field(obj, "esportsTournamentId"),
            str_field(obj, "esportsStageId"),
            str_field(obj, "date"),
            str_field(obj, "eventType"),
        ) else {
            continue;
        };
        if !BROADCAST_TYPES.contains(&event_type.as_str()) {
            continue;
        }
        let Ok(date) = DateTime::parse_from_rfc3339(&date) else {
            continue;
        };
        out.push(CompeteEvent {
            tournament_id,
            stage_id,
            date: date.with_timezone(&Utc),
            event_type,
        });
    }
    out
}

/// Parse the `/en-US/schedule` page into the season's tournament ids — every
/// `Tournament` object the site lists, independent of broadcast windows. Each id
/// drives discovery (id → overview → published sheet). Order-preserving + deduped.
#[must_use]
pub fn parse_tournament_ids(html: &str) -> Vec<String> {
    let blob = rsc_blob(html);
    let mut out = Vec::new();
    for chunk in blob.split("\"__typename\":\"Tournament\"").skip(1) {
        // The tournament's own `id` is the first field after the typename, so
        // truncating at the first `}` is enough to capture it — that brace closes
        // the nested `league` object, and `str_field` finds `id`, which precedes it.
        let obj = &chunk[..chunk.find('}').unwrap_or(chunk.len())];
        if let Some(id) = str_field(obj, "id") {
            if !out.contains(&id) {
                out.push(id);
            }
        }
    }
    out
}

/// One stage tab of a tournament: its stage id (joins to a schedule event), its
/// display title ("Day 1"/"Finals"), and the published-sheet key + gid it links.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompeteStage {
    pub stage_id: String,
    pub title: String,
    pub sheet_key: String,
    pub gid: Option<String>,
}

/// A tournament's overview: name, Set label, and its stage → sheet map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompeteTournament {
    pub id: String,
    pub name: String,
    pub set_label: String,
    pub stages: Vec<CompeteStage>,
}

/// The published-sheet key and gid from a `…/d/e/<KEY>/pubhtml#gid=<GID>` URL.
fn sheet_key_gid(url: &str) -> Option<(String, Option<String>)> {
    let key = url.split("/d/e/").nth(1)?.split('/').next()?.to_string();
    let gid = url
        .split("gid=")
        .nth(1)
        .map(|g| g.trim_matches(|c: char| !c.is_ascii_digit()).to_string())
        .filter(|g| !g.is_empty());
    Some((key, gid))
}

/// The substring of `blob` between `start` and the next `end` char after it.
fn between(blob: &str, start: &str, end: char) -> Option<String> {
    let s = blob.find(start)? + start.len();
    let e = blob[s..].find(end)? + s;
    Some(blob[s..e].to_string())
}

/// Parse a tournament `/overview` page into its name, Set label, and stage tabs.
/// The name comes from the page title (`Compete TFT | <name>`); the Set from the
/// `Set N Champion` heading; each stage is a `{id,title,url}` object whose URL is
/// a published Google Sheet.
#[must_use]
pub fn parse_tournament(id: &str, html: &str) -> CompeteTournament {
    let blob = rsc_blob(html);
    let name = between(&blob, "Compete TFT | ", '"').unwrap_or_default();
    let set_label = between(&blob, "\"children\":\"Set ", '"')
        .map(|s| format!("Set {s}"))
        .unwrap_or_default();

    let mut stages = Vec::new();
    let anchor = "\"url\":\"https://docs.google.com/spreadsheets";
    let mut from = 0;
    while let Some(rel) = blob[from..].find(anchor) {
        let url_start = from + rel + "\"url\":\"".len();
        let url_end = blob[url_start..]
            .find('"')
            .map_or(blob.len(), |e| url_start + e);
        let url = &blob[url_start..url_end];
        // The object's id + title precede its url; take the closest of each.
        let window = &blob[from..url_start];
        let back = |k: &str| -> Option<String> {
            let pat = format!("\"{k}\":\"");
            let s = window.rfind(&pat)? + pat.len();
            let e = window[s..].find('"')? + s;
            Some(window[s..e].to_string())
        };
        if let (Some(stage_id), Some(title), Some((sheet_key, gid))) =
            (back("id"), back("title"), sheet_key_gid(url))
            && !stages.iter().any(|s: &CompeteStage| s.stage_id == stage_id)
        {
            stages.push(CompeteStage {
                stage_id,
                title,
                sheet_key,
                gid,
            });
        }
        from = url_end;
    }
    CompeteTournament {
        id: id.to_string(),
        name,
        set_label,
        stages,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tournament_overview() {
        let html = include_str!("../fixtures/competetft_tournament.html");
        let t = parse_tournament("116323184504995859", html);
        assert!(t.name.contains("Tactician"));
        assert!(t.set_label.contains("Set 17"));
        let day2 = t
            .stages
            .iter()
            .find(|s| s.stage_id == "116323184505454613")
            .unwrap();
        assert_eq!(day2.title, "Day 2");
        assert!(day2.sheet_key.starts_with("2PACX-"));
        assert_eq!(day2.gid.as_deref(), Some("532077851"));
        assert_eq!(t.stages.len(), 3);
    }

    #[test]
    fn parses_schedule_feed() {
        let html = include_str!("../fixtures/competetft_schedule.html");
        let evs = parse_schedule(html);
        // Tactician's Crown = 3 broadcast days
        let tc: Vec<_> = evs
            .iter()
            .filter(|e| e.tournament_id == "116323184504995859")
            .collect();
        assert_eq!(tc.len(), 3);
        assert!(tc.iter().all(|e| e.event_type == "TACTICIANS_CROWN"));
        assert!(tc.iter().any(|e| e.stage_id == "116323184505454613"));
        // only broadcast event types leak through (no REGISTRATION/SNAPSHOT)
        assert!(
            evs.iter()
                .all(|e| e.event_type == "TACTICIANS_CROWN" || e.event_type == "REGIONAL_FINALS")
        );
        assert!(!evs.is_empty());
    }

    #[test]
    fn parse_tournament_ids_lists_the_whole_season() {
        let html = include_str!("../fixtures/competetft_schedule.html");
        let ids = parse_tournament_ids(html);
        // The schedule page embeds the full 20-tournament season.
        assert_eq!(ids.len(), 20);
        // Tactician's Crown is in the list.
        assert!(ids.contains(&"116323184504995859".to_string()));
        // Ids are unique (dedup held).
        let mut uniq = ids.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), ids.len());
    }

    #[test]
    fn parse_tournament_ids_dedupes_repeats_and_keeps_order() {
        // A synthetic RSC payload with tournament 111 listed twice, then 222.
        // Mirrors the `self.__next_f.push([n,"<escaped json>"])` wrapping that
        // `rsc_blob` decodes. Would fail if the dedup guard were removed
        // (naive push → ["111", "111", "222"]).
        let html = r#"<script>self.__next_f.push([1,"[{\"__typename\":\"Tournament\",\"id\":\"111\",\"name\":\"A\",\"league\":{}},{\"__typename\":\"Tournament\",\"id\":\"111\",\"name\":\"A again\",\"league\":{}},{\"__typename\":\"Tournament\",\"id\":\"222\",\"name\":\"B\",\"league\":{}}]"])</script>"#;
        let ids = parse_tournament_ids(html);
        assert_eq!(ids, vec!["111".to_string(), "222".to_string()]);
    }
}
