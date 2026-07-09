#![recursion_limit = "256"]

#[cfg(feature = "ssr")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(feature = "ssr")]
#[tokio::main]
async fn main() {
    #[cfg(feature = "ssr")]
    mod cache_middleware {
        use axum::{
            extract::{Request, State},
            middleware::Next,
            response::{IntoResponse, Response},
        };
        use moka::future::Cache;
        use std::sync::Arc;

        pub type HtmlCache = Arc<Cache<String, (axum::http::HeaderMap, axum::body::Bytes)>>;

        pub async fn html_cache_middleware(
            State(cache): State<HtmlCache>,
            req: Request,
            next: Next,
        ) -> Response {
            if req.method() != axum::http::Method::GET || req.uri().path().starts_with("/api") {
                return next.run(req).await;
            }

            let cookie_str = req
                .headers()
                .get(axum::http::header::COOKIE)
                .and_then(|h| h.to_str().ok())
                .unwrap_or("");
            let key = format!("{}|{}", req.uri(), cookie_str);

            if let Some((headers, body_bytes)) = cache.get(&key).await {
                let mut res = body_bytes.into_response();
                *res.headers_mut() = headers;
                return res;
            }

            let res = next.run(req).await;

            if res.status().is_success() {
                let is_html = res
                    .headers()
                    .get(axum::http::header::CONTENT_TYPE)
                    .is_some_and(|ct| ct.as_bytes().starts_with(b"text/html"));

                if is_html {
                    let (parts, body) = res.into_parts();
                    match axum::body::to_bytes(body, usize::MAX).await {
                        Ok(bytes) => {
                            // Never cache Set-Cookie: stored headers are replayed
                            // to every client sharing this key, so a cached cookie
                            // would leak across users (session fixation). The
                            // originating response below keeps its own headers.
                            let mut cached_headers = parts.headers.clone();
                            cached_headers.remove(axum::http::header::SET_COOKIE);
                            cache.insert(key, (cached_headers, bytes.clone())).await;
                            let mut cached_res = bytes.into_response();
                            *cached_res.headers_mut() = parts.headers;
                            return cached_res;
                        }
                        Err(_) => {
                            return axum::response::Response::from_parts(
                                parts,
                                axum::body::Body::empty(),
                            );
                        }
                    }
                }
            }

            res
        }
    }

    use axum::Router;
    use axum::http::header;
    use axum::response::IntoResponse;
    use leptos::logging::log;
    use leptos::prelude::*;
    use leptos_axum::{LeptosRoutes, generate_route_list};
    use plaintextesports::app::*;
    use tower_http::compression::CompressionLayer;

    /// Lightweight liveness probe for Docker HEALTHCHECK / load-balancers.
    async fn healthz() -> impl IntoResponse {
        (axum::http::StatusCode::OK, "ok")
    }

    /// Serve the Web Push service worker at the site root (scope `/`).
    async fn service_worker() -> impl IntoResponse {
        (
            [
                (header::CONTENT_TYPE, "application/javascript"),
                (
                    header::HeaderName::from_static("service-worker-allowed"),
                    "/",
                ),
            ],
            include_str!("sw.js"),
        )
    }

    /// Serve a loader.io domain-verification file live from disk as plain text
    /// (404 if it has since been removed). The file is a drop-in (gitignored)
    /// discovered at startup, so the token stays out of the repo and the binary.
    async fn serve_loaderio(path: std::path::PathBuf) -> axum::response::Response {
        use axum::http::StatusCode;
        match tokio::fs::read(&path).await {
            Ok(bytes) => ([(header::CONTENT_TYPE, "text/plain")], bytes).into_response(),
            Err(_) => StatusCode::NOT_FOUND.into_response(),
        }
    }

    /// Serve one optional site-icon file from `dir` if it exists; 404 otherwise.
    /// Read live from disk so a dropped-in file serves without a restart.
    async fn serve_icon(
        dir: std::path::PathBuf,
        file: &'static str,
        content_type: &'static str,
    ) -> axum::response::Response {
        use axum::http::StatusCode;
        match tokio::fs::read(dir.join(file)).await {
            Ok(bytes) => (
                [
                    (header::CONTENT_TYPE, content_type),
                    (header::CACHE_CONTROL, "public, max-age=86400"),
                ],
                bytes,
            )
                .into_response(),
            Err(_) => StatusCode::NOT_FOUND.into_response(),
        }
    }

    /// Serve the web manifest with the icon version stamped into each local
    /// `icons[].src` (so an installed PWA reads versioned icon URLs and refreshes
    /// when the icons change — busting even iOS's sticky home-screen icon cache).
    async fn serve_manifest(dir: std::path::PathBuf) -> axum::response::Response {
        use axum::http::StatusCode;
        match tokio::fs::read_to_string(dir.join("manifest.webmanifest")).await {
            Ok(text) => {
                let body = plaintextesports::icons::manifest_with_version(
                    &text,
                    plaintextesports::icons::version(),
                );
                (
                    [
                        (header::CONTENT_TYPE, "application/manifest+json"),
                        (header::CACHE_CONTROL, "public, max-age=86400"),
                    ],
                    body,
                )
                    .into_response()
            }
            Err(_) => StatusCode::NOT_FOUND.into_response(),
        }
    }

    /// Body of the service worker's `pushsubscriptionchange` POST: the rotated-out
    /// endpoint plus the fresh subscription it was replaced with.
    #[derive(serde::Deserialize)]
    struct MigrateBody {
        old_endpoint: String,
        endpoint: String,
        p256dh: String,
        auth: String,
    }

    /// Re-key a browser's stored reminders/subscriptions onto its new push
    /// subscription when the browser rotates it (the SW posts here from
    /// `pushsubscriptionchange`), so pending reminders aren't lost. A plain JSON
    /// route rather than a Leptos server fn, since the SW calls it with `fetch`.
    async fn push_migrate(axum::Json(b): axum::Json<MigrateBody>) -> axum::http::StatusCode {
        use axum::http::StatusCode;
        let cfg = plaintextesports::config::Config::from_env();
        if cfg.db_path.is_empty() {
            return StatusCode::OK;
        }
        let new = plaintextesports::types::PushSub {
            endpoint: b.endpoint,
            p256dh: b.p256dh,
            auth: b.auth,
        };
        tokio::task::spawn_blocking(
            move || match plaintextesports::store::shared(&cfg.db_path) {
                Ok(conn) => {
                    match plaintextesports::store::migrate_endpoint(&conn, &b.old_endpoint, &new) {
                        Ok(()) => StatusCode::OK,
                        Err(e) => {
                            log!("push-migrate failed: {e}");
                            StatusCode::INTERNAL_SERVER_ERROR
                        }
                    }
                }
                Err(e) => {
                    log!("push-migrate db open failed: {e}");
                    StatusCode::INTERNAL_SERVER_ERROR
                }
            },
        )
        .await
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
    }

    // Load .env (PANDASCORE_TOKEN, DISPLAY_TZ, ...) if present.
    dotenvy::dotenv().ok();

    let conf = get_configuration(None).unwrap();
    let addr = conf.leptos_options.site_addr;
    let leptos_options = conf.leptos_options;
    let routes = generate_route_list(App);

    let html_cache: cache_middleware::HtmlCache = std::sync::Arc::new(
        moka::future::Cache::builder()
            .time_to_live(std::time::Duration::from_secs(15))
            // Bound total cached HTML: the cache key embeds the client-supplied
            // Cookie header, so a flood of distinct cookies could otherwise grow
            // the cache without limit within the TTL window. Weighed by body
            // size, evicted once the total exceeds the ceiling.
            .max_capacity(64 * 1024 * 1024)
            .weigher(
                |_key: &String, value: &(axum::http::HeaderMap, axum::body::Bytes)| {
                    value.1.len().try_into().unwrap_or(u32::MAX)
                },
            )
            .build(),
    );

    // Start polling PandaScore (or load demo data if no token is set), and the
    // Web Push reminder sender (no-op unless VAPID keys are configured).
    plaintextesports::cache::spawn_poller();
    plaintextesports::push::spawn_sender();

    // Optional site-icon / PWA assets, served from `icons_dir` when present. The
    // manifest is served separately (serve_manifest) so its icon `src`s get the
    // version stamp; the rest are static bytes.
    let icon_assets: [(&str, &str, &str); 6] = [
        ("/favicon.ico", "favicon.ico", "image/x-icon"),
        ("/favicon.svg", "favicon.svg", "image/svg+xml"),
        ("/apple-touch-icon.png", "apple-touch-icon.png", "image/png"),
        ("/icon-192.png", "icon-192.png", "image/png"),
        ("/icon-512.png", "icon-512.png", "image/png"),
        (
            "/icon-512-maskable.png",
            "icon-512-maskable.png",
            "image/png",
        ),
    ];
    let icons_dir = std::path::PathBuf::from(&plaintextesports::config::config().icons_dir);

    // Optionally serve a loader.io domain-verification file. Drop the file
    // loader.io gives you (named `loaderio-<token>.txt`) into the working
    // directory and it is served verbatim at `/<filename>`, so load tests can
    // be authorized without committing the token to the repo. Discovered at
    // startup — a newly added file needs a restart to be picked up.
    let loaderio_file = std::fs::read_dir(".").ok().and_then(|entries| {
        entries.flatten().find_map(|entry| {
            entry
                .file_name()
                .to_str()
                .filter(|name| name.starts_with("loaderio"))
                .map(|name| (format!("/{name}"), entry.path()))
        })
    });

    let mut app = Router::new().leptos_routes(&leptos_options, routes, {
        let leptos_options = leptos_options.clone();
        move || shell(leptos_options.clone())
    });
    for (path, file, content_type) in icon_assets {
        let dir = icons_dir.clone();
        app = app.route(
            path,
            axum::routing::get(move || serve_icon(dir.clone(), file, content_type)),
        );
    }
    app = app.route(
        "/manifest.webmanifest",
        axum::routing::get({
            let dir = icons_dir.clone();
            move || serve_manifest(dir.clone())
        }),
    );
    if let Some((route, path)) = loaderio_file {
        log!("serving loader.io verification file at {route}");
        app = app.route(
            &route,
            axum::routing::get(move || serve_loaderio(path.clone())),
        );
    }
    let app = app
        .route("/healthz", axum::routing::get(healthz))
        .route("/sw.js", axum::routing::get(service_worker))
        .route("/api/push-migrate", axum::routing::post(push_migrate))
        .fallback(leptos_axum::file_and_error_handler(shell))
        .with_state(leptos_options)
        .layer(axum::middleware::from_fn_with_state(
            html_cache,
            cache_middleware::html_cache_middleware,
        ))
        .layer(CompressionLayer::new());

    log!("listening on http://{}", &addr);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app.into_make_service())
        .await
        .unwrap();
}

#[cfg(not(feature = "ssr"))]
pub fn main() {
    // Client entry point lives in lib.rs (`hydrate`).
}
