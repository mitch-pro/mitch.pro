//! `MITCH_DEV=1`: one switch for running the server on a development machine.
//!
//! It fills in the settings a dev run wants, each only when it is not already
//! set, so any of them can still be overridden:
//!
//! | Setting | Dev value | Why |
//! | --- | --- | --- |
//! | `MITCH_STATIC_LIVE` | `1` | frontend files are read from disk on every request, so edits show up on reload (no startup snapshot) |
//! | `GOD_MODE` | `1` | every visitor is signed in as the dev admin |
//! | `NODE_ENV` | `development` | |
//! | `PORT` | `8080` | the documented local port |
//! | `LISTEN_HOST` | `127.0.0.1` | God mode must not be reachable from the network |
//! | `SESSION_COOKIE_SECURE` | `0` | cookies work over plain http |
//! | `RECAPTCHA_SITE_KEY` / `_SECRET_KEY` | empty | no captcha |
//! | `MOVIES_EGRESS_INTERFACE` | `direct` | outbound traffic uses the system's default route, not the production VPN interface |
//! | `PROXMOX_VM_BRIDGE` | `vmbr0` | Proxmox's default bridge (production requires it to be set explicitly) |
//! | `MITCH_DEV_NO_MAIL` | `1` | the email digest workers do not run, so a dev server with a copy of real data can't email anyone |
//!
//! `MITCH_DEV` refuses to start when `NODE_ENV=production`, the same guard as
//! God mode. See `docs/dev-mode.md`.

/// Whether the all-in-one dev flag is on.
pub fn enabled() -> bool {
    flag("MITCH_DEV")
}

/// Whether background email workers should stay off.
pub fn mail_disabled() -> bool {
    flag("MITCH_DEV_NO_MAIL")
}

fn flag(name: &str) -> bool {
    matches!(
        std::env::var(name)
            .unwrap_or_default()
            .trim()
            .to_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// The dev defaults as `(name, value)`, in the order they are applied.
pub const DEFAULTS: &[(&str, &str)] = &[
    ("MITCH_STATIC_LIVE", "1"),
    ("GOD_MODE", "1"),
    ("NODE_ENV", "development"),
    ("PORT", "8080"),
    ("LISTEN_HOST", "127.0.0.1"),
    ("SESSION_COOKIE_SECURE", "0"),
    ("RECAPTCHA_SITE_KEY", ""),
    ("RECAPTCHA_SECRET_KEY", ""),
    ("MOVIES_EGRESS_INTERFACE", "direct"),
    ("PROXMOX_VM_BRIDGE", "vmbr0"),
    ("MITCH_DEV_NO_MAIL", "1"),
];

/// Which defaults would be applied given a way to look up the environment.
pub fn missing(get: impl Fn(&str) -> Option<String>) -> Vec<(&'static str, &'static str)> {
    DEFAULTS
        .iter()
        .copied()
        .filter(|(name, _)| get(name).is_none())
        .collect()
}

/// Applies the dev defaults. Call first thing in `main`, before anything reads
/// the environment. Panics when `NODE_ENV=production`.
pub fn apply() {
    if !enabled() {
        return;
    }
    if std::env::var("NODE_ENV").as_deref() == Ok("production") {
        panic!(
            "MITCH_DEV=1 is for development machines and refuses to run with NODE_ENV=production"
        );
    }
    for (name, value) in missing(|n| std::env::var(n).ok()) {
        std::env::set_var(name, value);
    }
}

/// Prints what dev mode turned on (call after logging is set up).
pub fn announce() {
    if !enabled() {
        return;
    }
    let bar = "=".repeat(60);
    tracing::warn!(
        "\n{bar}\n  MITCH_DEV=1: development mode.\n  - frontend read from disk on every request (no snapshot)\n  - GOD_MODE on: every visitor is the dev admin\n  - listening on {host}:{port} only, cookies not Secure, no captcha\n  - Movies and VMs use the system's default network\n  - email digest workers off\n  Never use on a server with real users. Refuses NODE_ENV=production.\n{bar}",
        host = std::env::var("LISTEN_HOST").unwrap_or_default(),
        port = std::env::var("PORT").unwrap_or_default(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_unset_settings_are_filled_in() {
        let set = |n: &str| (n == "PORT").then(|| "9000".to_string());
        let todo = missing(set);
        assert!(
            !todo.iter().any(|(n, _)| *n == "PORT"),
            "an explicit value wins"
        );
        assert!(todo.contains(&("GOD_MODE", "1")));
        assert!(todo.contains(&("LISTEN_HOST", "127.0.0.1")));
        assert!(todo.contains(&("MITCH_STATIC_LIVE", "1")));
    }

    #[test]
    fn dev_mode_never_listens_on_every_interface() {
        let listen = DEFAULTS.iter().find(|(n, _)| *n == "LISTEN_HOST").unwrap();
        assert_eq!(listen.1, "127.0.0.1");
    }
}
