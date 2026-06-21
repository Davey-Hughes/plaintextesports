#![recursion_limit = "256"]

#[cfg(feature = "ssr")]
#[tokio::main]
async fn main() {
    use axum::Router;
    use leptos::logging::log;
    use leptos::prelude::*;
    use leptos_axum::{generate_route_list, LeptosRoutes};
    use plaintextesports::app::*;
    use tower_http::compression::CompressionLayer;

    // Load .env (PANDASCORE_TOKEN, DISPLAY_TZ, ...) if present.
    dotenvy::dotenv().ok();

    let conf = get_configuration(None).unwrap();
    let addr = conf.leptos_options.site_addr;
    let leptos_options = conf.leptos_options;
    let routes = generate_route_list(App);

    // Start polling PandaScore (or load demo data if no token is set).
    plaintextesports::cache::spawn_poller();

    let app = Router::new()
        .leptos_routes(&leptos_options, routes, {
            let leptos_options = leptos_options.clone();
            move || shell(leptos_options.clone())
        })
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
