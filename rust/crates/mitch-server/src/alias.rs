//! Rotating URL aliases: `/<root-uuid>/<section-uuid>/rest` serves `/<section>/rest`.
//!
//! Each UTC day has a *root* UUID for the whole site and a *section* UUID for
//! every top-level section (`game-portal`, `games`, ...), all derived with
//! HMAC-SHA256 from the server's `id_secret`, so nothing is stored. A UUID
//! stays valid for [`VALID_DAYS`] days after the day it was issued for. The
//! tail of the path (a game name, a file) is left as-is so relative links on
//! the page keep resolving. `/<root-uuid>/<real path>` also works.
//!
//! Aliases are not access control: anyone can ask `/api/alias` for today's
//! values, and the real paths keep working. They only change what a URL
//! filter sees. Auth and bans run on the resolved path.
use axum::http::Uri;
use std::collections::BTreeMap;

/// How many days (including the issue day) an alias keeps working.
pub const VALID_DAYS: i64 = 365;

pub fn day_number(now_ms: i64) -> i64 {
    now_ms.div_euclid(86_400_000)
}

/// Format 16 HMAC bytes as a version-4 style UUID.
fn uuid_from(secret: &[u8], label: &str) -> String {
    let mac = mitch_lib::crypto::hmac_sha256(secret, label.as_bytes());
    let mut b = [0u8; 16];
    b.copy_from_slice(&mac[..16]);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

pub fn root_token(secret: &[u8], day: i64) -> String {
    uuid_from(secret, &format!("alias-root:{day}"))
}

pub fn section_token(secret: &[u8], day: i64, section: &str) -> String {
    uuid_from(secret, &format!("alias-section:{day}:{section}"))
}

fn looks_like_uuid(s: &str) -> bool {
    s.len() == 36
        && s.bytes().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == b'-',
            _ => c.is_ascii_hexdigit(),
        })
}

