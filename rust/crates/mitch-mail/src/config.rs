//! Env loading for the mail pipeline, ported from the `.env` parser +
//! `loadDopplerEnv()` duplicated in every `mail/*.js` script.
//!
//! Parity notes:
//! - `.env` lines OVERWRITE process env (the JS parser assigns
//!   `process.env[m[1]] = m[2]` unconditionally), matching the scripts.
//! - Line regex: `^\s*(?:export\s+)?([A-Z_]+)\s*=\s*"?([^"]*)"?\s*$`.
//! - Doppler fallback only sets keys that are still missing
//!   (`if (!process.env[k]) process.env[k] = ...`), trying
//!   `doppler secrets download --format json` then `sudo -n …`.

use std::path::PathBuf;

/// Loads KEY=VAL lines from `path`, overwriting existing vars. Missing file is
/// not an error (the JS scripts swallow the read failure).
pub fn load_env_file(path: &std::path::Path) {
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };
    for line in content.lines() {
        let Some((key, value)) = parse_env_line(line) else {
            continue;
        };
        std::env::set_var(&key, value);
    }
}

/// Parses one `.env` line with the same regex the JS scripts use.
/// Returns None for comments, blanks, and non-`[A-Z_]` keys.
fn parse_env_line(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    let rest = trimmed
        .strip_prefix("export ")
        .unwrap_or(trimmed)
        .trim_start();
    let (key, value) = rest.split_once('=')?;
    let key = key.trim();
    if key.is_empty() || !key.chars().all(|c| c.is_ascii_uppercase() || c == '_') {
        return None;
    }
    let mut value = value.trim().to_string();
    if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
        value = value[1..value.len() - 1].to_string();
    }
    Some((key.to_string(), value))
}

/// `loadDopplerEnv()`: if any of `keys` is missing, pull the full Doppler
/// secret set and fill in the gaps. Non-interactive attempts only.
pub fn ensure_secrets(keys: &[&str]) {
    let missing = keys.iter().any(|k| env_trim(k).is_empty());
    if !missing {
        return;
    }
    for command in [
        "doppler secrets download --format json",
        "sudo -n doppler secrets download --format json",
    ] {
        let Ok(raw) = std::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .output()
        else {
            continue;
        };
        if !raw.status.success() {
            continue;
        }
        let Ok(secrets) =
            serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(&raw.stdout)
        else {
            continue;
        };
        for (k, v) in secrets {
            if env_trim(&k).is_empty() {
                if let Some(s) = v.as_str() {
                    std::env::set_var(&k, s);
                } else if let Some(s) = v.as_i64() {
                    std::env::set_var(&k, s.to_string());
                }
            }
        }
        if keys.iter().all(|k| !env_trim(k).is_empty()) {
            return;
        }
    }
}

/// Env var value, trimmed, with surrounding quotes stripped — the same
/// normalization every script applies to its credentials.
pub fn env_trim(key: &str) -> String {
    std::env::var(key)
        .unwrap_or_default()
        .trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .to_string()
}

/// Runtime configuration for the mail service.
pub struct MailConfig {
    /// Repo root (mount point in docker). Env override: MITCH_BASE.
    pub base_dir: PathBuf,
    /// `<base>/data` unless DATA_DIR is set.
    pub data_dir: PathBuf,
    /// HTTP port of the send service (env MAIL_RS_PORT, default 6902).
    pub port: u16,
    /// MAIL_IMAP_HOST or `mail.mitch.pro`.
    pub imap_host: String,
    /// NTFY_TOPIC (empty disables ntfy notifications).
    pub ntfy_topic: String,
    /// NTFY_USER (optional basic auth username for ntfy).
    pub ntfy_user: String,
    /// NTFY_PASS (optional basic auth password for ntfy).
    pub ntfy_pass: String,
}

impl MailConfig {
    pub fn load() -> Self {
        let base_dir = std::env::var("MITCH_BASE")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
        let data_dir = std::env::var("DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| base_dir.join("data"));
        Self {
            data_dir,
            base_dir,
            port: std::env::var("MAIL_RS_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(6902),
            imap_host: {
                let h = env_trim("MAIL_IMAP_HOST");
                if h.is_empty() {
                    "mail.mitch.pro".into()
                } else {
                    h
                }
            },
            ntfy_topic: env_trim("NTFY_TOPIC"),
            ntfy_user: env_trim("NTFY_USER"),
            ntfy_pass: env_trim("NTFY_PASS"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_env_lines_like_the_scripts() {
        std::env::set_var("MMITCH_TEST_A", "");
        let tmp = std::env::temp_dir().join("mitch-mail-env-test.env");
        std::fs::write(
            &tmp,
            "# comment\nexport FOO_BAR=\"quoted value\"\nBAZ=plain\nlowercase=skip\n123=skip\nURL=https://x.y?z=1\n",
        )
        .unwrap();
        std::env::set_var("FOO_BAR", "before");
        std::env::set_var("BAZ", "before");
        std::env::set_var("URL", "before");
        load_env_file(&tmp);
        // .env overwrites existing env (parity with process.env[k] = v).
        assert_eq!(std::env::var("FOO_BAR").unwrap(), "quoted value");
        assert_eq!(std::env::var("BAZ").unwrap(), "plain");
        assert_eq!(std::env::var("URL").unwrap(), "https://x.y?z=1");
        std::fs::remove_file(&tmp).ok();
    }
}
