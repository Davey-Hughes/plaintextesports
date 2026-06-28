#![recursion_limit = "256"]

#[cfg(feature = "ssr")]
#[tokio::main]
async fn main() {
    use axum::http::header;
    use axum::response::IntoResponse;
    use axum::Router;
    use leptos::logging::log;
    use leptos::prelude::*;
    use leptos_axum::{generate_route_list, LeptosRoutes};
    use plaintextesports::app::*;
    use tower_http::compression::CompressionLayer;

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
        match plaintextesports::store::shared(&cfg.db_path) {
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
        }
    }

    // Load .env (PANDASCORE_TOKEN, DISPLAY_TZ, ...) if present.
    dotenvy::dotenv().ok();

    let conf = get_configuration(None).unwrap();
    let addr = conf.leptos_options.site_addr;
    let leptos_options = conf.leptos_options;
    let routes = generate_route_list(App);

    // Start polling PandaScore (or load demo data if no token is set), and the
    // Web Push reminder sender (no-op unless VAPID keys are configured).
    plaintextesports::cache::spawn_poller();
    plaintextesports::push::spawn_sender();

    let app = Router::new()
        .leptos_routes(&leptos_options, routes, {
            let leptos_options = leptos_options.clone();
            move || shell(leptos_options.clone())
        })
        .route("/sw.js", axum::routing::get(service_worker))
        .route("/api/push-migrate", axum::routing::post(push_migrate))
        .fallback(leptos_axum::file_and_error_handler(shell))
        .with_state(leptos_options)
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
