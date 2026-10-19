use super::{forbidden, AdminCtx, Resp};
use crate::{errors::json_resp, state::AppState};
use axum::http::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeSet, sync::Arc, time::Duration};

mod template;

#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct Draft {
    subject: String,
    preheader: String,
    edition: String,
    title: String,
    intro: String,
    hero_url: String,
    button_label: String,
    button_url: String,
    signature: String,
    accent: String,
    theme: String,
    audience: String,
    recipients: Vec<String>,
    sections: Vec<Section>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Section {
    title: String,
    body: String,
}

impl Default for Draft {
    fn default() -> Self {
        Self {
            subject: "mitch.pro updates — October 16–18".into(),
            preheader: "A weekend of updates to profiles, chat, movies, Blooket, privacy, and more. From Mitch & Tyler.".into(),
            edition: "October 16–18, 2026".into(),
            title: "This weekend on mitch.pro".into(),
            intro: "Hey everyone,\n\nWe’ve been updating the site throughout Friday, Saturday, and Sunday. Here’s what changed, from the pages you use every day to the tools running behind them.".into(),
            hero_url: String::new(),
            button_label: "Open mitch.pro".into(),
            button_url: "https://mitchdog.com".into(),
            signature: "Mitch & Tyler".into(),
            accent: "#2563eb".into(),
            theme: "paper".into(),
            audience: "selected".into(),
            recipients: vec![],
            sections: vec![
                Section { title: "Homepage and navigation".into(), body: "We refreshed the homepage, preferences, games, shop, coins, friends, members, casino, and school pages with simpler layouts and more direct labels. The site logo and MitchCoin artwork were updated too.\n\nMovies now opens the Rivestream homepage through the site’s proxy. Blooket Bot has its own full-width shortcut below the other four homepage cards and a link in the top navigation.".into() },
                Section { title: "Profiles and the leaderboard".into(), body: "The leaderboard was rebuilt around people’s actual profile photos and equipped cosmetics, including their profile colors, badges, and effects. Search and the layout were cleaned up, and Tylerium’s account display now keeps those same profile details.".into() },
                Section { title: "Sign-in and the new site tour".into(), body: "The login and public signup pages have simpler layouts and clearer account controls. Login styles also load correctly before you sign in.\n\nThe public introduction now centers on the new recording, edited to 3 minutes and 57 seconds with a louder soundtrack. The extra promotional copy and soundtrack caption were removed.".into() },
                Section { title: "Matrix chat and suggestions".into(), body: "Direct messages now find or reuse the right conversation. We fixed the disappearing Home space button and the empty bottom gap in the installed Matrix app.\n\nThere’s a public #suggestions channel, an administrator-only channel manager, and a moderation audit log. Unbanning someone no longer repeatedly invites or rejects them.".into() },
                Section { title: "People, friends, and privacy".into(), body: "Presence now follows individual tabs more accurately across People and Friends. Page and activity details are restricted to friends, yourself, and administrators.\n\nA Page Location toggle in Preferences lets you hide those details from everyone, including administrators. Privacy choices are enforced by the backend, and live presence stays in memory.".into() },
                Section { title: "Cloud Linux and administration".into(), body: "We repaired VM controls and desktop connections. Owners can adjust CPU, memory, and disk from VM Lab’s resource dialog, and staff account search now suggests names, usernames, and email addresses.\n\nThe admin panel’s collapsing layouts, passphrase wiring, and backend routes were fixed. Account, ban, economy, and VM tools received related fixes, and routine moderator visits are no longer added to the activity log. The Activity Log also has a fullscreen view.".into() },
                Section { title: "Tylerium Services and Blooket".into(), body: "Woodcreek’s main app is now Tylerium Services, with Tylerium branding and navigation. Its dashboard is available at /blooket-bot/ through Mitch, and both backends share Mitch accounts and sign-in.\n\nBlooket joins and retries use a shared queue with capacity checks, cancellation, and clearer connection errors. Existing game identities are preserved. Settings and game controls now use one controller, and a visible Start answering / Stop answering button controls answering. Server status controls and the remaining school-themed labels were removed; the browser tab uses Clever Portal branding.".into() },
                Section { title: "Email and site maintenance".into(), body: "Admin email tools now connect to the existing mitch-mail-rs service. Newsletters have their own editor with member selection, individual addresses, an everyone audience, saved drafts, and a preview before sending. Unsubscribed addresses are excluded.\n\nBuild-cache handling was repaired and capped at 12 GB. Rust formatting and CI checks were fixed, larger pull-request reviews handle their diffs correctly, and the push-notification workflow’s AI provider was updated. This email and its editor have also been redesigned, with this weekend’s changes loaded as the default update.\n\nThanks for using the site. If something needs fixing, let us know at support@mitch.pro.".into() },
            ],
        }
    }
}

