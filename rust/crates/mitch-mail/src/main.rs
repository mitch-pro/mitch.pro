//! mitch-mail — Rust port of the mail pipeline (plan Step 2).
//!
//! HTTP service exposing the send operations of the three nodemailer CLIs
//! (`mail/send_email.js` / `mail/noreply_send.js` / `mail/support_send.js`)
//! plus the `mail/imap_watcher.js` loop as a background task. The JS scripts
//! become shims that forward here and fall back to nodemailer when this
//! service is unreachable.

// Tests may unwrap; production code may not (Cargo.toml lints).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use mitch_lib::data::DataStore;
use serde_json::json;
use std::sync::Arc;

mod config;
mod send;
mod template;
mod watcher;

use config::MailConfig;
use watcher::WatcherStatus;

#[derive(Clone)]
struct AppState {
    cfg: Arc<MailConfig>,
    store: Arc<DataStore>,
    watcher: Arc<WatcherStatus>,
}

async fn healthz() -> Json<serde_json::Value> {
    Json(json!({ "ok": true }))
}

async fn status() -> Json<serde_json::Value> {
    let configured =
        |user, pass| !config::env_trim(user).is_empty() && !config::env_trim(pass).is_empty();
    Json(json!({
        "ok": true,
        "senders": {
            "noreply": configured("NOREPLY_USER", "NOREPLY_PASS"),
            "gmail": configured("GMAIL_USER", "GMAIL_PASS"),
            "support": configured("SUPPORT_USER", "SUPPORT_PASS"),
        }
    }))
}

async fn watch_status(State(state): State<AppState>) -> (StatusCode, Json<serde_json::Value>) {
    if state.watcher.healthy() {
        (StatusCode::OK, Json(json!({ "ok": true })))
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "ok": false })),
        )
    }
}

async fn send_handler(
    State(state): State<AppState>,
    Json(req): Json<send::SendRequest>,
) -> (StatusCode, Json<send::SendResponse>) {
    let cfg = state.cfg.clone();
    let store = state.store.clone();
    let result = tokio::task::spawn_blocking(move || {
        let prepared = send::prepare(&store, &cfg, &req)?;
        if req.dry_run {
            Ok(send::SendResponse {
                ok: true,
                error: None,
                dry_run: Some(send::DryRunArtifact {
                    text_body: prepared.text_body,
                    html_body: prepared.html_body,
                    subject: prepared.subject,
                    from: prepared.from,
                    headers: prepared.headers,
                }),
            })
        } else {
            send::deliver(&prepared)?;
            Ok(send::SendResponse {
                ok: true,
                error: None,
                dry_run: None,
            })
        }
    })
    .await;

    match result {
        Ok(Ok(resp)) => (StatusCode::OK, Json(resp)),
        Ok(Err(err)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(send::SendResponse {
                ok: false,
                error: Some(err),
                dry_run: None,
            }),
        ),
        Err(join_err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(send::SendResponse {
                ok: false,
                error: Some(format!("send task panicked: {join_err}")),
                dry_run: None,
            }),
        ),
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    // .env lives at the base dir and must load BEFORE MailConfig resolves
    // MAIL_RS_PORT / hosts — same ordering the JS scripts use.
    let base_dir = std::env::var("MITCH_BASE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
    config::load_env_file(&base_dir.join(".env"));
    config::ensure_secrets(&[
        "GMAIL_USER",
        "GMAIL_PASS",
        "GMAIL_USER_ALT",
        "GMAIL_PASS_ALT",
        "NOREPLY_USER",
        "NOREPLY_PASS",
        "SUPPORT_USER",
        "SUPPORT_PASS",
    ]);

    let cfg = Arc::new(MailConfig::load());
    let port = cfg.port;
    let store = Arc::new(
        DataStore::open(&cfg.base_dir, &cfg.data_dir)
            .unwrap_or_else(|e| panic!("open data store at {}: {e}", cfg.data_dir.display())),
    );

    // Watcher task replaces `mail/imap_watcher.js` (supervised by server.js's
    // shim, which defers to this service while /watch/status is healthy).
    let watcher = Arc::new(WatcherStatus::default());
    let has_imap_creds = !config::env_trim("SUPPORT_USER").is_empty()
        && !config::env_trim("SUPPORT_PASS").is_empty();
    if has_imap_creds {
        watcher::spawn_watcher(cfg.clone(), store.clone(), watcher.clone());
        tracing::info!("IMAP watcher started (host {})", cfg.imap_host);
    } else {
        tracing::info!("IMAP watcher disabled (SUPPORT_USER or SUPPORT_PASS not set)");
    }

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/status", get(status))
        .route("/watch/status", get(watch_status))
        .route("/send", post(send_handler))
        .with_state(AppState {
            cfg,
            store,
            watcher,
        });
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
        .await
        .unwrap_or_else(|e| panic!("bind 0.0.0.0:{port}: {e}"));
    tracing::info!("mitch-mail listening on 0.0.0.0:{port}");
    axum::serve(listener, app)
        .await
        .unwrap_or_else(|e| panic!("serve: {e}"));
}
