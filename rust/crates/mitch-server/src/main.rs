//! mitch-server — the Rust HTTP core for mitch.pro / rjuhsd.school /
//! sexypickleclub.com.
//!
//! `main.rs` only wires: config, State build, router assembly, serve. All
//! handler logic lives in the per-subsystem modules (one concern per file)
//! and `mitch-lib`.
//!
//! Step 4 scope: host routing, static serving, HTML injection pipeline,
//! request prelude, /enroll/ health. Session-dependent pieces are stubbed
//! (Step 6); API routes are later steps.

// Tests assert on panic-y outcomes with unwrap/expect freely (mitch-lib does
// the same via lib.rs:17); the binary itself keeps them denied.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use axum::extract::FromRequestParts;
use axum::extract::State;
use axum::http::Request;
use axum::response::Response;
use axum::Router;
use std::sync::Arc;

mod env_file;
mod errors;
mod handler;
mod hosts;
mod inject;
mod manifests;
mod pipeline;
mod proxmox_desktop;
mod routes;
mod state;
mod static_files;
mod workers;
mod workers_email;
mod workers_site;
mod workers_vm;
mod ws;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,tower=warn,hyper=warn".into()),
        )
        .init();

    if mitch_lib::auth::god_mode_enabled() {
        let bar = "=".repeat(60);
        tracing::warn!(
            "\n{bar}\n  GOD_MODE=1 is active — EVERY visitor, logged in or not,\n  is auto-logged-in as admin@mitch.pro with full\n  owner/admin/moderator privileges. LOCAL DEV ONLY.\n  Refuses to activate when NODE_ENV=production.\n{bar}"
        );
    }

    let cfg = hosts::SiteConfig::load();
    let store = Arc::new(
        mitch_lib::data::DataStore::open(&cfg.base_dir, &cfg.data_dir)
            .unwrap_or_else(|e| panic!("open data store at {}: {e}", cfg.data_dir.display())),
    );
    let state = Arc::new(state::AppState::new(cfg.clone(), store.clone()));

    mitch_lib::log::log_rewrite(
        &store,
        "info",
        "mitch-server (rust) core skeleton listening — host routing, static serving, HTML pipeline ported",
    )
    .await;

    let app: Router = Router::new().fallback(get_any).with_state(state.clone());

    // Background interval workers (canvas 30s flush, heatmap sweep, …).
    crate::workers::spawn(state);

    let port = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(6801_u16);
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
        .await
        .unwrap_or_else(|e| panic!("bind 0.0.0.0:{port}: {e}"));
    tracing::info!("mitch-server (rust) listening on 0.0.0.0:{port}");
    axum::serve(listener, app)
        .await
        .unwrap_or_else(|e| panic!("serve: {e}"));
}

/// Every request funnels through the ported flow, exactly like bun's single
/// `handleRequest`.
async fn get_any(
    State(state): State<Arc<state::AppState>>,
    req: Request<axum::body::Body>,
) -> Response {
    let (mut parts, body) = req.into_parts();
    let method = parts.method.clone();
    let headers = parts.headers.clone();
    let uri = parts.uri.clone();
    // The WS upgrade half-connection lives in the request extensions (hyper
    // inserts it for upgrade requests); handler.rs's `/ws` arm needs it, and
    // this is the only point that owns the full request. Extraction fails
    // (None) for any non-upgrade request.
    let ws_upgrade = axum::extract::ws::WebSocketUpgrade::from_request_parts(&mut parts, &())
        .await
        .ok();
    // Read the body (empty for GET/HEAD; bounded for API POSTs). The JS
    // enforces its caps inside the handlers — the largest chat JSON envelope
    // is ~2.3MB and raw attachment uploads reach 250MB — so the read cap is
    // path-aware. A body that overflows the cap is replaced by cap+1 zero
    // bytes so the route's own length check fires (the JS
    // readRequestTextLimited throw), instead of an empty body silently
    // parsing to {}.
    let cap = match uri.path() {
        "/api/dm/attachment/upload" => crate::routes::dm::upload_body_cap(),
        "/api/dm/send" => mitch_lib::dm::max_chat_json_body_bytes(),
        // Matrix media uploads (photos, GIFs, short videos posted in chat)
        // proxy straight through to Conduit — without a path-specific cap
        // here they fell into the 256KB default below and got silently
        // truncated to cap+1 zero bytes, which Conduit then stored as a
        // corrupt file that always failed to load.
        p if p.starts_with("/_matrix/media/") => 100 * 1024 * 1024,
        _ => 256 * 1024,
    };
    let body_bytes = match axum::body::to_bytes(body, cap).await {
        Ok(b) => b,
        Err(_) => axum::body::Bytes::from(vec![0u8; cap + 1]),
    };
    handler::handle(state, method, &uri, &headers, &body_bytes, ws_upgrade).await
}