/// Paths that get their own section UUID: the webroot's top-level
/// directories, `proxy/movies`, and every local link in `data/sites` (the
/// homepage's page list) cut back to its directory, e.g. `games/adrian-clicker`.
/// A section is one or more path segments. Hidden directories and `api` are
/// never listed.
pub fn sections(webroot: &std::path::Path, data_dir: &std::path::Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(webroot)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.') && n != "api")
        .collect();
    out.push("proxy".to_string());
    out.push("proxy/movies".to_string());
    if let Ok(list) = std::fs::read_to_string(data_dir.join("sites")) {
        for line in list.lines().filter(|l| !l.trim_start().starts_with("//")) {
            for word in line.split_whitespace().filter(|w| w.starts_with('/')) {
                let path = word.split(['?', '#']).next().unwrap_or("");
                let path = path.trim_matches('/');
                let valid = !path.is_empty()
                    && path.split('/').count() <= 3
                    && !path.starts_with("api/")
                    && !path
                        .split('/')
                        .any(|seg| seg.is_empty() || seg.starts_with('.'))
                    && !path.contains('.');
                if valid {
                    out.push(path.to_string());
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Today's root UUID and the section UUIDs, for `GET /api/alias`.
pub fn today(
    secret: &[u8],
    webroot: &std::path::Path,
    data_dir: &std::path::Path,
    now_ms: i64,
) -> serde_json::Value {
    let day = day_number(now_ms);
    // Matrix chat can't run under a prefix (its client and homeserver URLs are
    // fixed to /matrix/), so it is never handed out; see `is_matrix`.
    let map: BTreeMap<String, String> = sections(webroot, data_dir)
        .into_iter()
        .filter(|s| s != "matrix")
        .map(|s| {
            let t = section_token(secret, day, &s);
            (s, t)
        })
        .collect();
    serde_json::json!({ "root": root_token(secret, day), "sections": map })
}

/// Map an aliased path to the real one, or `None` if it isn't an alias.
pub fn resolve(
    secret: &[u8],
    webroot: &std::path::Path,
    data_dir: &std::path::Path,
    now_ms: i64,
    path: &str,
) -> Option<String> {
    let rest = path.strip_prefix('/')?;
    let (first, tail) = match rest.split_once('/') {
        Some((a, b)) => (a, Some(b)),
        None => (rest, None),
    };
    if !looks_like_uuid(first) {
        return None;
    }
    let today = day_number(now_ms);
    let day = (today - VALID_DAYS + 1..=today)
        .rev()
        .find(|d| root_token(secret, *d) == first)?;
    let tail = tail.unwrap_or("");
    let (second, after) = match tail.split_once('/') {
        Some((a, b)) => (a, Some(b)),
        None => (tail, None),
    };
    if looks_like_uuid(second) {
        let section = sections(webroot, data_dir)
            .into_iter()
            .find(|s| section_token(secret, day, s) == second)?;
        return Some(match after {
            Some(a) => format!("/{section}/{a}"),
            None => format!("/{section}"),
        });
    }
    Some(format!("/{tail}"))
}

/// True for the Matrix chat paths, which are redirected to the real URL
/// instead of being served under an alias.
pub fn is_matrix(path: &str) -> bool {
    path == "/matrix" || path.starts_with("/matrix/")
}

/// Rewrite `uri` to its real path (query kept) when it is an alias.
pub fn rewrite(
    secret: &[u8],
    webroot: &std::path::Path,
    data_dir: &std::path::Path,
    now_ms: i64,
    uri: &Uri,
) -> Option<Uri> {
    let real = resolve(secret, webroot, data_dir, now_ms, uri.path())?;
    let full = match uri.query() {
        Some(q) => format!("{real}?{q}"),
        None => real,
    };
    full.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    const NOW: i64 = 1_800_000_000_000;

    fn webroot() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mitch-alias-test-{}",
            mitch_lib::crypto::random_bytes_hex(8)
        ));
        std::fs::create_dir_all(dir.join("game-portal")).unwrap();
        std::fs::create_dir_all(dir.join("games")).unwrap();
        std::fs::create_dir_all(dir.join("matrix")).unwrap();
        dir
    }

    #[test]
    fn two_level_alias_resolves_and_keeps_the_tail() {
        let (key, dir) = (b"k".as_slice(), webroot());
        let day = day_number(NOW);
        let root = root_token(key, day);
        let gp = section_token(key, day, "game-portal");
        let games = section_token(key, day, "games");
        let r = |p: &str| resolve(key, &dir, &dir, NOW, p);
        assert_eq!(r(&format!("/{root}/{gp}/")).unwrap(), "/game-portal/");
        assert_eq!(
            r(&format!("/{root}/{games}/run-3/index.html")).unwrap(),
            "/games/run-3/index.html"
        );
        assert_eq!(
            r(&format!("/{root}/games/run-3/")).unwrap(),
            "/games/run-3/"
        );
        assert_eq!(r(&format!("/{root}")).unwrap(), "/");
        assert_eq!(r(&format!("/{root}/")).unwrap(), "/");
        assert!(r("/games/run-3/").is_none());
        let mx = section_token(key, day, "matrix");
        assert_eq!(r(&format!("/{root}/{mx}/")).unwrap(), "/matrix/");
        assert!(is_matrix("/matrix/") && is_matrix("/matrix") && !is_matrix("/matrixx"));
        assert!(today(key, &dir, &dir, NOW)["sections"]
            .get("matrix")
            .is_none());
        assert!(r(&format!("/{}/{gp}/", root_token(b"other", day))).is_none());
        // A section UUID from another day does not match this root.
        assert!(r(&format!(
            "/{root}/{}/",
            section_token(key, day - 1, "games")
        ))
        .is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn multi_segment_sections_come_from_the_sites_list() {
        let (key, dir) = (b"k".as_slice(), webroot());
        std::fs::write(
            dir.join("sites"),
            "// comment /hidden/\nurl /games/run-3/ Run 3\nurl https://x.test/a Off\nurl /preferences/#account Prefs\n",
        )
        .unwrap();
        let day = day_number(NOW);
        let root = root_token(key, day);
        let movies = section_token(key, day, "proxy/movies");
        let run3 = section_token(key, day, "games/run-3");
        let r = |p: &str| resolve(key, &dir, &dir, NOW, p);
        assert_eq!(r(&format!("/{root}/{movies}/")).unwrap(), "/proxy/movies/");
        assert_eq!(
            r(&format!("/{root}/{run3}/index.html")).unwrap(),
            "/games/run-3/index.html"
        );
        assert!(r(&format!("/{root}/{}/", section_token(key, day, "hidden"))).is_none());
        let today = today(key, &dir, &dir, NOW);
        assert!(today["sections"].get("preferences").is_some());
        assert!(today["sections"].get("games/run-3").is_some());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn aliases_last_a_year_then_expire() {
        let (key, dir) = (b"k".as_slice(), webroot());
        let issued = day_number(NOW);
        let root = root_token(key, issued);
        let day_ms = 86_400_000;
        let at = |days: i64| resolve(key, &dir, &dir, NOW + days * day_ms, &format!("/{root}/x"));
        assert!(at(0).is_some());
        assert!(at(VALID_DAYS - 1).is_some());
        assert!(at(VALID_DAYS).is_none());
        // Not valid before the day it was issued for.
        assert!(at(-1).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rewrite_keeps_the_query() {
        let (key, dir) = (b"k".as_slice(), webroot());
        let day = day_number(NOW);
        let uri: Uri = format!(
            "/{}/{}/?a=1",
            root_token(key, day),
            section_token(key, day, "games")
        )
        .parse()
        .unwrap();
        assert_eq!(rewrite(key, &dir, &dir, NOW, &uri).unwrap(), "/games/?a=1");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
