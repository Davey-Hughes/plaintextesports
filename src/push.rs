//! Web Push: build + send encrypted push messages (server-only).
//!
//! Uses `web-push-native` (pure-Rust RustCrypto, no OpenSSL) to build an
//! RFC 8291-encrypted, VAPID-signed `http::Request`, which we send with our
//! existing reqwest client. A background task scans the `reminders` table and
//! delivers due reminders, pruning dead subscriptions (404/410).

use crate::config::Config;
use crate::http::DynError;
use crate::store::{self, Reminder};
use base64ct::{Base64UrlUnpadded, Encoding};
use chrono::Utc;
use std::time::Duration;
use web_push_native::jwt_simple::algorithms::ES256KeyPair;
use web_push_native::p256::PublicKey;
use web_push_native::{Auth, WebPushBuilder};

/// How often the sender scans for due reminders.
const TICK: Duration = Duration::from_secs(30);
/// Drop reminders this long after their match has started (see
/// [`store::prune_reminders`] — pruning is keyed on the start, not the notify
/// time, so a long-lead reminder for a still-upcoming match isn't reaped early
/// and re-armed into a duplicate).
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
            // Expand sport/event subscriptions into per-match reminders first.
            expand_subscriptions(&conn);

            let now = Utc::now().timestamp_millis();

            // Bring armed reminders back in line with the latest start times:
            // rewrite shifted notify times/bodies and drop canceled matches (sync),
            // returning one "earlier than scheduled" alert per match that jumped
            // earlier past a lead window, plus one "canceled" notice per canceled
            // match. Runs before the due scan so it sees the corrected times;
            // subsumed timers are marked sent below, so the due scan never
            // double-delivers them.
            let (alerts, cancel_notices) = reschedule_writes(&conn, now);
            let mut early_sent = 0usize;
            for alert in &alerts {
                match send_one(&client, &key, &cfg.vapid_subject, &alert.reminder).await {
                    Outcome::Sent => {
                        early_sent += 1;
                        for s in &alert.subsumed {
                            if let Err(e) = store::mark_reminder_sent(
                                &conn,
                                &s.endpoint,
                                s.match_id,
                                &s.sport,
                                s.lead_ms,
                            ) {
                                leptos::logging::log!(
                                    "reschedule: mark subsumed sent failed (match {}): {e}",
                                    s.match_id
                                );
                            }
                        }
                    }
                    Outcome::Gone => {
                        if let Err(e) = store::delete_endpoint(&conn, &alert.reminder.endpoint) {
                            leptos::logging::log!("reschedule: delete_endpoint failed: {e}");
                        }
                    }
                    // Leave the subsumed timers unsent — retry next tick.
                    Outcome::Failed => {}
                }
            }
            if early_sent > 0 {
                leptos::logging::log!(
                    "reschedule: sent {early_sent} 'earlier than scheduled' alert(s)"
                );
            }

            // Cancellation notices — best-effort (the rows are already dropped, so
            // a failed send only misses telling them; nothing stale can fire).
            let mut cancel_sent = 0usize;
            for notice in &cancel_notices {
                match send_one(&client, &key, &cfg.vapid_subject, notice).await {
                    Outcome::Sent => cancel_sent += 1,
                    Outcome::Gone => {
                        if let Err(e) = store::delete_endpoint(&conn, &notice.endpoint) {
                            leptos::logging::log!("reschedule: delete_endpoint failed: {e}");
                        }
                    }
                    Outcome::Failed => {}
                }
            }
            if cancel_sent > 0 {
                leptos::logging::log!("reschedule: sent {cancel_sent} 'canceled' notice(s)");
            }

            let due = match store::due_reminders(&conn, now) {
                Ok(d) => d,
                Err(e) => {
                    // Don't silently treat a read error as "nothing due" — that
                    // would drop every reminder with no trace. Log and retry next tick.
                    leptos::logging::log!("push sender: due_reminders failed: {e}");
                    Vec::new()
                }
            };

            // Send (async) without holding a DB borrow across awaits.
            let mut outcomes = Vec::with_capacity(due.len());
            for r in &due {
                let outcome = send_one(&client, &key, &cfg.vapid_subject, r).await;
                // Carry the full timer key so the apply loop marks exactly this
                // row sent (a match has one row per lead time).
                outcomes.push((
                    r.endpoint.clone(),
                    r.match_id,
                    r.sport.clone(),
                    r.lead_ms,
                    outcome,
                ));
            }

            // Apply results (sync). Only mark a reminder sent on success; a
            // transient Failed is left untouched so the next tick retries it
            // (still bounded by notify_at_ms and the 24h prune).
            for (endpoint, match_id, sport, lead_ms, outcome) in &outcomes {
                let res = match outcome {
                    Outcome::Sent => {
                        store::mark_reminder_sent(&conn, endpoint, *match_id, sport, *lead_ms)
                    }
                    Outcome::Gone => store::delete_endpoint(&conn, endpoint),
                    Outcome::Failed => continue,
                };
                if let Err(e) = res {
                    leptos::logging::log!("push sender: db update failed: {e}");
                }
            }
            if !outcomes.is_empty() {
                let sent = outcomes
                    .iter()
                    .filter(|(.., o)| *o == Outcome::Sent)
                    .count();
                leptos::logging::log!("push: sent {sent}/{} due reminder(s)", outcomes.len());
            }

            let _ = store::prune_reminders(&conn, now - PRUNE_AFTER_MS);
            tokio::time::sleep(TICK).await;
        }
    });
}

