use super::{forbidden, AdminCtx, Resp};
use crate::{errors::json_resp, state::AppState};
use axum::http::Method;
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

pub async fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    body: &Value,
    ctx: &AdminCtx,
) -> Resp {
    if !path.starts_with("/api/admin/email/") {
        return None;
    }
    // The global gate checks the session and passphrase. Moderators cannot send mail.
    if !ctx.is_admin(state) {
        return Some(forbidden());
    }
    let client = reqwest::Client::new();
    let base = crate::routes::push::mail_service_url();
    match (method, path) {
        (&Method::GET, "/api/admin/email/status") => {
            let result = client
                .get(format!("{base}/status"))
                .timeout(Duration::from_secs(5))
                .send()
                .await;
            let senders = match result {
                Ok(response) if response.status().is_success() => {
                    response.json::<Value>().await.ok().and_then(|v| {
                        (v["ok"] == true).then(|| {
                            json!({
                                "noreply": v["senders"]["noreply"].as_bool().unwrap_or(false),
                                "gmail": v["senders"]["gmail"].as_bool().unwrap_or(false),
                                "support": v["senders"]["support"].as_bool().unwrap_or(false),
                            })
                        })
                    })
                }
                _ => None,
            };
            Some(json_resp(
                200,
                json!({
                    "ok": true, "connected": senders.is_some(), "senders": senders,
                }),
            ))
        }
        (&Method::POST, "/api/admin/email/preview" | "/api/admin/email/test") => {
            let template = body["template"].as_str().unwrap_or("security");
            let Some((subject, html)) = test_message(state, template) else {
                return Some(json_resp(400, json!({"error": "Unknown email template."})));
            };
            if path.ends_with("/preview") {
                return Some(json_resp(
                    200,
                    json!({
                        "ok": true, "html": html, "subject": subject,
                    }),
                ));
            }
            if body
                .as_object()
                .is_none_or(|obj| obj.keys().any(|k| k != "template" && k != "to"))
            {
                return Some(json_resp(
                    400,
                    json!({"error": "Only the template and recipient may be selected."}),
                ));
            }
            let to = body["to"].as_str().map(str::trim).unwrap_or("");
            if to.is_empty() || !to.contains('@') || to.contains('\r') || to.contains('\n') {
                return Some(json_resp(
                    400,
                    json!({"error": "Please provide a valid recipient email address."}),
                ));
            }
            let result = send_test(&client, &base, to, &subject, &html).await;
            mitch_lib::admin::log_admin_action(
                &state.store,
                &state.cfg.data_dir,
                &ctx.email(state),
                "email_test",
                json!({"to": to, "template": template, "accepted": result.is_ok()}),
            );
            Some(match result {
                Ok(()) => json_resp(200, json!({"ok": true, "to": to})),
                Err(error) => json_resp(502, json!({"error": error})),
            })
        }
        (&Method::GET | &Method::POST, _) => {
            Some(json_resp(404, json!({"error": "Unknown email endpoint."})))
        }
        _ => Some(json_resp(405, json!({"error": "Method not allowed."}))),
    }
}

fn test_message(state: &Arc<AppState>, template: &str) -> Option<(String, String)> {
    // Empty recipient omits the unsubscribe link and avoids changing account data in previews.
    let (label, html) = match template {
        "security" => ("Security verification", crate::routes::me::security::make_verification_code_html(
            state, "this test", "123456", 10.0, "",
        ).replace(
            "⚠️ This verification code is active and valid for <strong>10 minutes</strong>. If you did not request this action, please secure your account.",
            "This sample code cannot authorize an account action.",
        )),
        "signup" | "login" | "reset" => {
            let label = match template {
                "signup" => "Verify your email",
                "login" => "Sign in",
                _ => "Reset your password",
            };
            (label, crate::routes::auth::make_verification_code_html(state, label, "123456", 10, ""))
        }
        _ => return None,
    };
    let banner = "<p style=\"padding:12px;text-align:center;background:#fff3cd;color:#332701;\">Test email only. The sample code 123456 does not authorize any account action.</p>";
    Some((
        format!("[Test] {label} - mitchdog.com"),
        html.replacen("<table ", &format!("{banner}<table "), 1),
    ))
}

