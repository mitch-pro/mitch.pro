//! Send operations, replacing `mail/send_email.js`, `mail/noreply_send.js`,
//! and `mail/support_send.js` (three near-duplicate nodemailer CLIs).
//!
//! Per-sender behavior is ported exactly:
//! - gmail  → GMAIL_USER/GMAIL_PASS via smtp.gmail.com:465 (nodemailer
//!   `service: 'gmail'`), `priority: 'high'` (which nodemailer turns into
//!   `X-Priority: 1 (Highest)`, `X-MSMail-Priority: High`, `Importance: High`
//!   — its setPriorityHeaders() overwrites the user-supplied headers, see
//!   node_modules/nodemailer/lib/mailer/index.js:191). `-a` switches to
//!   GMAIL_*_ALT and appends 1-8 zero-width spaces to the subject.
//! - noreply → NOREPLY_USER/PASS via MAIL_SMTP_HOST:465, AUTH PLAIN, invalid
//!   TLS certs accepted (`rejectUnauthorized: false`), no priority headers.
//! - support → SUPPORT_USER/PASS via MAIL_SMTP_HOST:465, default TLS
//!   verification, `--raw` supported.
//!
//! Header contract (verified against nodemailer 8.x): List-Unsubscribe /
//! List-Unsubscribe-Post on all senders (token hardcoded to mitch.pro host);
//! In-Reply-To + References when `in_reply_to` is set (except gmail -a).

use crate::config::{env_trim, MailConfig};
use crate::template::{format_html_email, Sender};
use mitch_lib::data::DataStore;

#[derive(serde::Deserialize)]
pub struct SendRequest {
    /// One of "gmail" | "noreply" | "support".
    pub sender: String,
    pub to: String,
    pub subject: String,
    /// Raw text body (argv body or piped stdin in the CLI scripts).
    pub body: String,
    /// Already angle-wrapped `<msgid>`, passed through as raw header values.
    /// The JS shims send camelCase (`inReplyTo`).
    #[serde(default, rename = "inReplyTo")]
    pub in_reply_to: Option<String>,
    /// `-a` flag (gmail only).
    #[serde(default)]
    pub alt: bool,
    /// `--raw` flag (gmail + support): no unsubscribe suffix, plain footer.
    #[serde(default)]
    pub raw: bool,
    /// Build everything but skip SMTP delivery — used for parity testing.
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(serde::Serialize)]
pub struct SendResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Echoed artifacts so tests can diff the rendered bodies/headers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dry_run: Option<DryRunArtifact>,
}

#[derive(serde::Serialize)]
pub struct DryRunArtifact {
    pub text_body: String,
    pub html_body: String,
    pub subject: String,
    pub from: String,
    pub headers: Vec<(String, String)>,
}

pub struct Prepared {
    pub to: String,
    pub subject: String,
    pub from: String,
    pub text_body: String,
    pub html_body: String,
    /// (name, value) pairs emitted as raw headers — exact JS header strings.
    pub headers: Vec<(String, String)>,
    /// SMTP host/port/auth/TLS mode resolved from env.
    pub smtp: SmtpParams,
}

pub struct SmtpParams {
    pub host: String,
    /// 465 like the JS scripts (`port: 465, secure: true`); MAIL_SMTP_PORT
    /// overrides it for local SMTP-sink testing.
    pub port: u16,
    pub user: String,
    pub pass: String,
    /// noreply sets `tls: { rejectUnauthorized: false }`.
    pub insecure_tls: bool,
}

fn strip_trailing_slash(s: &str) -> &str {
    s.strip_suffix('/').unwrap_or(s)
}

