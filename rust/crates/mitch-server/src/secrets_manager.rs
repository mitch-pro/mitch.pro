//! Fetches the data encryption key from Infisical when the server is
//! configured for it (see `docs/encryption-at-rest.md`).
//!
//! Doppler needs no code here: `doppler run -- docker compose ...` puts
//! `DATA_ENCRYPTION_KEY` in the environment. Infisical's CLI can do the same
//! (`infisical run`); this module is for running without wrapping the start
//! command, using a machine identity's universal-auth credentials:
//!
//! | Variable | Meaning |
//! | --- | --- |
//! | `INFISICAL_CLIENT_ID`, `INFISICAL_CLIENT_SECRET` | Machine identity (both required to enable this) |
//! | `INFISICAL_PROJECT_ID` | Project id (required) |
//! | `INFISICAL_ENVIRONMENT` | Environment slug, default `prod` |
//! | `INFISICAL_SECRET_PATH` | Folder, default `/` |
//! | `INFISICAL_SECRET_NAME` | Secret name, default `DATA_ENCRYPTION_KEY` |
//! | `INFISICAL_URL` | Default `https://app.infisical.com` (EU: `https://eu.infisical.com`, or a self-hosted URL) |
//!
//! A key already in `DATA_ENCRYPTION_KEY` wins and no request is made. A failed
//! fetch is logged and startup carries on: `KeyRing::load` then uses the
//! bootstrap file, or refuses to start if data was encrypted under a key it
//! can't find, so an outage can never lead to a fresh key replacing the real one.

use serde_json::{json, Value};
use std::time::Duration;

const DEFAULT_URL: &str = "https://app.infisical.com";

#[derive(Debug, PartialEq, Eq)]
pub struct InfisicalConfig {
    pub base_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub project_id: String,
    pub environment: String,
    pub secret_path: String,
    pub secret_name: String,
}

impl InfisicalConfig {
    /// `None` when Infisical isn't configured (no client id or secret).
    pub fn from_env() -> Option<Self> {
        let get = |name: &str| {
            std::env::var(name)
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let client_id = get("INFISICAL_CLIENT_ID")?;
        let client_secret = get("INFISICAL_CLIENT_SECRET")?;
        Some(InfisicalConfig {
            base_url: get("INFISICAL_URL")
                .unwrap_or_else(|| DEFAULT_URL.into())
                .trim_end_matches('/')
                .to_string(),
            client_id,
            client_secret,
            project_id: get("INFISICAL_PROJECT_ID").unwrap_or_default(),
            environment: get("INFISICAL_ENVIRONMENT").unwrap_or_else(|| "prod".into()),
            secret_path: get("INFISICAL_SECRET_PATH").unwrap_or_else(|| "/".into()),
            secret_name: get("INFISICAL_SECRET_NAME")
                .unwrap_or_else(|| mitch_lib::keyring::KEY_ENV.into()),
        })
    }

    /// Credentials only go over HTTPS (plain HTTP just for a local mock).
    fn url_allowed(&self) -> bool {
        self.base_url.starts_with("https://")
            || self.base_url.starts_with("http://127.0.0.1")
            || self.base_url.starts_with("http://localhost")
    }
}

/// Loads the key from Infisical into `mitch_lib::keyring` when configured and
/// not already in the environment. Never panics and never returns the key.
pub async fn load_external_key() {
    if std::env::var(mitch_lib::keyring::KEY_ENV).is_ok_and(|v| !v.trim().is_empty()) {
        return;
    }
    let Some(cfg) = InfisicalConfig::from_env() else {
        return;
    };
    if cfg.project_id.is_empty() || !cfg.url_allowed() {
        tracing::error!(
            "[keys] Infisical is configured but INFISICAL_PROJECT_ID is missing or INFISICAL_URL is not https; not fetching the key"
        );
        return;
    }
    for attempt in 1..=3u64 {
        match fetch_secret(&cfg).await {
            Ok(value) => {
                mitch_lib::keyring::set_external_key(value);
                tracing::info!("[keys] data encryption key loaded from Infisical");
                return;
            }
            Err(e) => {
                tracing::warn!("[keys] Infisical fetch failed (attempt {attempt}/3): {e}");
                tokio::time::sleep(Duration::from_secs(attempt)).await;
            }
        }
    }
    tracing::error!("[keys] could not fetch the data encryption key from Infisical");
}

async fn fetch_secret(cfg: &InfisicalConfig) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;
    let login: Value = client
        .post(format!("{}/api/v1/auth/universal-auth/login", cfg.base_url))
        .json(&json!({ "clientId": cfg.client_id, "clientSecret": cfg.client_secret }))
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("login: {}", e.without_url()))?
        .json()
        .await
        .map_err(|e| format!("login response: {}", e.without_url()))?;
    let token = login
        .get("accessToken")
        .and_then(Value::as_str)
        .ok_or("login response had no accessToken")?;
    let secret: Value = client
        .get(format!(
            "{}/api/v3/secrets/raw/{}",
            cfg.base_url,
            urlencode(&cfg.secret_name)
        ))
        .query(&[
            ("workspaceId", cfg.project_id.as_str()),
            ("environment", cfg.environment.as_str()),
            ("secretPath", cfg.secret_path.as_str()),
        ])
        .bearer_auth(token)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("secret: {}", e.without_url()))?
        .json()
        .await
        .map_err(|e| format!("secret response: {}", e.without_url()))?;
    secret
        .pointer("/secret/secretValue")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| "secret response had no secretValue".into())
}

