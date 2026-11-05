//! Shared harness for the integration tests in `tests/`.
//!
//! Each test starts the real `mitch-server` binary against a throwaway data
//! directory (never production data), seeds fixture accounts straight into its
//! SQLite database, and talks to it over HTTP. This replaces the old Node/Playwright
//! scripts for everything that is a server contract rather than browser layout.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use mitch_lib::auth::make_email_id;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// A running server plus its temp directory. Dropping it kills the process and
/// removes the directory.
pub struct TestServer {
    child: Child,
    pub dir: PathBuf,
    pub url: String,
    secret: Vec<u8>,
    client: reqwest::Client,
}

pub struct Reply {
    pub status: u16,
    pub json: serde_json::Value,
    pub text: String,
}

impl TestServer {
    /// Start a server. `data_files` are written under `data/` before launch
    /// (e.g. `("testers.json", "[...]")`). `ready_path` is polled until it
    /// answers with `ready_status`, which shows the router is serving.
    pub async fn start(data_files: &[(&str, &str)], ready_path: &str, ready_status: u16) -> Self {
        // Tests in one binary run in parallel and the clock can be coarse, so
        // pid + time alone collided (the second server's symlink then failed
        // with "File exists"); a per-process counter keeps every dir unique.
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "mitch-it-{}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("data")).unwrap();
        let webserver = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../webserver");
        #[cfg(unix)]
        std::os::unix::fs::symlink(webserver.canonicalize().unwrap(), dir.join("webserver"))
            .unwrap();
        for (name, content) in data_files {
            std::fs::write(dir.join("data").join(name), content).unwrap();
        }

        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let child = Command::new(env!("CARGO_BIN_EXE_mitch-server"))
            .current_dir(&dir)
            .env("DATA_DIR", dir.join("data"))
            .env("MITCH_BASE", &dir)
            .env("USERDATA_DIR", dir.join("userdata"))
            .env("NODE_ENV", "test")
            .env("GOD_MODE", "0")
            .env("PORT", port.to_string())
            .env("VAPID_PUBLIC_KEY", "")
            .env("RECAPTCHA_SITE_KEY", "")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("start mitch-server");

        let url = format!("http://127.0.0.1:{port}");
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        let mut ready = false;
        for _ in 0..300 {
            if let Ok(r) = client.get(format!("{url}{ready_path}")).send().await {
                if r.status().as_u16() == ready_status {
                    ready = true;
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(ready, "mitch-server did not become ready at {ready_path}");
        let secret = std::fs::read(dir.join("data/id_secret.key")).expect("id_secret.key");
        TestServer {
            child,
            dir,
            url,
            secret,
            client,
        }
    }

    /// Create a signed-in session for `email` and return the session id used in
    /// the `studentId`/`id` cookies.
    pub fn session_for(&self, email: &str) -> String {
        let sid = make_email_id(email, 0, &self.secret);
        let db = rusqlite::Connection::open(self.dir.join("data/mitchpro.db")).unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        db.execute(
            "INSERT INTO names(sid,email,norm_email,created_at) VALUES(?1,?2,?2,?3)",
            rusqlite::params![sid, email, now],
        )
        .unwrap();
        let data = serde_json::json!({ "email": email, "used": true }).to_string();
        db.execute(
            "INSERT INTO tokens(token,data,updated_at) VALUES(?1,?2,?3)",
            rusqlite::params![sid, data, now],
        )
        .unwrap();
        sid
    }

    /// Send a request. `sid` is the session id from [`session_for`], or `None`
    /// for a signed-out visitor.
    pub async fn call(
        &self,
        sid: Option<&str>,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Reply {
        self.call_with(sid, method, path, body, &[]).await
    }

    /// Like [`call`], with extra request headers (e.g. `Origin`,
    /// `X-Admin-Passphrase`). A header named here replaces the default.
    pub async fn call_with(
        &self,
        sid: Option<&str>,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
        headers: &[(&str, &str)],
    ) -> Reply {
        let mut req = self
            .client
            .request(method.parse().unwrap(), format!("{}{path}", self.url))
            .header("X-Mitch-Requested-With", "1");
        for (name, value) in headers {
            req = req.header(*name, *value);
        }
        if let Some(sid) = sid {
            req = req.header("Cookie", format!("studentId={sid}; id={sid}"));
        }
        if let Some(body) = body {
            req = req.json(&body);
        }
        let resp = req.send().await.expect("request");
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        let json = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
        Reply { status, json, text }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
