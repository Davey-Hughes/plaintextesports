//! Twitch GraphQL (server-only, UNOFFICIAL): reads the official co-streamer list
//! `User.adProperties.costreamers` via the public web client-id. Undocumented,
//! unsanctioned, and may break without notice — gated behind
//! `twitch_discovery.gql_costreamers` (off by default) until validated against a
//! live co-streamed event. Any error ⇒ empty, so the page never depends on it.

use serde::Deserialize;

const GQL_URL: &str = "https://gql.twitch.tv/gql";
/// The public web client-id Twitch ships in its site markup (not a secret).
const WEB_CLIENT_ID: &str = "kimne78kx3ncx6brgo4mv6wki5h1ko";

#[derive(Deserialize)]
struct Resp {
    #[serde(default)]
    data: Option<DataField>,
}
#[derive(Deserialize)]
struct DataField {
    #[serde(default)]
    user: Option<UserField>,
}
#[derive(Deserialize)]
struct UserField {
    #[serde(default, rename = "adProperties")]
    ad_properties: Option<AdField>,
}
#[derive(Deserialize)]
struct AdField {
    #[serde(default)]
    costreamers: Option<Vec<CoField>>,
}
#[derive(Deserialize)]
struct CoField {
    #[serde(default)]
    login: String,
}

/// Extract the co-streamer logins (lowercased, empties dropped) from a GQL
/// response body. Malformed / null ⇒ empty, never panics.
pub fn parse_costreamers(json: &str) -> Vec<String> {
    serde_json::from_str::<Resp>(json)
        .ok()
        .and_then(|r| r.data)
        .and_then(|d| d.user)
        .and_then(|u| u.ad_properties)
        .and_then(|a| a.costreamers)
        .unwrap_or_default()
        .into_iter()
        .map(|c| c.login.to_ascii_lowercase())
        .filter(|l| !l.is_empty())
        .collect()
}

/// The official co-streamer logins configured for `login`'s channel, or empty on
/// any error / when none. UNOFFICIAL GQL call — see module header.
pub async fn costreamers(login: &str) -> Vec<String> {
    // Sanitize: logins are alphanumeric/underscore; strip anything else so the
    // inlined query string can't be broken out of.
    let safe: String = login
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    if safe.is_empty() {
        return Vec::new();
    }
    let query = format!("query{{user(login:\"{safe}\"){{adProperties{{costreamers{{login}}}}}}}}");
    let body = serde_json::json!({ "query": query });
    let Ok(resp) = crate::twitch::client()
        .post(GQL_URL)
        .header("Client-Id", WEB_CLIENT_ID)
        .json(&body)
        .send()
        .await
    else {
        return Vec::new();
    };
    if !resp.status().is_success() {
        return Vec::new();
    }
    let Ok(text) = resp.text().await else {
        return Vec::new();
    };
    parse_costreamers(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_costreamers_extracts_logins() {
        let json = r#"{"data":{"user":{"adProperties":{"costreamers":[
            {"login":"Caedrel"},{"login":"sneaky"},{"login":""}
        ]}}}}"#;
        assert_eq!(parse_costreamers(json), vec!["caedrel", "sneaky"]); // lowercased, empty dropped
                                                                        // Empty list / null / malformed → empty, no panic.
        assert!(
            parse_costreamers(r#"{"data":{"user":{"adProperties":{"costreamers":[]}}}}"#)
                .is_empty()
        );
        assert!(parse_costreamers(r#"{"data":{"user":null}}"#).is_empty());
        assert!(parse_costreamers("nonsense").is_empty());
    }
}
