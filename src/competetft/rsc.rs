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

#[cfg(test)]
mod tests {
    use super::*;

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
}