fn now() -> i64 {
    mitch_lib::school::now_millis()
}

fn valid_email(value: &str) -> bool {
    value.len() <= 254
        && !value
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "*,;<>\"".contains(c))
        && value.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty()
                && domain.contains('.')
                && !domain.contains('@')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
        })
}

fn safe_url(value: &str) -> bool {
    value.is_empty()
        || (value.len() <= 2048
            && url::Url::parse(value).is_ok_and(|u| {
                matches!(u.scheme(), "https" | "http")
                    && u.host_str().is_some()
                    && u.username().is_empty()
                    && u.password().is_none()
            }))
}

fn parse_draft(body: &Value, ready: bool) -> Result<Draft, &'static str> {
    let draft: Draft =
        serde_json::from_value(body.clone()).map_err(|_| "Invalid newsletter fields.")?;
    if draft.subject.len() > 180
        || draft.subject.chars().any(char::is_control)
        || draft.preheader.len() > 250
        || draft.edition.len() > 80
        || draft.title.len() > 180
        || draft.intro.len() > 12_000
        || draft.signature.len() > 120
        || draft.button_label.len() > 80
        || draft.sections.len() > 8
        || draft
            .sections
            .iter()
            .any(|s| s.title.len() > 180 || s.body.len() > 12_000)
        || draft.recipients.len() > 5000
    {
        return Err(
            "Keep the subject and headings under 180 characters and use up to eight sections.",
        );
    }
    if !matches!(draft.audience.as_str(), "everyone" | "selected")
        || !matches!(draft.theme.as_str(), "midnight" | "paper")
    {
        return Err("Choose a valid audience and design.");
    }
    if draft.accent.len() != 7
        || !draft.accent.starts_with('#')
        || !draft.accent[1..].bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("Choose a valid accent color.");
    }
    if !safe_url(&draft.hero_url) || !safe_url(&draft.button_url) {
        return Err("Image and button links must be HTTP or HTTPS URLs.");
    }
    if ready
        && (draft.subject.trim().is_empty()
            || draft.title.trim().is_empty()
            || draft.intro.trim().is_empty())
    {
        return Err("Add a subject, title, and introduction before reviewing.");
    }
    Ok(draft)
}

fn unsubscribed(state: &AppState) -> BTreeSet<String> {
    mitch_lib::newsletter_unsub::newsletter_unsub_all_emails(&state.store)
        .into_iter()
        .map(|e| e.trim().to_lowercase())
        .collect()
}

fn directory(state: &AppState) -> Vec<Value> {
    let unsub = unsubscribed(state);
    let mut seen = BTreeSet::new();
    super::accounts::directory(state).into_iter().filter_map(|account| {
        let email = account["email"].as_str()?.trim().to_lowercase();
        if !valid_email(&email) || !seen.insert(email.clone()) { return None; }
        Some(json!({"email":email,"displayName":account["displayName"],"username":account["username"],"eligible":!unsub.contains(&email)}))
    }).collect()
}

fn resolve_recipients(state: &AppState, draft: &Draft) -> Result<Vec<String>, &'static str> {
    let unsub = unsubscribed(state);
    let candidates = if draft.audience == "everyone" {
        directory(state)
            .iter()
            .filter_map(|a| a["email"].as_str().map(String::from))
            .collect()
    } else {
        draft.recipients.clone()
    };
    let mut recipients = BTreeSet::new();
    for candidate in candidates {
        let email = candidate.trim().to_lowercase();
        if !valid_email(&email) {
            return Err("One of the recipient email addresses is invalid.");
        }
        if !unsub.contains(&email) {
            recipients.insert(email);
        }
    }
    if recipients.is_empty() {
        return Err("Choose at least one recipient who has not unsubscribed.");
    }
    Ok(recipients.into_iter().collect())
}