fn urlencode(s: &str) -> String {
    form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::Query;
    use axum::http::HeaderMap;
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use std::collections::HashMap;

    fn config(base: &str) -> InfisicalConfig {
        InfisicalConfig {
            base_url: base.into(),
            client_id: "id".into(),
            client_secret: "shh".into(),
            project_id: "proj".into(),
            environment: "prod".into(),
            secret_path: "/".into(),
            secret_name: "DATA_ENCRYPTION_KEY".into(),
        }
    }

    #[tokio::test]
    async fn fetches_the_secret_with_a_machine_identity_login() {
        let app = Router::new()
            .route(
                "/api/v1/auth/universal-auth/login",
                post(|Json(body): Json<Value>| async move {
                    assert_eq!(body["clientId"], "id");
                    assert_eq!(body["clientSecret"], "shh");
                    Json(json!({ "accessToken": "tok" }))
                }),
            )
            .route(
                "/api/v3/secrets/raw/DATA_ENCRYPTION_KEY",
                get(
                    |headers: HeaderMap, Query(q): Query<HashMap<String, String>>| async move {
                        assert_eq!(headers["authorization"], "Bearer tok");
                        assert_eq!(q["workspaceId"], "proj");
                        assert_eq!(q["environment"], "prod");
                        assert_eq!(q["secretPath"], "/");
                        Json(json!({ "secret": { "secretValue": "the-key" } }))
                    },
                ),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        assert_eq!(fetch_secret(&config(&base)).await.unwrap(), "the-key");
    }

    #[tokio::test]
    async fn a_missing_secret_is_an_error() {
        let app = Router::new()
            .route(
                "/api/v1/auth/universal-auth/login",
                post(|| async { Json(json!({ "accessToken": "tok" })) }),
            )
            .route(
                "/api/v3/secrets/raw/DATA_ENCRYPTION_KEY",
                get(|| async { Json(json!({ "secret": {} })) }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        assert!(fetch_secret(&config(&base)).await.is_err());
    }

    #[test]
    fn credentials_are_not_sent_over_plain_http() {
        assert!(config("https://app.infisical.com").url_allowed());
        assert!(config("http://127.0.0.1:9").url_allowed());
        assert!(!config("http://infisical.example.com").url_allowed());
    }
}
