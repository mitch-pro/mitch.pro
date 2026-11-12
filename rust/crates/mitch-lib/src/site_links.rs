//! The homepage's "All pages" catalog — the `site_links` table.
//!
//! This replaces the old plain-text `data/sites` file (one `url /path Label`
//! line per link, `//` for disabled ones, an `admin ` prefix for staff-only).
//! The file was hand-edited, regex-patched by the admin mirror tool, and
//! re-parsed by three different readers (homepage API, URL aliases, admin).
//! Now there is one table and one set of functions.
//!
//! ## Rows
//! - `kind`: how the homepage opens it (`url`, `iframe`, `popup`, ...).
//! - `visible`: `0` hides the row without deleting it (the old `//` lines).
//! - `deprecated`: the page was retired from the site's navigation. Deprecated
//!   rows are forced hidden on every startup by [`RETIRED_HREFS`], so a stale
//!   database or an old copy of `data/sites` cannot bring one back. The page
//!   itself is documented as deprecated in `docs/`; only the link is gone.
//! - `sort_order`: display order within a category (the homepage groups by
//!   label itself).
//!
//! ## Legacy import
//! On the first start with an empty table, [`seed`] imports `data/sites` if it
//! still exists (production keeps its admin edits), otherwise [`DEFAULT_LINKS`].
//! The file is never written again and is no longer in the repository.

use crate::data::DataStore;
use std::path::Path;

/// One catalog row as the homepage consumes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteLink {
    pub kind: String,
    pub href: String,
    pub label: String,
    pub admin_only: bool,
}

/// Links retired from navigation. Forced `visible = 0, deprecated = 1` on
/// every startup (see the module docs). Keep `docs/deprecated-pages.md` in
/// sync when adding to this list.
pub const RETIRED_HREFS: &[&str] = &[
    // MitchCoin Games section
    "/games/adrian-clicker/",
    "/games/kodys-keyboard/",
    "/games/lillians-logic/",
    "/games/richards-riches/",
    "/games/pennys-piano-keys/",
    "/games/sebastians-piccolo/",
    "/games/battleship/",
    "/games/jeopardy-battle/",
    // Marketplace, PixelPlace
    "/marketplace/",
    "/canvas/",
    // Join the Team moved into Feedback; account settings are just Preferences.
    "/apply/",
    "/preferences/#account",
    // Utilities pared down to inventory, companion, saves, notifications, blog.
    "/",
    "/casino/",
    "/games/retro/",
    "/ab/",
    "/tabs/",
    "/bell/",
    "https://discord.gg/P5vZgTydXw",
    // Still active; unlinked here because the homepage already reaches them
    // elsewhere (Shortcuts, Explore, the People rail and the mobile menu).
    "/games/",
    "/matrix/",
    "/invite/",
    "/shop/",
    "/friends/",
    "/members/",
];

/// Links every install must have. Inserted if no row with that `href` exists.
const REQUIRED_LINKS: &[(&str, &str, &str)] = &[
    ("url", "/learn/", "Learn About the Site"),
    ("url", "/support/", "Support"),
];

/// Catalog used when there is no legacy `data/sites` to import. Entries with
/// a leading `//` are disabled (kept for admins to re-enable by SQL).
const DEFAULT_LINKS: &str = "\
url /inventory/ Inventory
url /blog/ Blog
url /notifications/ Notification Settings
url /leaderboard/ Leaderboard
url /feedback/ Feedback
url /saves/ Save Manager
url /companion/ Companion
// url https://docs.google.com/document/d/1w8lZBN6pa4SlR6ZdHg93nlpGr_6YHe7amgtdBvcq6eo/edit?usp=sharing Mitch.pro Mirrors
";

/// Creates the table. Called from `DataStore::open`.
pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS site_links (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL DEFAULT 'url',
    href TEXT NOT NULL,
    label TEXT NOT NULL,
    admin_only INTEGER NOT NULL DEFAULT 0,
    visible INTEGER NOT NULL DEFAULT 1,
    deprecated INTEGER NOT NULL DEFAULT 0,
    sort_order INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_site_links_order ON site_links(visible, sort_order);
CREATE INDEX IF NOT EXISTS idx_site_links_href ON site_links(href);
";

/// One parsed legacy line: `(link, visible)`.
fn parse_line(line: &str) -> Option<(SiteLink, bool)> {
    let mut line = line.trim();
    let mut visible = true;
    if let Some(rest) = line.strip_prefix("//") {
        visible = false;
        line = rest.trim();
    }
    let mut parts = line.split_whitespace().peekable();
    let admin_only = parts.peek() == Some(&"admin");
    if admin_only {
        parts.next();
    }
    let kind = parts.next()?;
    let href = parts.next()?;
    let label = parts.collect::<Vec<_>>().join(" ");
    // A disabled prose comment ("// Lines starting with // ...") has no
    // recognizable kind; only import lines that look like entries.
    let known_kind = matches!(
        kind,
        "url" | "iframe" | "eframe" | "popup" | "tab" | "newtab"
    );
    if !known_kind || label.is_empty() {
        return None;
    }
    Some((
        SiteLink {
            kind: kind.to_string(),
            href: href.to_string(),
            label,
            admin_only,
        },
        visible,
    ))
}

/// Fills an empty table (legacy file, else defaults), then applies the
/// retirement list and required links. Idempotent; cheap after the first run.
pub fn seed(store: &DataStore, data_dir: &Path) {
    let conn = store.conn();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM site_links", [], |r| r.get(0))
        .unwrap_or(0);
    if count == 0 {
        let raw = std::fs::read_to_string(data_dir.join("sites"))
            .unwrap_or_else(|_| DEFAULT_LINKS.to_string());
        for (i, (link, visible)) in raw.lines().filter_map(parse_line).enumerate() {
            let _ = conn.execute(
                "INSERT INTO site_links (kind, href, label, admin_only, visible, sort_order)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    link.kind,
                    link.href,
                    link.label,
                    link.admin_only,
                    visible,
                    i as i64
                ],
            );
        }
    }
    for href in RETIRED_HREFS {
        let _ = conn.execute(
            "UPDATE site_links SET visible = 0, deprecated = 1 WHERE href = ?1",
            rusqlite::params![href],
        );
    }
    for (kind, href, label) in REQUIRED_LINKS {
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM site_links WHERE href = ?1)",
                rusqlite::params![href],
                |r| r.get(0),
            )
            .unwrap_or(true);
        if !exists {
            let _ = conn.execute(
                "INSERT INTO site_links (kind, href, label, sort_order)
                 VALUES (?1, ?2, ?3, (SELECT COALESCE(MAX(sort_order), 0) + 1 FROM site_links))",
                rusqlite::params![kind, href, label],
            );
        }
    }
}

