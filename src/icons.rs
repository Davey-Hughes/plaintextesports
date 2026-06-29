//! Optional site-icon cache-busting.
//!
//! Icon files are served at stable URLs (`/icon-512.png`, …) with a day-long
//! `Cache-Control`, so changing an icon's *contents* would otherwise be masked by
//! browser, CDN, and (notoriously) iOS home-screen icon caches. We derive a short
//! content token from the icon files and append it as `?v=<token>` to every icon
//! URL — both in the `<head>` links and in the manifest's `icons[].src` (which is
//! what an installed PWA reads) — so any icon change yields fresh URLs that no
//! cache can satisfy from a stale entry. The token is computed once at startup, so
//! a regenerated icon set takes effect on the next restart (same model as the
//! `<head>` presence scan).

use std::path::Path;
use std::sync::OnceLock;

/// The icon files whose contents define the version token (and which carry `?v=`).
const ICON_FILES: [&str; 7] = [
    "favicon.ico",
    "favicon.svg",
    "apple-touch-icon.png",
    "icon-192.png",
    "icon-512.png",
    "icon-512-maskable.png",
    "manifest.webmanifest",
];

/// A short hex token derived from the contents of the present icon files in `dir`.
/// Changes whenever any icon file changes; `None` when no icon files exist (so we
/// append nothing). Pure over the filesystem.
fn compute_token(dir: &Path) -> Option<String> {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut any = false;
    for name in ICON_FILES {
        if let Ok(bytes) = std::fs::read(dir.join(name)) {
            any = true;
            name.hash(&mut hasher);
            bytes.hash(&mut hasher);
        }
    }
    any.then(|| format!("{:016x}", hasher.finish()))
}

/// The process-wide icon version, computed once at startup from `config().icons_dir`.
/// Empty string when no icons are present (append nothing). Otherwise append as
/// `?v=<version()>` to icon URLs.
pub fn version() -> &'static str {
    static V: OnceLock<String> = OnceLock::new();
    V.get_or_init(|| {
        compute_token(Path::new(&crate::config::config().icons_dir)).unwrap_or_default()
    })
}

/// Append `?v=<token>` to each local (`/…`) icon `src` in a web-manifest JSON
/// document, so the manifest an installed PWA reads points at versioned icon URLs.
/// Returns the input unchanged when `token` is empty, the JSON can't be parsed, or
/// a `src` is external / already carries a query.
pub fn manifest_with_version(text: &str, token: &str) -> String {
    if token.is_empty() {
        return text.to_string();
    }
    let Ok(mut doc) = serde_json::from_str::<serde_json::Value>(text) else {
        return text.to_string();
    };
    if let Some(icons) = doc.get_mut("icons").and_then(|i| i.as_array_mut()) {
        for icon in icons.iter_mut() {
            let versioned = match icon.get("src").and_then(|s| s.as_str()) {
                Some(src) if src.starts_with('/') && !src.contains('?') => {
                    format!("{src}?v={token}")
                }
                _ => continue,
            };
            icon["src"] = serde_json::Value::String(versioned);
        }
    }
    serde_json::to_string(&doc).unwrap_or_else(|_| text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn token_none_when_empty_stable_when_unchanged_changes_on_edit() {
        let dir = std::env::temp_dir().join(format!("pte_iconver_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        assert!(compute_token(&dir).is_none(), "no icons -> no token");

        fs::write(dir.join("icon-512.png"), b"AAAA").unwrap();
        let t1 = compute_token(&dir).expect("token once a file exists");
        assert_eq!(
            compute_token(&dir).as_deref(),
            Some(t1.as_str()),
            "same contents -> same token (stable across restarts)"
        );

        fs::write(dir.join("icon-512.png"), b"BBBB").unwrap();
        assert_ne!(
            compute_token(&dir).as_deref(),
            Some(t1.as_str()),
            "changed contents -> different token (busts caches)"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn manifest_versions_local_icon_srcs_only() {
        let m = r#"{"name":"pte","icons":[
            {"src":"/icon-192.png","sizes":"192x192"},
            {"src":"/icon-512.png","sizes":"512x512","purpose":"maskable"},
            {"src":"https://cdn.example/x.png","sizes":"48x48"}
        ]}"#;
        let out = manifest_with_version(m, "abc123");
        assert!(
            out.contains("/icon-192.png?v=abc123"),
            "local 192 versioned"
        );
        assert!(
            out.contains("/icon-512.png?v=abc123"),
            "local 512 versioned"
        );
        assert!(
            out.contains("https://cdn.example/x.png"),
            "external src kept"
        );
        assert!(!out.contains("x.png?v="), "external src not versioned");

        // Empty token or unparseable input -> returned unchanged.
        assert_eq!(manifest_with_version(m, ""), m);
        assert_eq!(manifest_with_version("not json", "abc123"), "not json");
    }
}