fn draft_path(state: &AppState, actor: &str) -> std::path::PathBuf {
    let key = mitch_lib::crypto::hmac_sha256(&state.id_secret, actor.as_bytes());
    state.data_dir().join(format!(
        "newsletters/draft-{}.json",
        key.iter().map(|b| format!("{b:02x}")).collect::<String>()
    ))
}

fn job_path(state: &AppState, id: &str) -> Option<std::path::PathBuf> {
    (id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| state.data_dir().join(format!("newsletters/job-{id}.json")))
}

fn job_summary(job: &Value) -> Value {
    let mut status = job["status"].as_str().unwrap_or("unknown");
    if status == "sending" && now() - job["updatedAt"].as_i64().unwrap_or(0) > 120_000 {
        status = "interrupted";
    }
    json!({"id":job["id"],"status":status,"total":job["recipients"].as_array().map_or(0,Vec::len),
        "accepted":job["accepted"],"failed":job["failed"],"uncertain":job["uncertain"],"skipped":job["skipped"],"updatedAt":job["updatedAt"]})
}

pub async fn handle(
    state: &Arc<AppState>,
    method: &Method,
    path: &str,
    search: &str,
    body: &Value,
    ctx: &AdminCtx,
) -> Resp {
    if !path.starts_with("/api/admin/newsletters/") {
        return None;
    }
    if !ctx.is_admin(state) {
        return Some(forbidden());
    }
    let actor = ctx.email(state);
    let response = match (method, path) {
        (&Method::GET, "/api/admin/newsletters/recipients") => {
            let accounts = directory(state);
            json_resp(
                200,
                json!({"ok":true,"accounts":accounts,"eligible":accounts.iter().filter(|a| a["eligible"] == true).count()}),
            )
        }
        (&Method::GET, "/api/admin/newsletters/draft") => {
            let saved = state
                .store
                .read_document(&draft_path(state, &actor), Value::Null);
            json_resp(
                200,
                json!({"ok":true,"draft":saved.get("draft").cloned().unwrap_or_else(|| json!(Draft::default())),"example":Draft::default(),"savedAt":saved["savedAt"]}),
            )
        }
        (
            &Method::POST,
            "/api/admin/newsletters/draft"
            | "/api/admin/newsletters/preview"
            | "/api/admin/newsletters/review",
        ) => {
            let ready = path.ends_with("/review");
            let draft = match parse_draft(body, ready) {
                Ok(draft) => draft,
                Err(error) => return Some(json_resp(400, json!({"error":error}))),
            };
            if path.ends_with("/preview") {
                json_resp(
                    200,
                    json!({"ok":true,"html":template::render(&draft, "#"),"subject":draft.subject,"preheader":draft.preheader}),
                )
            } else if path.ends_with("/draft") {
                let saved = json!({"draft":draft,"savedAt":now()});
                match state
                    .store
                    .write_document(&draft_path(state, &actor), &saved)
                {
                    Ok(()) => json_resp(200, json!({"ok":true,"savedAt":saved["savedAt"]})),
                    Err(_) => json_resp(500, json!({"error":"The draft could not be saved."})),
                }
            } else {
                let recipients = match resolve_recipients(state, &draft) {
                    Ok(recipients) => recipients,
                    Err(error) => return Some(json_resp(400, json!({"error":error}))),
                };
                let id = mitch_lib::crypto::random_bytes_hex(16);
                let job = json!({"id":id,"actor":actor,"draft":draft,"recipients":recipients,"status":"review","createdAt":now(),"updatedAt":now(),"accepted":0,"failed":0,"uncertain":0,"skipped":0});
                let Some(file) = job_path(state, &id) else {
                    return Some(json_resp(500, json!({"error":"Could not prepare review."})));
                };
                match state.store.write_document(&file, &job) {
                    Ok(()) => json_resp(
                        200,
                        json!({"ok":true,"id":id,"recipients":job["recipients"],"count":job["recipients"].as_array().map_or(0,Vec::len),"subject":draft.subject,"html":template::render(&draft,"#")}),
                    ),
                    Err(_) => json_resp(
                        500,
                        json!({"error":"The newsletter could not be prepared."}),
                    ),
                }
            }
        }
        (&Method::GET, "/api/admin/newsletters/job")
        | (&Method::POST, "/api/admin/newsletters/send") => {
            let query = crate::handler::query(search);
            let id = if *method == Method::GET {
                query.get("id").map(String::as_str).unwrap_or("")
            } else {
                body["id"].as_str().unwrap_or("")
            };
            let Some(file) = job_path(state, id) else {
                return Some(json_resp(404, json!({"error":"Newsletter not found."})));
            };
            let mut job = state.store.read_document(&file, Value::Null);
            if job["actor"].as_str() != Some(&actor) {
                return Some(json_resp(404, json!({"error":"Newsletter not found."})));
            }
            if *method == Method::GET {
                return Some(json_resp(200, json!({"ok":true,"job":job_summary(&job)})));
            }
            if body["confirm"] != true {
                return Some(json_resp(
                    400,
                    json!({"error":"Review and confirm the newsletter before sending."}),
                ));
            }
            if job["status"] != "review" {
                return Some(json_resp(200, json!({"ok":true,"job":job_summary(&job)})));
            }
            if now() - job["createdAt"].as_i64().unwrap_or(0) > 900_000 {
                return Some(json_resp(
                    409,
                    json!({"error":"This review expired. Review the newsletter again."}),
                ));
            }
            let expected = job.clone();
            job["status"] = json!("sending");
            job["updatedAt"] = json!(now());
            match state
                .store
                .compare_exchange_document(&file, &expected, &job)
            {
                Ok(true) => {
                    mitch_lib::admin::log_admin_action(
                        &state.store,
                        &state.cfg.data_dir,
                        &actor,
                        "newsletter_send",
                        json!({"id":id,"subject":job["draft"]["subject"],"count":job["recipients"].as_array().map_or(0,Vec::len)}),
                    );
                    let base = crate::routes::push::mail_service_url();
                    let state = state.clone();
                    let work = job.clone();
                    tokio::spawn(async move {
                        deliver(state, file, work, base).await;
                    });
                    json_resp(202, json!({"ok":true,"job":job_summary(&job)}))
                }
                Ok(false) => json_resp(
                    409,
                    json!({"error":"This newsletter was already submitted. Check its delivery status."}),
                ),
                Err(_) => json_resp(
                    500,
                    json!({"error":"The newsletter could not be queued. No emails were started."}),
                ),
            }
        }
        _ => json_resp(405, json!({"error":"Method not allowed."})),
    };
    Some(response)
}