/// Reconcile armed reminders against the latest match start times (the sync,
/// DB-only half of the reschedule step). Rewrites the notify time + start-time
/// body of any timer whose match was rescheduled and drops reminders for canceled
/// matches, then returns the notifications the caller must deliver: the collapsed
/// "earlier than scheduled" alerts (one per match that jumped earlier past a lead
/// window) and the "canceled" notices (one per canceled match the subscriber still
/// had a pending reminder for). Sends are kept out of here so no `&Connection` is
/// held across a push send's await; the canceled rows are already deleted, so a
/// failed cancel send can't leave a stale reminder to fire.
fn reschedule_writes(
    conn: &rusqlite::Connection,
    now: i64,
) -> (Vec<crate::cache::CollapseAlert>, Vec<store::Reminder>) {
    let unsent = match store::unsent_reminders(conn) {
        Ok(u) => u,
        Err(e) => {
            leptos::logging::log!("reschedule: unsent_reminders failed: {e}");
            return (Vec::new(), Vec::new());
        }
    };
    if unsent.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let plan = crate::cache::current_reschedule_plan(&unsent, now);

    // Rewrite shifted timers to the new start (notify time + body).
    for u in &plan.updates {
        if let Err(e) = store::update_reminder_schedule(conn, u) {
            leptos::logging::log!("reschedule: update failed (match {}): {e}", u.match_id);
        }
    }
    // Drop reminders whose match is now canceled (remove_reminder clears every
    // lead row for the match, so repeats within a group are harmless no-ops).
    for c in &plan.cancels {
        if let Err(e) = store::remove_reminder(conn, &c.endpoint, c.match_id, &c.sport) {
            leptos::logging::log!("reschedule: cancel drop failed (match {}): {e}", c.match_id);
        }
    }
    (plan.alerts, plan.cancel_alerts)
}

/// Turn each sport/event subscription into per-match reminders — one row per
/// (lead offset × matching match). Insert-if-absent so already-sent reminders
/// aren't re-armed; opted-out matches (the `exclusions` table) are skipped.
fn expand_subscriptions(conn: &rusqlite::Connection) {
    let subs = match store::list_subscriptions(conn) {
        Ok(s) => s,
        Err(e) => {
            leptos::logging::log!("subscriptions read failed: {e}");
            return;
        }
    };
    let excluded = match store::list_exclusions(conn) {
        Ok(x) => x,
        Err(e) => {
            leptos::logging::log!("exclusions read failed: {e}");
            return;
        }
    };
    let now = Utc::now().timestamp_millis();
    for s in subs {
        for &lead_ms in &s.lead_list {
            for seed in crate::cache::scope_reminder_seeds(
                &s.scope_kind,
                &s.scope_value,
                lead_ms,
                &s.tz,
                s.hour24,
            ) {
                // Don't retroactively arm a timer whose lead window already opened
                // (the common first-start case: a long lead means every match in
                // that window is already "due"). It would fire at once; skip it.
                if !seed.is_armable(now) {
                    continue;
                }
                // A per-match opt-out blocks every timer for that match.
                if excluded.contains(&(s.endpoint.clone(), seed.match_id, seed.sport.clone())) {
                    continue;
                }
                let r = store::Reminder {
                    endpoint: s.endpoint.clone(),
                    p256dh: s.p256dh.clone(),
                    auth: s.auth.clone(),
                    match_id: seed.match_id,
                    lead_ms: seed.lead_ms,
                    notify_at_ms: seed.notify_at_ms,
                    title: seed.title,
                    body: seed.body,
                    url: seed.url,
                    sport: seed.sport,
                    league: seed.league,
                    team_a: seed.team_a,
                    team_b: seed.team_b,
                    event: seed.event,
                    tz: seed.tz,
                    hour24: seed.hour24,
                };
                if let Err(e) = store::add_reminder_if_absent(conn, &r) {
                    leptos::logging::log!(
                        "expand_subscriptions: add_reminder_if_absent failed: {e}"
                    );
                }
            }
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
        let p256dh =
            Base64UrlUnpadded::encode_string(ua_secret.public_key().to_sec1_bytes().as_ref());
        let auth = Base64UrlUnpadded::encode_string(&[3u8; 16]);

        let key = ES256KeyPair::generate();
        let r = Reminder {
            endpoint: "https://push.example.com/abc".into(),
            p256dh,
            auth,
            match_id: 1,
            lead_ms: 900_000,
            notify_at_ms: 0,
            title: "T1 vs GEN".into(),
            body: "LCK · starts soon".into(),
            url: "https://example.com/".into(),
            sport: "lol".into(),
            league: "LCK".into(),
            team_a: "T1".into(),
            team_b: "Gen.G".into(),
            event: "LCK Spring".into(),
            tz: String::new(),
            hour24: false,
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