/// `data/site.json` read with plain fs (it is a PRESERVED_DATA_FILE), with the
/// per-sender fallback defaults from the scripts. Returns (PRIMARY, ALT).
fn site_urls(cfg: &MailConfig, sender: Sender) -> (String, String) {
    let fallback_alt = match sender {
        Sender::Gmail => "https://mitchdog.com",
        Sender::Noreply | Sender::Support => "https://mitch.88chan.me",
    };
    let mut primary = "https://mitch.pro".to_string();
    let mut alt = fallback_alt.to_string();
    if let Ok(raw) = std::fs::read_to_string(cfg.data_dir.join("site.json")) {
        if let Ok(site) = serde_json::from_str::<serde_json::Value>(&raw) {
            if let Some(p) = site.get("primary").and_then(|v| v.as_str()) {
                primary = strip_trailing_slash(p).to_string();
            }
            if let Some(a) = site.get("alternate").and_then(|v| v.as_str()) {
                alt = strip_trailing_slash(a).to_string();
            }
        }
    }
    (primary, alt)
}

/// Reads or creates the recipient's unsubscribe token through the data store
/// (`data/unsubscribe_tokens.json` → DB `json_documents`), exactly like the
/// scripts' `readDocument`/`writeDocument` flow.
fn unsubscribe_token(
    store: &DataStore,
    cfg: &MailConfig,
    recipient: &str,
) -> Result<String, String> {
    use rand::RngCore;
    let file = cfg.data_dir.join("unsubscribe_tokens.json");
    let mut tokens = store.read_document(&file, serde_json::json!({}));
    if !tokens.is_object() {
        tokens = serde_json::json!({});
    }
    let obj = tokens.as_object_mut().ok_or("tokens not an object")?;
    let key = recipient.to_lowercase();
    if let Some(existing) = obj.get(&key).and_then(|v| v.as_str()) {
        return Ok(existing.to_string());
    }
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    obj.insert(key, serde_json::Value::String(token.clone()));
    store
        .write_document(&file, &serde_json::Value::Object(obj.clone()))
        .map_err(|e| format!("failed to write unsubscribe token: {e}"))?;
    Ok(token)
}