async fn deliver(state: Arc<AppState>, file: std::path::PathBuf, mut job: Value, base: String) {
    let Ok(draft) = serde_json::from_value::<Draft>(job["draft"].clone()) else {
        return;
    };
    let recipients: Vec<String> = job["recipients"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();
    let client = reqwest::Client::new();
    let registered: BTreeSet<String> = directory(&state)
        .iter()
        .filter_map(|a| a["email"].as_str().map(String::from))
        .collect();
    for email in recipients {
        let outcome = if unsubscribed(&state).contains(&email)
            || (draft.audience == "everyone" && !registered.contains(&email))
        {
            "skipped"
        } else {
            let unsubscribe = super::legacy::unsubscribe_url(&state, &email);
            let sender = if email.ends_with("@student.rjuhsd.us") {
                "gmail"
            } else {
                "noreply"
            };
            match client.post(format!("{base}/send")).timeout(Duration::from_secs(30))
                .json(&json!({"sender":sender,"to":email,"subject":draft.subject,"body":template::render(&draft,&unsubscribe),"raw":true,"dry_run":false})).send().await {
                Ok(response) => {
                    let status = response.status();
                    match response.json::<Value>().await {
                        Ok(result) if status.is_success() && result["ok"] == true => "accepted",
                        Ok(result) if result["ok"] == false => "failed",
                        _ => "uncertain",
                    }
                }
                Err(_) => "uncertain",
            }
        };
        job[outcome] = json!(job[outcome].as_u64().unwrap_or(0) + 1);
        job["updatedAt"] = json!(now());
        if state.store.write_document(&file, &job).is_err() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    job["status"] = json!("complete");
    job["updatedAt"] = json!(now());
    let _ = state.store.write_document(&file, &job);
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{http::StatusCode, routing::post, Json, Router};
    use mitch_lib::{auth, data::DataStore, newsletter_unsub, passwords, profiles};

    struct Fixture {
        state: Arc<AppState>,
        dir: std::path::PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "newsletter-test-{}",
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
                    &json!({"owners":["admin@example.test"]}),
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
    async fn body(response: axum::response::Response) -> Value {
        serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 1 << 20)
                .await
                .unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn audience_uses_registered_accounts_deduplicates_and_respects_unsubscribes() {
        let fixture = Fixture::new();
        let state = &fixture.state;
        passwords::password_set(
            &state.store,
            "password@example.test",
            &mitch_lib::crypto::random_bytes_hex(32),
        );
        profiles::profile_set(
            &state.store,
            "profile@example.test",
            &json!({"username":"profile"}),
        );
        profiles::profile_set(
            &state.store,
            "unsub@example.test",
            &json!({"username":"unsub"}),
        );
        profiles::profile_set(&state.store, "ad***n@example.test", &json!({}));
        newsletter_unsub::newsletter_unsub_add(&state.store, "unsub@example.test");
        let mut draft = Draft {
            audience: "everyone".into(),
            ..Draft::default()
        };
        assert_eq!(
            resolve_recipients(state, &draft).unwrap(),
            vec!["password@example.test", "profile@example.test"]
        );
        draft.audience = "selected".into();
        draft.recipients = vec![
            " PERSON@example.test ".into(),
            "person@example.test".into(),
            "unsub@example.test".into(),
        ];
        assert_eq!(
            resolve_recipients(state, &draft).unwrap(),
            vec!["person@example.test"]
        );
        draft
            .recipients
            .push("a@example.test,b@example.test".into());
        assert!(resolve_recipients(state, &draft).is_err());
    }

    #[test]
    fn content_is_escaped_and_unsafe_links_and_headers_are_rejected() {
        let mut draft = Draft {
            title: "<script>alert(1)</script>".into(),
            ..Draft::default()
        };
        let html = template::render(&draft, "https://mitchdog.com/unsubscribe/token");
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>"));
        assert!(html.contains("https://mitchdog.com/unsubscribe/token"));
        draft.edition = "<img src=x onerror=alert(1)>".into();
        let html = template::render(&draft, "#");
        assert!(html.contains("&lt;img src=x onerror=alert(1)&gt;"));
        assert!(html.contains("Mitch &amp; Tyler"));
        draft.edition = "x".repeat(81);
        assert!(parse_draft(&json!(draft), false).is_err());
        draft.edition.clear();
        draft.hero_url = "javascript:alert(1)".into();
        assert!(parse_draft(&json!(draft), false).is_err());
        draft.hero_url.clear();
        draft.subject = "Hello\r\nBcc: other@example.test".into();
        assert!(parse_draft(&json!(draft), false).is_err());
        draft.subject.clear();
        assert!(parse_draft(&json!(draft), false).is_ok());
        assert!(parse_draft(&json!(draft), true).is_err());
    }

    #[tokio::test]
    async fn previews_and_reviews_never_send_and_require_full_admin_access() {
        let fixture = Fixture::new();
        let state = &fixture.state;
        let member = fixture.ctx("member@example.test");
        let admin = fixture.ctx("admin@example.test");
        let draft = Draft {
            recipients: vec!["only@example.test".into()],
            ..Draft::default()
        };
        for path in ["draft", "preview", "review", "send", "recipients", "job"] {
            assert_eq!(
                handle(
                    state,
                    &Method::POST,
                    &format!("/api/admin/newsletters/{path}"),
                    "",
                    &json!(draft),
                    &member
                )
                .await
                .unwrap()
                .status(),
                403
            );
        }
        let preview = body(
            handle(
                state,
                &Method::POST,
                "/api/admin/newsletters/preview",
                "",
                &json!(draft),
                &admin,
            )
            .await
            .unwrap(),
        )
        .await;
        assert!(preview["html"].as_str().unwrap().contains("mitch.pro"));
        let reviewed = body(
            handle(
                state,
                &Method::POST,
                "/api/admin/newsletters/review",
                "",
                &json!(draft),
                &admin,
            )
            .await
            .unwrap(),
        )
        .await;
        assert_eq!(reviewed["count"], 1);
        let file = job_path(state, reviewed["id"].as_str().unwrap()).unwrap();
        let original = state.store.read_document(&file, Value::Null);
        assert_eq!(original["status"], "review");
        assert_eq!(original["accepted"], 0);
        assert_eq!(
            state
                .store
                .read_document(&state.data_dir().join("unsubscribe_tokens.json"), json!({})),
            json!({})
        );
        assert_eq!(
            handle(
                state,
                &Method::POST,
                "/api/admin/newsletters/send",
                "",
                &json!({"id":reviewed["id"]}),
                &admin
            )
            .await
            .unwrap()
            .status(),
            400
        );
        let mut expired = original.clone();
        expired["createdAt"] = json!(now() - 901_000);
        state.store.write_document(&file, &expired).unwrap();
        assert_eq!(
            handle(
                state,
                &Method::POST,
                "/api/admin/newsletters/send",
                "",
                &json!({"id":reviewed["id"],"confirm":true}),
                &admin
            )
            .await
            .unwrap()
            .status(),
            409
        );
        let mut claimed = original.clone();
        claimed["status"] = json!("sending");
        state.store.write_document(&file, &original).unwrap();
        assert!(state
            .store
            .compare_exchange_document(&file, &original, &claimed)
            .unwrap());
        let second_store = DataStore::open(&fixture.dir, &state.cfg.data_dir).unwrap();
        assert!(!second_store
            .compare_exchange_document(&file, &original, &claimed)
            .unwrap());
        let repeated = body(
            handle(
                state,
                &Method::POST,
                "/api/admin/newsletters/send",
                "",
                &json!({"id":reviewed["id"],"confirm":true}),
                &admin,
            )
            .await
            .unwrap(),
        )
        .await;
        assert_eq!(repeated["job"]["status"], "sending");
    }

    #[tokio::test]
    async fn worker_reports_acceptance_failure_and_uncertainty_without_retries() {
        let fixture = Fixture::new();
        let state = &fixture.state;
        let captured = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
        let output = captured.clone();
        let app = Router::new().route(
            "/send",
            post(move |Json(payload): Json<Value>| {
                let output = output.clone();
                async move {
                    let email = payload["to"].as_str().unwrap().to_string();
                    output.lock().unwrap().push(payload);
                    match email.as_str() {
                        "failed@example.test" => {
                            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"ok":false})))
                        }
                        "uncertain@example.test" => (StatusCode::OK, Json(json!({}))),
                        _ => (StatusCode::OK, Json(json!({"ok":true}))),
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        newsletter_unsub::newsletter_unsub_add(&state.store, "unsub@example.test");
        let job = json!({"id":"abc","status":"sending","draft":Draft::default(),"recipients":["ok@example.test","failed@example.test","uncertain@example.test","student@student.rjuhsd.us","unsub@example.test"],"accepted":0,"failed":0,"uncertain":0,"skipped":0});
        let file = state.data_dir().join("newsletters/test-job.json");
        deliver(state.clone(), file.clone(), job, base).await;
        let result = state.store.read_document(&file, Value::Null);
        assert_eq!(result["status"], "complete");
        assert_eq!(result["accepted"], 2);
        assert_eq!(result["failed"], 1);
        assert_eq!(result["uncertain"], 1);
        assert_eq!(result["skipped"], 1);
        let messages = captured.lock().unwrap();
        assert_eq!(messages.len(), 4);
        for message in messages.iter() {
            assert!(message.get("cc").is_none() && message.get("bcc").is_none());
            assert!(message["body"].as_str().unwrap().contains("/unsubscribe/"));
            assert_eq!(message["raw"], true);
            if message["to"] == "student@student.rjuhsd.us" {
                assert_eq!(message["sender"], "gmail");
            }
        }
        server.abort();
    }
}
