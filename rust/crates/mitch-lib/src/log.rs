//! Application logging into the shared SQLite `app_logs` table — the admin
//! panel's log viewer (`/api/admin/app-logs` → `queryAppLogs()`).
//!
//! Contract (from `lib/data_store.js` appendAppLog): level in
//! {debug, info, warn, error} (anything else → info), category lowercased and
//! collapsed to [a-z0-9._-] (max 48 chars, default "general"), message
//! cleaned of ANSI escapes and capped at 4000 chars, details stringified and
//! capped at 8000. Rows older than 30 days are pruned (see `DataStore::prune_app_logs_sync`).

use crate::data::DataStore;
use std::time::{SystemTime, UNIX_EPOCH};

pub const RUST_REWRITE_CATEGORY: &str = "rust-rewrite";

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// A static regex that must compile; failure is a programming error.
#[allow(clippy::expect_used)]
fn static_regex(pattern: &str) -> regex::Regex {
    regex::Regex::new(pattern).expect("static regex")
}

/// `cleanLogText`: strip ANSI escapes, cap length.
pub(crate) fn clean_log_text(value: &str, max: usize) -> String {
    let re = static_regex("\u{1b}\\[[0-9;]*m");
    let mut out: String = re.replace_all(value, "").into();
    if out.len() > max {
        // Cap by chars, like JS String.slice.
        out = out.chars().take(max).collect();
    }
    out
}

fn normalize_level(level: &str) -> &'static str {
    match level.to_lowercase().trim() {
        "debug" => "debug",
        "info" => "info",
        "warn" => "warn",
        "error" => "error",
        _ => "info",
    }
}

fn normalize_category(category: &str) -> String {
    let re = static_regex(r"[^a-z0-9._-]");
    let lowered = category.to_lowercase();
    let collapsed = re
        .replace_all(lowered.trim(), "-")
        .replace("--", "-")
        .replace("--", "-");
    let collapsed = collapsed.trim_matches('-');
    let out = if collapsed.is_empty() {
        "general"
    } else {
        collapsed
    };
    out.chars().take(48).collect()
}

/// `appendAppLog` — one row into app_logs; old rows (30 days) are pruned every
/// 250th write.
pub async fn append_app_log(
    store: &std::sync::Arc<DataStore>,
    level: &str,
    category: &str,
    message: &str,
    details: Option<&str>,
) {
    let level = normalize_level(level);
    let category = normalize_category(category);
    let message = clean_log_text(message, 4000);
    let details = clean_log_text(details.unwrap_or(""), 8000);
    let store = store.clone();
    let _ = tokio::task::spawn_blocking(move || {
        if let Err(e) = store.append_app_log_sync(now_millis(), level, &category, message, details)
        {
            tracing::warn!("app log write failed: {e}");
        }
    })
    .await;
}

/// Fire-and-forget helper for the rewrite's own progress channel: viewable in
/// the admin panel's log viewer under category `rust-rewrite`.
pub async fn log_rewrite(store: &std::sync::Arc<DataStore>, level: &str, message: &str) {
    append_app_log(store, level, RUST_REWRITE_CATEGORY, message, None).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::DataStore;
    use std::path::PathBuf;

    fn temp_store(tag: &str) -> (PathBuf, std::sync::Arc<DataStore>) {
        let base = std::env::temp_dir().join(format!(
            "mitch-lib-log-test-{tag}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(base.join("data")).unwrap();
        (
            base.clone(),
            std::sync::Arc::new(DataStore::open(&base, &base.join("data")).unwrap()),
        )
    }

    #[tokio::test]
    async fn appends_rows_with_normalized_fields() {
        let (base, store) = temp_store("append");
        append_app_log(
            &store,
            "WARN",
            "Rust Rewrite!",
            "progress \u{1b}[31mred\u{1b}[0m",
            None,
        )
        .await;
        let logs = store.query_app_logs_sync("all", "all", "", 100).unwrap();
        assert_eq!(logs.len(), 1);
        let row = &logs[0];
        assert_eq!(row.level, "warn");
        assert_eq!(row.category, "rust-rewrite");
        assert!(!row.message.contains('\u{1b}'));
        std::fs::remove_dir_all(base).ok();
    }

    #[tokio::test]
    async fn message_length_capped() {
        let (base, store) = temp_store("cap");
        append_app_log(&store, "info", "c", &"x".repeat(6000), None).await;
        let logs = store.query_app_logs_sync("all", "all", "", 100).unwrap();
        assert_eq!(logs[0].message.chars().count(), 4000);
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn prune_drops_rows_older_than_thirty_days() {
        let (base, store) = temp_store("prune");
        let now = now_millis();
        let day = 24 * 60 * 60 * 1000;
        store
            .append_app_log_sync(now - 31 * day, "info", "c", "old".into(), String::new())
            .unwrap();
        store
            .append_app_log_sync(now - 29 * day, "info", "c", "recent".into(), String::new())
            .unwrap();
        store.prune_app_logs_sync(now).unwrap();
        let logs = store.query_app_logs_sync("all", "all", "", 100).unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].message, "recent");
        std::fs::remove_dir_all(base).ok();
    }
}
