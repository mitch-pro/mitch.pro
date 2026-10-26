//! Shared layer for the mitch.pro Rust services.
//!
//! Module layout is fixed by the rewrite plan — one concern per module, no
//! catch-all files. Any function approaching ~300 lines gets split.
//!
//! - [`config`]: env/`.env` loading, path resolution, host→webroot map
//! - [`data`]: SQLite `json_documents` store, `PRESERVED_DATA_FILES`, atomic writes
//! - [`crypto`]: ID_SECRET, `enc1:` at-rest seal/open, HMAC ids, argon2
//! - [`auth`]: sessions, cookies, CSRF, rate-limit tables
//! - [`state`]: shared in-memory State (tokens/coins/presence caches) + flushers
//! - [`email`]: shared email template + delivery helpers
//!
//! Compatibility contract: these modules must stay byte-compatible with the
//! Bun implementation (`server.js`, `lib/data_store.js`, `lib/jsonStore.js`).

// Tests may unwrap; production code may not (Cargo.toml lints).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod achievements;
pub mod admin;
pub mod auth;
pub mod bans;
pub mod bell_sync;
pub mod blog;
pub mod casino;
pub mod chat;
pub mod coins;
pub mod companion;
pub mod config;
pub mod crypto;
pub mod daily_logins;
pub mod data;
pub mod dm;
pub mod e2e;
pub mod email;
pub mod friends;
pub mod guest;
pub mod invites;
pub mod jstime;
pub mod jsval;
pub mod log;
pub mod matrix;
pub mod minigames;
pub mod newsletter_unsub;
pub mod passwords;
pub mod profile;
pub mod profiles;
pub mod push_subs;
pub mod revoked;
pub mod school;
pub mod service_auth;
pub mod shop;
pub mod sso;
pub mod state;
pub mod tokens;
pub mod totp;
pub mod user_stats;
pub mod vm;
pub mod vm_security;
pub mod webauthn;

/// Library version, matching the workspace version. Read from Cargo.toml at
/// compile time, not hand-maintained.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_is_reported() {
        assert!(!super::version().is_empty());
    }
}

pub mod chat_cleanup;