/// Visible, non-deprecated links in display order. Staff-only links are
/// included only when `include_admin` is set.
pub fn list_visible(store: &DataStore, include_admin: bool) -> Vec<SiteLink> {
    let conn = store.conn();
    let Ok(mut stmt) = conn.prepare(
        "SELECT kind, href, label, admin_only FROM site_links
         WHERE visible = 1 AND deprecated = 0 AND (admin_only = 0 OR ?1)
         ORDER BY sort_order, id",
    ) else {
        return Vec::new();
    };
    stmt.query_map(rusqlite::params![include_admin], |r| {
        Ok(SiteLink {
            kind: r.get(0)?,
            href: r.get(1)?,
            label: r.get(2)?,
            admin_only: r.get(3)?,
        })
    })
    .map(|rows| rows.flatten().collect())
    .unwrap_or_default()
}

/// The legacy line format the homepage's `parseSites()` reads:
/// `[admin ]kind href label`, one per line.
pub fn render_lines(links: &[SiteLink]) -> String {
    links
        .iter()
        .map(|l| {
            let prefix = if l.admin_only { "admin " } else { "" };
            format!("{prefix}{} {} {}", l.kind, l.href, l.label)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Local (`/path`) hrefs of every visible, non-deprecated row —
/// the URL-alias system hands out section UUIDs for these directories.
pub fn local_hrefs(store: &DataStore) -> Vec<String> {
    let conn = store.conn();
    let Ok(mut stmt) = conn.prepare(
        "SELECT href FROM site_links WHERE visible = 1 AND deprecated = 0 AND href LIKE '/%'",
    ) else {
        return Vec::new();
    };
    stmt.query_map([], |r| r.get::<_, String>(0))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

/// Points the "Mitch.pro Mirrors" row at a new document URL (admin
/// content-mirror tool). Returns whether a row was updated.
pub fn set_mirror_url(store: &DataStore, url: &str) -> bool {
    store
        .conn()
        .execute(
            "UPDATE site_links SET href = ?1 WHERE label = 'Mitch.pro Mirrors'",
            rusqlite::params![url],
        )
        .map(|n| n > 0)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(tag: &str) -> (DataStore, std::path::PathBuf) {
        let base = std::env::temp_dir().join(format!(
            "mitch-site-links-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(base.join("data")).unwrap();
        let s = DataStore::open(&base, &base.join("data")).unwrap();
        (s, base)
    }

    #[test]
    fn defaults_exclude_retired_and_include_learn() {
        let (s, base) = store("defaults");
        let hrefs: Vec<_> = list_visible(&s, false)
            .into_iter()
            .map(|l| l.href)
            .collect();
        assert!(hrefs.contains(&"/learn/".to_string()));
        assert_eq!(hrefs.len(), 9);
        assert!(hrefs.contains(&"/inventory/".to_string()));
        for retired in RETIRED_HREFS {
            assert!(!hrefs.contains(&retired.to_string()), "{retired}");
        }
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn legacy_file_is_imported_then_retirements_apply() {
        let base = std::env::temp_dir().join(format!(
            "mitch-site-links-legacy-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(base.join("data")).unwrap();
        std::fs::write(
            base.join("data/sites"),
            "// prose comment\nurl /marketplace/ Marketplace\nadmin url /staff/ Staff\n// url /old/ Old\nurl /blog/ Blog\n",
        )
        .unwrap();
        let s = DataStore::open(&base, &base.join("data")).unwrap();
        let user: Vec<_> = list_visible(&s, false)
            .into_iter()
            .map(|l| l.href)
            .collect();
        assert!(!user.contains(&"/marketplace/".to_string()));
        assert!(!user.contains(&"/staff/".to_string()));
        assert!(!user.contains(&"/old/".to_string()));
        assert!(user.contains(&"/blog/".to_string()));
        let admin = list_visible(&s, true);
        assert!(render_lines(&admin).contains("admin url /staff/ Staff"));
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn mirror_update_and_local_hrefs() {
        let (s, base) = store("mirror");
        assert!(set_mirror_url(&s, "https://example.test/doc"));
        assert!(local_hrefs(&s).contains(&"/blog/".to_string()));
        assert!(!local_hrefs(&s).contains(&"/marketplace/".to_string()));
        std::fs::remove_dir_all(base).unwrap();
    }
}
