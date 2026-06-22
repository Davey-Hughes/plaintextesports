//! Web Push: build + send encrypted push messages (server-only).
//!
//! Uses `web-push-native` (pure-Rust RustCrypto, no OpenSSL) to build an
//! RFC 8291-encrypted, VAPID-signed `http::Request`, which we send with our
//! existing reqwest client. A background task scans the `reminders` table and
//! delivers due reminders, pruning dead subscriptions (404/410).

use crate::config::Config;
use crate::store::{self, Reminder};
use base64ct::{Base64UrlUnpadded, Encoding};
use chrono::Utc;
use std::time::Duration;
use web_push_native::jwt_simple::algorithms::ES256KeyPair;
use web_push_native::p256::PublicKey;
use web_push_native::{Auth, WebPushBuilder};

type DynError = Box<dyn std::error::Error + Send + Sync>;

/// How often the sender scans for due reminders.
const TICK: Duration = Duration::from_secs(30);
/// Drop reminders whose notify time is older than this.
const PRUNE_AFTER_MS: i64 = 24 * 60 * 60 * 1000;

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Sent,
    /// Subscription is gone (404/410) — delete it.
    Gone,
    /// Transient/other failure — give up on this reminder (best-effort).
    Failed,
}

fn classify(status: u16) -> Outcome {
    match status {
        200..=299 => Outcome::Sent,
        404 | 410 => Outcome::Gone,
        _ => Outcome::Failed,
    }
}

/// base64url-decode, tolerating accidental padding.
fn decode(b64: &str) -> Result<Vec<u8>, DynError> {
    Ok(Base64UrlUnpadded::decode_vec(b64.trim_end_matches('='))?)
}

/// Parse the VAPID private key (base64url 32-byte scalar) into a signing key.
pub fn vapid_key(cfg: &Config) -> Result<ES256KeyPair, DynError> {
    Ok(ES256KeyPair::from_bytes(&decode(&cfg.vapid_private)?)?)
}

/// Build the encrypted, VAPID-signed push request for a reminder.
fn build_push_request(
    key: &ES256KeyPair,
    subject: &str,
    r: &Reminder,
) -> Result<http::Request<Vec<u8>>, DynError> {
    let ua_public = PublicKey::from_sec1_bytes(&decode(&r.p256dh)?)?;
    let auth_bytes = decode(&r.auth)?;
    if auth_bytes.len() != 16 {
        return Err("auth secret must be 16 bytes".into());
    }
    let auth = Auth::clone_from_slice(&auth_bytes);

    let payload = serde_json::json!({
        "title": r.title,
        "body": r.body,
        "url": r.url,
        "tag": r.match_id.to_string(),
    })
    .to_string();

    Ok(WebPushBuilder::new(r.endpoint.parse()?, ua_public, auth)
        .with_vapid(key, subject)
        .build(payload.into_bytes())?)
}

async fn send_one(
    client: &reqwest::Client,
    key: &ES256KeyPair,
    subject: &str,
    r: &Reminder,
) -> Outcome {
    let request = match build_push_request(key, subject, r) {
        Ok(req) => req,
        Err(e) => {
            leptos::logging::log!("push build failed (match {}): {e}", r.match_id);
            return Outcome::Failed;
        }
    };
    let (parts, body) = request.into_parts();
    let mut rb = client.post(parts.uri.to_string());
    for (name, value) in parts.headers.iter() {
        rb = rb.header(name.as_str(), value.as_bytes());
    }
    match rb.body(body).send().await {
        Ok(resp) => classify(resp.status().as_u16()),
        Err(e) => {
            leptos::logging::log!("push send error (match {}): {e}", r.match_id);
            Outcome::Failed
        }
    }
}

