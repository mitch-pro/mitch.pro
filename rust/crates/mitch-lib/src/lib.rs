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
pub mod blog;
pub mod chat;
pub mod coins;
pub mod config;
pub mod crypto;
pub mod data;
pub mod dm;
pub mod e2e;
pub mod email;
pub mod guest;
pub mod invites;
pub mod jstime;
pub mod jsval;
pub mod log;
pub mod matrix;
pub mod minigames;
pub mod profile;
pub mod school;
pub mod shop;
pub mod sso;
pub mod state;
pub mod totp;
pub mod vm;
pub mod vm_security;
pub mod webauthn;

/// Library version, matching the workspace version.
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