/// Resolves credentials and SMTP transport per sender. Mirrors each script's
/// env handling and hard exit conditions.
fn smtp_params(sender: Sender, alt: bool) -> Result<SmtpParams, String> {
    let (user_key, pass_key) = match (sender, alt) {
        (Sender::Gmail, false) => ("GMAIL_USER", "GMAIL_PASS"),
        (Sender::Gmail, true) => ("GMAIL_USER_ALT", "GMAIL_PASS_ALT"),
        (Sender::Noreply, _) => ("NOREPLY_USER", "NOREPLY_PASS"),
        (Sender::Support, _) => ("SUPPORT_USER", "SUPPORT_PASS"),
    };
    let user = env_trim(user_key);
    let pass = env_trim(pass_key);
    if user.is_empty() || pass.is_empty() {
        return Err(match sender {
            Sender::Gmail => format!("Set {user_key} and {pass_key} env vars in Doppler/.env"),
            other => format!(
                "Set {} and {} in environment/dotenv/doppler",
                match other {
                    Sender::Noreply => "NOREPLY_USER",
                    Sender::Support => "SUPPORT_USER",
                    Sender::Gmail => unreachable!(),
                },
                match other {
                    Sender::Noreply => "NOREPLY_PASS",
                    Sender::Support => "SUPPORT_PASS",
                    Sender::Gmail => unreachable!(),
                }
            ),
        });
    }
    let (host, insecure_tls) = match sender {
        Sender::Gmail => ("smtp.gmail.com".to_string(), false),
        _ => {
            let h = env_trim("MAIL_SMTP_HOST");
            (
                if h.is_empty() {
                    "mail.mitch.pro".to_string()
                } else {
                    h
                },
                sender == Sender::Noreply,
            )
        }
    };
    let port = std::env::var("MAIL_SMTP_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(465);
    Ok(SmtpParams {
        host,
        port,
        user,
        pass,
        insecure_tls,
    })
}

/// Zero-width-space subject jitter for gmail `-a` (1-8 chars, like the JS).
fn zwsp_run() -> String {
    use rand::Rng;
    let n = rand::rng().random_range(1..=8);
    "\u{200b}".repeat(n)
}

/// Builds everything except the SMTP transaction: token, text suffix, HTML,
/// subject, from-line, raw headers. This is the deterministic half — fully
/// testable without network.
pub fn prepare(store: &DataStore, cfg: &MailConfig, req: &SendRequest) -> Result<Prepared, String> {
    let sender = match req.sender.as_str() {
        "gmail" => Sender::Gmail,
        "noreply" => Sender::Noreply,
        "support" => Sender::Support,
        other => return Err(format!("unknown sender: {other}")),
    };
    if req.alt && sender != Sender::Gmail {
        return Err("-a is only valid for the gmail sender".to_string());
    }
    let (primary, alt_url) = site_urls(cfg, sender);
    let token = unsubscribe_token(store, cfg, &req.to)?;
    let smtp = smtp_params(sender, req.alt)?;

    // JS parity: gmail skips the suffix for BOTH --raw and -a
    // (`(useAlt || useRaw) ? rawBody : rawBody + suffix`); the other senders
    // only for --raw.
    let skip_suffix = req.raw || (sender == Sender::Gmail && req.alt);
    let text_body = if skip_suffix {
        req.body.clone()
    } else {
        let support_note = match sender {
            Sender::Gmail => "For support: email SUPPORT to support@mitch.pro or mitchell.fogler@student.rjuhsd.us\n",
            Sender::Support => "For support: support@mitch.pro or mitchell.fogler@student.rjuhsd.us\n",
            Sender::Noreply => "",
        };
        format!(
            "{}\n\n---\nVisit {primary}/unsubscribe/{token} to unsubscribe.\nAlso available at {alt_url}/unsubscribe/{token}\n{}2014 Capitol Ave #100, Sacramento, CA 95811",
            req.body, support_note
        )
    };

    let unsub_url = if req.raw {
        None
    } else {
        Some(format!("{primary}/unsubscribe/{token}"))
    };
    let html_body = format_html_email(
        sender,
        &req.subject,
        &req.body,
        unsub_url.as_deref(),
        &primary,
        &alt_url,
    );

    let (from, subject) = match sender {
        Sender::Gmail => {
            let user = env_trim(if req.alt {
                "GMAIL_USER_ALT"
            } else {
                "GMAIL_USER"
            });
            let name = if req.alt {
                user.clone()
            } else {
                "mitch.pro".to_string()
            };
            let subject = if req.alt {
                format!("{}{}", req.subject, zwsp_run())
            } else {
                req.subject.clone()
            };
            (format!("{name} <{user}>"), subject)
        }
        Sender::Noreply => (
            "mitch.pro <noreply@mitch.pro>".to_string(),
            req.subject.clone(),
        ),
        Sender::Support => (
            "mitch.pro Support <support@mitch.pro>".to_string(),
            req.subject.clone(),
        ),
    };

    let mut headers = Vec::new();
    if sender == Sender::Gmail {
        headers.push(("X-Priority".to_string(), "1 (Highest)".to_string()));
        headers.push(("X-MSMail-Priority".to_string(), "High".to_string()));
        headers.push(("Importance".to_string(), "High".to_string()));
    }
    headers.push((
        "List-Unsubscribe".to_string(),
        format!("<https://mitch.pro/unsubscribe/{token}>, <mailto:support@mitch.pro?subject=unsubscribe>"),
    ));
    headers.push((
        "List-Unsubscribe-Post".to_string(),
        "List-Unsubscribe=One-Click".to_string(),
    ));
    let reply_headers_apply = match sender {
        Sender::Gmail => req.in_reply_to.is_some() && !req.alt,
        _ => req.in_reply_to.is_some(),
    };
    if reply_headers_apply {
        let id = req.in_reply_to.clone().unwrap_or_default();
        headers.push(("In-Reply-To".to_string(), id.clone()));
        headers.push(("References".to_string(), id));
    }

    Ok(Prepared {
        to: req.to.clone(),
        subject,
        from,
        text_body,
        html_body,
        headers,
        smtp,
    })
}

fn build_lettre_message(p: &Prepared) -> Result<lettre::message::Message, String> {
    use lettre::message::{header::HeaderName, header::HeaderValue, Mailbox, Message, MultiPart};

    let from: Mailbox = p.from.parse().map_err(|e| format!("bad from: {e}"))?;
    let to: Mailbox = p.to.parse().map_err(|e| format!("bad to: {e}"))?;

    let mut builder = Message::builder()
        .message_id(None) // nodemailer always ensures a Message-ID
        .from(from)
        .to(to)
        .subject(&p.subject);

    for (name, value) in &p.headers {
        // Values are ASCII printable header strings (ids wrapped in <>, urls,
        // one-click directives) — no folding or RFC2047 needed.
        debug_assert!(value.is_ascii() && !value.contains('\r') && !value.contains('\n'));
        builder = builder.raw_header(HeaderValue::dangerous_new_pre_encoded(
            HeaderName::new_from_ascii(name.to_string())
                .map_err(|_| format!("invalid header name: {name}"))?,
            value.clone(),
            value.clone(),
        ));
    }

    builder
        .multipart(MultiPart::alternative_plain_html(
            p.text_body.clone(),
            p.html_body.clone(),
        ))
        .map_err(|e| format!("multipart build: {e}"))
}

/// Sends via lettre. Gmail: verified TLS. Noreply: invalid certs accepted
/// (parity with `tls: { rejectUnauthorized: false }`).
pub fn deliver(p: &Prepared) -> Result<(), String> {
    use lettre::{
        transport::smtp::authentication::Credentials, transport::smtp::client::Tls,
        transport::smtp::client::TlsParameters, SmtpTransport, Transport,
    };

    let email = build_lettre_message(p).map_err(|e| e.to_string())?;

    let tls_parameters = TlsParameters::builder(p.smtp.host.clone())
        .dangerous_accept_invalid_certs(p.smtp.insecure_tls)
        .build()
        .map_err(|e| format!("tls params: {e}"))?;

    let transport = SmtpTransport::relay(&p.smtp.host)
        .map_err(|e| format!("relay: {e}"))?
        .port(p.smtp.port)
        .tls(Tls::Wrapper(tls_parameters))
        .credentials(Credentials::new(p.smtp.user.clone(), p.smtp.pass.clone()))
        .build();

    transport
        .send(&email)
        .map_err(|e| format!("smtp send failed: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_cfg() -> (MailConfig, DataStore) {
        let base = std::env::temp_dir().join(format!(
            "mitch-mail-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(base.join("data")).unwrap();
        let cfg = MailConfig {
            base_dir: base.clone(),
            data_dir: base.join("data"),
            port: 6902,
            imap_host: "mail.mitch.pro".into(),
            ntfy_topic: String::new(),
            ntfy_user: String::new(),
            ntfy_pass: String::new(),
        };
        let store = DataStore::open(&base, &base.join("data")).unwrap();
        (cfg, store)
    }

    fn req(sender: &str) -> SendRequest {
        SendRequest {
            sender: sender.into(),
            to: "user@example.com".into(),
            subject: "Test".into(),
            body: "Line one\n\nLine two".into(),
            in_reply_to: None,
            alt: false,
            raw: false,
            dry_run: true,
        }
    }

    /// One serial test: all scenarios mutate process env (like the scripts),
    /// which is process-global and would race under parallel tests.
    #[test]
    fn prepare_matches_sender_contracts() {
        std::env::remove_var("MAIL_SMTP_HOST");

        // ── noreply ─────────────────────────────────────────────────────
        std::env::set_var("NOREPLY_USER", "noreply@mitch.pro");
        std::env::set_var("NOREPLY_PASS", "secret");
        let (cfg, store) = test_cfg();
        let p = prepare(&store, &cfg, &req("noreply")).unwrap();
        assert!(p
            .text_body
            .contains("---\nVisit https://mitch.pro/unsubscribe/"));
        assert!(p.text_body.contains("2014 Capitol Ave #100"));
        assert!(!p.text_body.contains("For support"));
        assert_eq!(p.from, "mitch.pro <noreply@mitch.pro>");
        assert!(
            !p.headers.iter().any(|(n, _)| n == "X-Priority"),
            "noreply has no priority headers"
        );
        let token = store
            .read_document(&cfg.data_dir.join("unsubscribe_tokens.json"), json!({}))
            .get("user@example.com")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        let lu = p
            .headers
            .iter()
            .find(|(n, _)| n == "List-Unsubscribe")
            .unwrap();
        assert_eq!(
            lu.1,
            format!("<https://mitch.pro/unsubscribe/{token}>, <mailto:support@mitch.pro?subject=unsubscribe>")
        );
        drop(store);
        std::fs::remove_dir_all(&cfg.base_dir).ok();

        // ── gmail ───────────────────────────────────────────────────────
        std::env::set_var("GMAIL_USER", "test@gmail.com");
        std::env::set_var("GMAIL_PASS", "secret");
        let (cfg, store) = test_cfg();
        let p = prepare(&store, &cfg, &req("gmail")).unwrap();
        assert_eq!(
            p.headers.iter().find(|(n, _)| n == "X-Priority").unwrap().1,
            "1 (Highest)"
        );
        assert!(p.headers.iter().any(|(n, _)| n == "X-MSMail-Priority"));
        assert!(p.text_body.contains(
            "For support: email SUPPORT to support@mitch.pro or mitchell.fogler@student.rjuhsd.us"
        ));
        assert_eq!(p.from, "mitch.pro <test@gmail.com>");
        drop(store);
        std::fs::remove_dir_all(&cfg.base_dir).ok();

        // ── support --raw ───────────────────────────────────────────────
        std::env::set_var("SUPPORT_USER", "support@mitch.pro");
        std::env::set_var("SUPPORT_PASS", "secret");
        let (cfg, store) = test_cfg();
        let mut r = req("support");
        r.raw = true;
        let p = prepare(&store, &cfg, &r).unwrap();
        assert_eq!(p.text_body, r.body);
        assert!(!p.html_body.contains("unsubscribe from this list"));
        assert!(
            p.html_body.contains("or mitchell.fogler@student.rjuhsd.us"),
            "support footer keeps the student line even raw"
        );
        // But the token is still minted (JS writes the token regardless).
        assert!(store
            .read_document(&cfg.data_dir.join("unsubscribe_tokens.json"), json!({}))
            .get("user@example.com")
            .is_some());
        drop(store);
        std::fs::remove_dir_all(&cfg.base_dir).ok();

        // ── gmail -a ────────────────────────────────────────────────────
        std::env::set_var("GMAIL_USER_ALT", "alt@gmail.com");
        std::env::set_var("GMAIL_PASS_ALT", "secret");
        std::env::remove_var("GMAIL_USER");
        std::env::remove_var("GMAIL_PASS");
        let (cfg, store) = test_cfg();
        let mut r = req("gmail");
        r.alt = true;
        let p = prepare(&store, &cfg, &r).unwrap();
        assert_eq!(p.from, "alt@gmail.com <alt@gmail.com>");
        assert!(p.subject.starts_with("Test"));
        assert!(p
            .subject
            .chars()
            .skip("Test".len())
            .all(|c| c == '\u{200b}'));
        assert!(
            !p.headers.iter().any(|(n, _)| n == "In-Reply-To"),
            "-a suppresses reply headers"
        );
        assert!(
            !p.text_body.contains("---"),
            "alt skips the unsubscribe suffix"
        );
        assert!(
            p.html_body.contains("unsubscribe from this list"),
            "alt keeps the HTML unsubscribe footer"
        );
        drop(store);
        std::fs::remove_dir_all(&cfg.base_dir).ok();
    }
}