/// Start the background reminder sender. No-op unless Web Push is configured and
/// a DB is available (reminders are stored there).
pub fn spawn_sender() {
    let cfg = Config::from_env();
    if !cfg.push_enabled() {
        leptos::logging::log!("Web Push disabled (set VAPID_PUBLIC_KEY/PRIVATE_KEY/SUBJECT)");
        return;
    }
    if cfg.db_path.is_empty() {
        leptos::logging::log!("Web Push needs DB_PATH for reminder storage; disabled");
        return;
    }
    let key = match vapid_key(&cfg) {
        Ok(k) => k,
        Err(e) => {
            leptos::logging::log!("invalid VAPID_PRIVATE_KEY: {e}");
            return;
        }
    };

    tokio::spawn(async move {
        let conn = match store::open(&cfg.db_path) {
            Ok(c) => c,
            Err(e) => {
                leptos::logging::log!("push sender: db open failed: {e}");
                return;
            }
        };
        let client = match reqwest::Client::builder().build() {
            Ok(c) => c,
            Err(e) => {
                leptos::logging::log!("push sender: http client failed: {e}");
                return;
            }
        };
        leptos::logging::log!("Web Push sender started");

        loop {
            // Expand game/event subscriptions into per-match reminders first.
            expand_subscriptions(&conn);

            let now = Utc::now().timestamp_millis();
            let due = store::due_reminders(&conn, now).unwrap_or_default();

            // Send (async) without holding a DB borrow across awaits.
            let mut outcomes = Vec::with_capacity(due.len());
            for r in &due {
                let outcome = send_one(&client, &key, &cfg.vapid_subject, r).await;
                outcomes.push((r.endpoint.clone(), r.match_id, outcome));
            }

            // Apply results (sync).
            for (endpoint, match_id, outcome) in &outcomes {
                let res = match outcome {
                    Outcome::Gone => store::delete_endpoint(&conn, endpoint),
                    _ => store::mark_reminder_sent(&conn, endpoint, *match_id),
                };
                if let Err(e) = res {
                    leptos::logging::log!("push sender: db update failed: {e}");
                }
            }
            if !outcomes.is_empty() {
                let sent = outcomes.iter().filter(|(_, _, o)| *o == Outcome::Sent).count();
                leptos::logging::log!("push: sent {sent}/{} due reminder(s)", outcomes.len());
            }

            let _ = store::prune_reminders(&conn, now - PRUNE_AFTER_MS);
            tokio::time::sleep(TICK).await;
        }
    });
}

/// Turn each game/event subscription into per-match reminders (insert-if-absent
/// so already-sent reminders aren't re-armed).
fn expand_subscriptions(conn: &rusqlite::Connection) {
    let subs = match store::list_subscriptions(conn) {
        Ok(s) => s,
        Err(e) => {
            leptos::logging::log!("subscriptions read failed: {e}");
            return;
        }
    };
    for s in subs {
        for seed in crate::cache::scope_reminder_seeds(&s.scope_kind, &s.scope_value, s.lead_ms) {
            let r = store::Reminder {
                endpoint: s.endpoint.clone(),
                p256dh: s.p256dh.clone(),
                auth: s.auth.clone(),
                match_id: seed.match_id,
                notify_at_ms: seed.notify_at_ms,
                title: seed.title,
                body: seed.body,
                url: seed.url,
                game: seed.game,
                league: seed.league,
            };
            let _ = store::add_reminder_if_absent(conn, &r);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_status() {
        assert_eq!(classify(201), Outcome::Sent);
        assert_eq!(classify(200), Outcome::Sent);
        assert_eq!(classify(404), Outcome::Gone);
        assert_eq!(classify(410), Outcome::Gone);
        assert_eq!(classify(429), Outcome::Failed);
        assert_eq!(classify(500), Outcome::Failed);
    }

    #[test]
    fn decode_tolerates_padding() {
        // "hi" => aGk
        assert_eq!(decode("aGk").unwrap(), b"hi");
        assert_eq!(decode("aGk=").unwrap(), b"hi");
    }

    #[test]
    fn build_push_request_produces_signed_encrypted_request() {
        use web_push_native::p256::SecretKey;
        // A deterministic "browser" subscription keypair.
        let ua_secret = SecretKey::from_slice(&[7u8; 32]).unwrap();
        let p256dh = Base64UrlUnpadded::encode_string(
            ua_secret.public_key().to_sec1_bytes().as_ref(),
        );
        let auth = Base64UrlUnpadded::encode_string(&[3u8; 16]);

        let key = ES256KeyPair::generate();
        let r = Reminder {
            endpoint: "https://push.example.com/abc".into(),
            p256dh,
            auth,
            match_id: 1,
            notify_at_ms: 0,
            title: "T1 vs GEN".into(),
            body: "LCK · starts soon".into(),
            url: "https://example.com/".into(),
            game: "lol".into(),
            league: "LCK".into(),
        };

        let req = build_push_request(&key, "mailto:dev@example.com", &r).expect("build");
        assert_eq!(req.method(), http::Method::POST);
        assert!(req.headers().contains_key("authorization"));
        assert_eq!(
            req.headers().get("content-encoding").map(|v| v.as_bytes()),
            Some(b"aes128gcm".as_ref())
        );
        assert!(!req.body().is_empty());
    }
}