async fn send_test(
    client: &reqwest::Client,
    base: &str,
    to: &str,
    subject: &str,
    html: &str,
) -> Result<(), &'static str> {
    let response = client
        .post(format!("{base}/send"))
        .timeout(Duration::from_secs(30))
        .json(&json!({
            "sender": "noreply", "to": to, "subject": subject,
            "body": html, "raw": true, "dry_run": false,
        }))
        .send()
        .await
        .map_err(|_| {
            "No delivery confirmation from mitch-mail-rs. Check the inbox before retrying."
        })?;
    let success = response.status().is_success();
    let result = response.json::<Value>().await.unwrap_or(Value::Null);
    if success && result["ok"] == true {
        Ok(())
    } else {
        Err("mitch-mail-rs did not confirm SMTP acceptance. Check the inbox and sender configuration before retrying.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{http::StatusCode, routing::post, Json, Router};
    use mitch_lib::{auth, data::DataStore};

    struct Fixture {
        state: Arc<AppState>,
        dir: std::path::PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "mitch-email-{}",
                mitch_lib::crypto::random_bytes_hex(8)
            ));
            std::fs::create_dir_all(dir.join("data")).unwrap();
            let cfg = crate::hosts::SiteConfig {
                base_dir: dir.clone(),
                data_dir: dir.join("data"),
                ..crate::hosts::SiteConfig::load()
            };
            let store = Arc::new(DataStore::open(&dir, &cfg.data_dir).unwrap());
            store
                .write_document(
                    &cfg.data_dir.join("admins.json"),
                    &json!({"owners": ["admin@example.test"]}),
                )
                .unwrap();
            Self {
                state: Arc::new(AppState::new(cfg, store)),
                dir,
            }
        }

        fn ctx(&self, email: &str) -> AdminCtx {
            let sid = auth::make_email_id(email, 0, &self.state.id_secret);
            auth::names_set(&self.state.store, &sid, email);
            AdminCtx {
                cookies: auth::Cookies::default(),
                sid,
                ip: "127.0.0.1".into(),
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.dir).ok();
        }
    }

    #[tokio::test]
    async fn email_tools_require_admin_and_validate_parameters() {
        let fixture = Fixture::new();
        let member = fixture.ctx("member@example.test");
        for (method, path) in [
            (Method::GET, "/api/admin/email/status"),
            (Method::POST, "/api/admin/email/preview"),
            (Method::POST, "/api/admin/email/test"),
        ] {
            assert_eq!(
                handle(&fixture.state, &method, path, &json!({}), &member)
                    .await
                    .unwrap()
                    .status(),
                403
            );
        }
        let admin = fixture.ctx("admin@example.test");
        for key in ["cc", "bcc", "sender", "subject", "body", "dry_run"] {
            let payload = json!({"template": "security", "to": "test@example.test", key: "other@example.test"});
            assert_eq!(
                handle(
                    &fixture.state,
                    &Method::POST,
                    "/api/admin/email/test",
                    &payload,
                    &admin
                )
                .await
                .unwrap()
                .status(),
                400,
                "{key}"
            );
        }
        for invalid_to in ["", "invalid-address", "bad@test\r\nbcc:evil@example.test"] {
            let payload = json!({"template": "security", "to": invalid_to});
            assert_eq!(
                handle(
                    &fixture.state,
                    &Method::POST,
                    "/api/admin/email/test",
                    &payload,
                    &admin
                )
                .await
                .unwrap()
                .status(),
                400,
                "{invalid_to}"
            );
        }
        assert_eq!(
            handle(
                &fixture.state,
                &Method::POST,
                "/api/admin/email/test",
                &json!({"template": "security"}),
                &admin
            )
            .await
            .unwrap()
            .status(),
            400
        );
        assert_eq!(
            handle(
                &fixture.state,
                &Method::GET,
                "/api/admin/email/test",
                &json!({}),
                &admin
            )
            .await
            .unwrap()
            .status(),
            404
        );
        assert_eq!(
            handle(
                &fixture.state,
                &Method::POST,
                "/api/admin/email/preview",
                &json!({"template":"unknown"}),
                &admin
            )
            .await
            .unwrap()
            .status(),
            400
        );
    }

    #[test]
    fn previews_use_real_templates_without_account_mutations() {
        let fixture = Fixture::new();
        for template in ["security", "signup", "login", "reset"] {
            let (subject, html) = test_message(&fixture.state, template).unwrap();
            assert!(subject.starts_with("[Test]"));
            assert!(html.contains("123456"));
            assert!(html.contains("Test email only."));
            assert!(!html.contains("This verification code is active"));
            assert!(!html.contains("unsubscribe from this list"));
        }
        assert_eq!(
            fixture
                .state
                .store
                .read_document(&fixture.dir.join("data/unsubscribe_tokens.json"), json!({})),
            json!({})
        );
        assert_eq!(
            fixture.state.store.read_document(
                &fixture.dir.join("data/pending_security_actions.json"),
                json!({})
            ),
            json!({})
        );
    }

    #[tokio::test]
    async fn delivery_requires_smtp_acceptance_and_forwards_target_recipient() {
        for (status, reply, expected) in [
            (StatusCode::OK, json!({"ok":true}), true),
            (StatusCode::OK, json!({"ok":false}), false),
            (StatusCode::OK, json!({}), false),
            (StatusCode::INTERNAL_SERVER_ERROR, json!({"ok":true}), false),
        ] {
            let app = Router::new().route(
                "/send",
                post(move |Json(body): Json<Value>| async move {
                    assert_eq!(body["to"], "recipient@example.test");
                    assert_eq!(body["sender"], "noreply");
                    assert_eq!(body["raw"], true);
                    assert_eq!(body["dry_run"], false);
                    assert_eq!(body["subject"], "[Test] subject");
                    assert_eq!(body["body"], "<p>Sample</p>");
                    assert!(body.get("cc").is_none());
                    assert!(body.get("bcc").is_none());
                    (status, Json(reply))
                }),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            assert_eq!(
                send_test(
                    &reqwest::Client::new(),
                    &base,
                    "recipient@example.test",
                    "[Test] subject",
                    "<p>Sample</p>"
                )
                .await
                .is_ok(),
                expected
            );
            server.abort();
        }
    }
}
