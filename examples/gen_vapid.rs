//! Generate a VAPID keypair for Web Push reminders — no Node/npx required.
//!
//! Run:  cargo run --example gen_vapid --features ssr
//!
//! Prints base64url keys in the same format as `npx web-push generate-vapid-keys`,
//! ready to paste into `.env`.

use base64ct::{Base64UrlUnpadded, Encoding};
use web_push_native::p256::SecretKey;
use web_push_native::p256::elliptic_curve::sec1::ToEncodedPoint;

fn main() {
    // 32 random bytes from the OS → a P-256 private scalar (retry on the
    // astronomically rare invalid draw).
    let secret = loop {
        let mut seed = [0u8; 32];
        getrandom::getrandom(&mut seed).expect("OS RNG unavailable");
        if let Ok(s) = SecretKey::from_slice(&seed) {
            break s;
        }
    };

    let private = Base64UrlUnpadded::encode_string(secret.to_bytes().as_slice());
    let point = secret.public_key().to_encoded_point(false); // uncompressed (65 bytes)
    let public = Base64UrlUnpadded::encode_string(point.as_bytes());

    println!("VAPID_PUBLIC_KEY={public}");
    println!("VAPID_PRIVATE_KEY={private}");
    println!("VAPID_SUBJECT=mailto:you@example.com");
}
