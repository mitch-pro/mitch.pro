//! Site content that used to live in JSON files under `data/`, now real tables:
//! `bad_words`, `moderators`, `prox_blocklist`, `game_categories`, `madlibs` /
//! `madlib_parts`, and the portal catalog (`portal_catalog_groups` /
//! `portal_catalog_games`). AGENTS.md: no new JSON blobs; anything new is a table.
//!
//! # Where the first rows come from
//!
//! [`seed`] runs on every `DataStore::open` and only fills a table that is empty,
//! so it is a cheap no-op after the first boot and never overwrites edits:
//!
//! - **Built in** (`site_content_seed.rs`): the madlibs and the base/local game
//!   categories. These were committed JSON files; they are Rust constants now.
//! - **Imported from a legacy file** if one is on disk: `moderators.json` and
//!   `prox_blocklist.json` (staff-edited at runtime, so the production copy can
//!   differ from git; imported once, recorded in `metadata`, so a list staff later
//!   empty is never refilled from the file), and
//!   `game_categories_external.json` / `portal_catalog.json`
//!   (large crawled catalogs, too big to embed in code). The files stay in the
//!   repo for one release as the import source and nothing writes them any more.
//!   Delete them in a later change once every server has booted this version.
//!
//! The JSON shapes the HTTP API returns are unchanged; the accessors rebuild them.

use crate::data::DataStore;
use rusqlite::{params, Connection};
use serde_json::{json, Map, Value};
use std::path::Path;

/// Created by `DataStore::open` together with the other tables.
pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS bad_words (
    word TEXT PRIMARY KEY
);
CREATE TABLE IF NOT EXISTS moderators (
    email TEXT PRIMARY KEY
);
CREATE TABLE IF NOT EXISTS prox_blocklist (
    domain TEXT PRIMARY KEY
);
CREATE TABLE IF NOT EXISTS game_categories (
    source TEXT NOT NULL CHECK (source IN ('base', 'local', 'external')),
    path TEXT NOT NULL,
    category TEXT NOT NULL,
    PRIMARY KEY (source, path)
);
CREATE TABLE IF NOT EXISTS madlibs (
    id INTEGER PRIMARY KEY,
    title TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS madlib_parts (
    madlib_id INTEGER NOT NULL REFERENCES madlibs(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN ('text', 'blank')),
    position INTEGER NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY (madlib_id, kind, position)
);
CREATE TABLE IF NOT EXISTS portal_catalog_groups (
    position INTEGER PRIMARY KEY,
    title TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS portal_catalog_games (
    group_position INTEGER NOT NULL REFERENCES portal_catalog_groups(position) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    title TEXT NOT NULL,
    image TEXT NOT NULL DEFAULT '',
    url TEXT NOT NULL DEFAULT '',
    description TEXT NOT NULL DEFAULT '',
    flag TEXT,
    PRIMARY KEY (group_position, position)
);
";

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn count(conn: &Connection, table: &str, filter: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table} {filter}"), [], |r| {
        r.get(0)
    })
    .unwrap_or(0)
}

/// Reads a legacy file that may sit in `data_dir` (or, for the pre-split layout,
/// `base_dir/data`). Missing or unparsable files read as `None`.
fn legacy_json(store: &DataStore, name: &str) -> Option<Value> {
    let paths = [
        store.data_dir().join(name),
        store.base_dir.join("data").join(name),
    ];
    paths.iter().find_map(|p| read_json_file(p))
}

fn read_json_file(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// Fills any empty content table. Idempotent; see the module docs.
pub fn seed(store: &DataStore) {
    let mut conn = store.conn();
    if let Err(e) = seed_conn(&mut conn, store) {
        eprintln!("[site_content] seeding failed: {e}");
    }
}

fn seed_conn(conn: &mut Connection, store: &DataStore) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;

    if count(&tx, "madlibs", "") == 0 {
        for (title, text, blanks) in crate::site_content_seed::MADLIBS {
            tx.execute("INSERT INTO madlibs (title) VALUES (?1)", params![title])?;
            let id = tx.last_insert_rowid();
            for (kind, parts) in [("text", *text), ("blank", *blanks)] {
                for (i, value) in parts.iter().enumerate() {
                    tx.execute(
                        "INSERT INTO madlib_parts (madlib_id, kind, position, value) VALUES (?1, ?2, ?3, ?4)",
                        params![id, kind, i as i64, value],
                    )?;
                }
            }
        }
    }

    for (source, rows) in [
        ("base", crate::site_content_seed::GAME_CATEGORIES_BASE),
        ("local", crate::site_content_seed::GAME_CATEGORIES_LOCAL),
    ] {
        if count(
            &tx,
            "game_categories",
            &format!("WHERE source = '{source}'"),
        ) == 0
        {
            for (path, category) in rows {
                tx.execute(
                    "INSERT OR IGNORE INTO game_categories (source, path, category) VALUES (?1, ?2, ?3)",
                    params![source, path, category],
                )?;
            }
        }
    }

    if count(&tx, "game_categories", "WHERE source = 'external'") == 0 {
        if let Some(Value::Object(map)) = legacy_json(store, "game_categories_external.json") {
            let mut insert = tx.prepare(
                "INSERT OR IGNORE INTO game_categories (source, path, category) VALUES ('external', ?1, ?2)",
            )?;
            for (path, category) in &map {
                if let Some(category) = category.as_str() {
                    insert.execute(params![path, category])?;
                }
            }
        }
    }

    // Staff-edited lists are imported from the old file exactly once, recorded in
    // `metadata`. Keying on "table is empty" instead would re-import the file
    // after staff removed the last entry, bringing a demoted moderator back.
    for (table, column, file) in [
        ("moderators", "email", "moderators.json"),
        ("prox_blocklist", "domain", "prox_blocklist.json"),
    ] {
        let marker = format!("site_content_imported:{table}");
        let done: bool = tx
            .query_row(
                "SELECT 1 FROM metadata WHERE key = ?1",
                params![marker],
                |_| Ok(true),
            )
            .unwrap_or(false);
        if done {
            continue;
        }
        if let Some(Value::Array(list)) = legacy_json(store, file) {
            for value in list.iter().filter_map(Value::as_str) {
                tx.execute(
                    &format!("INSERT OR IGNORE INTO {table} ({column}) VALUES (?1)"),
                    params![value],
                )?;
            }
        }
        tx.execute(
            "INSERT OR IGNORE INTO metadata (key, value, updated_at) VALUES (?1, '1', ?2)",
            params![marker, now_millis()],
        )?;
    }

    if count(&tx, "portal_catalog_groups", "") == 0 {
        if let Some(data) = legacy_json(store, "portal_catalog.json") {
            import_portal_catalog(&tx, &data)?;
        }
    }

    tx.commit()
}

/// `{"links": [{"title", "games": [[title, image, url, description, flag?]]}]}`.
fn import_portal_catalog(conn: &Connection, data: &Value) -> rusqlite::Result<()> {
    let mut group =
        conn.prepare("INSERT INTO portal_catalog_groups (position, title) VALUES (?1, ?2)")?;
    let mut game = conn.prepare(
        "INSERT INTO portal_catalog_games (group_position, position, title, image, url, description, flag)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )?;
    for (gi, section) in data["links"].as_array().into_iter().flatten().enumerate() {
        group.execute(params![gi as i64, section["title"].as_str().unwrap_or("")])?;
        for (pi, entry) in section["games"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            let field = |i: usize| entry.get(i).and_then(Value::as_str).unwrap_or("");
            // Only an explicit fifth element is a flag; keep "absent" distinct from "".
            let flag = entry.get(4).and_then(Value::as_str);
            game.execute(params![
                gi as i64,
                pi as i64,
                field(0),
                field(1),
                field(2),
                field(3),
                flag
            ])?;
        }
    }
    Ok(())
}

// ---- bad words -------------------------------------------------------------

/// Words that make a nickname unusable (case-sensitive substring match; see e2e.rs).
pub fn bad_words(store: &DataStore) -> Vec<String> {
    string_column(&store.conn(), "SELECT word FROM bad_words ORDER BY rowid")
}

// ---- moderators ------------------------------------------------------------

/// Moderator emails as stored (raw, not normalized), in the order they were added.
pub fn moderators(store: &DataStore) -> Vec<String> {
    string_column(&store.conn(), "SELECT email FROM moderators ORDER BY rowid")
}

/// Replaces the moderator list. One transaction, so readers never see it half-written.
pub fn set_moderators(store: &DataStore, emails: &[String]) {
    let conn = store.conn();
    let _ = conn.execute_batch("BEGIN IMMEDIATE");
    let ok = conn.execute("DELETE FROM moderators", []).is_ok()
        && emails.iter().all(|e| {
            conn.execute(
                "INSERT OR IGNORE INTO moderators (email) VALUES (?1)",
                params![e],
            )
            .is_ok()
        });
    let _ = conn.execute_batch(if ok { "COMMIT" } else { "ROLLBACK" });
}

// ---- proxy blocklist -------------------------------------------------------

pub fn prox_blocklist(store: &DataStore) -> Vec<String> {
    string_column(
        &store.conn(),
        "SELECT domain FROM prox_blocklist ORDER BY rowid",
    )
}

pub fn prox_block_add(store: &DataStore, domain: &str) {
    let _ = store.conn().execute(
        "INSERT OR IGNORE INTO prox_blocklist (domain) VALUES (?1)",
        params![domain],
    );
}

fn string_column(conn: &Connection, sql: &str) -> Vec<String> {
    conn.prepare(sql)
        .and_then(|mut s| s.query_map([], |r| r.get(0))?.collect())
        .unwrap_or_default()
}

// ---- game categories -------------------------------------------------------

/// `{path: category}` merged base, then local, then external: a later source
/// overrides an earlier one for the same path.
pub fn game_categories(store: &DataStore) -> Map<String, Value> {
    let conn = store.conn();
    let mut out = Map::new();
    let Ok(mut stmt) = conn.prepare(
        "SELECT path, category FROM game_categories
         ORDER BY CASE source WHEN 'base' THEN 0 WHEN 'local' THEN 1 ELSE 2 END",
    ) else {
        return out;
    };
    if let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))) {
        for (path, category) in rows.flatten() {
            out.insert(path, Value::String(category));
        }
    }
    out
}

// ---- madlibs ---------------------------------------------------------------

/// Every story as `{"title", "text": [...], "blanks": [...]}`, in id order.
pub fn madlibs(store: &DataStore) -> Vec<Value> {
    let conn = store.conn();
    let Ok(mut stmt) = conn.prepare(
        "SELECT m.id, m.title, p.kind, p.value FROM madlibs m
         LEFT JOIN madlib_parts p ON p.madlib_id = m.id
         ORDER BY m.id, p.kind, p.position",
    ) else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, Option<String>>(3)?,
        ))
    }) else {
        return Vec::new();
    };
    // Rows arrive grouped by story id; two stories may share a title, so group on the id.
    let mut stories: Vec<(i64, String, Vec<Value>, Vec<Value>)> = Vec::new();
    for (id, title, kind, value) in rows.flatten() {
        if stories.last().map(|s| s.0) != Some(id) {
            stories.push((id, title, Vec::new(), Vec::new()));
        }
        if let (Some(kind), Some(value), Some(story)) = (kind, value, stories.last_mut()) {
            if kind == "text" {
                story.2.push(Value::String(value));
            } else {
                story.3.push(Value::String(value));
            }
        }
    }
    stories
        .into_iter()
        .map(|(_, title, text, blanks)| json!({ "title": title, "text": text, "blanks": blanks }))
        .collect()
}

// ---- portal catalog --------------------------------------------------------

/// The catalog as the original JSON, `{"links": [{"title", "games": [[...]]}]}`,
/// so `Catalog::from_json` keeps its ordering and ids exactly.
pub fn portal_catalog(store: &DataStore) -> Value {
    let conn = store.conn();
    let Ok(mut stmt) = conn.prepare(
        "SELECT g.position, g.title, a.title, a.image, a.url, a.description, a.flag
         FROM portal_catalog_groups g
         LEFT JOIN portal_catalog_games a ON a.group_position = g.position
         ORDER BY g.position, a.position",
    ) else {
        return json!({ "links": [] });
    };
    let Ok(rows) = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, Option<String>>(4)?,
            r.get::<_, Option<String>>(5)?,
            r.get::<_, Option<String>>(6)?,
        ))
    }) else {
        return json!({ "links": [] });
    };
    let mut links: Vec<(i64, String, Vec<Value>)> = Vec::new();
    for (pos, group_title, title, image, url, description, flag) in rows.flatten() {
        if links.last().map(|l| l.0) != Some(pos) {
            links.push((pos, group_title, Vec::new()));
        }
        if let Some(title) = title {
            let mut entry = vec![
                json!(title),
                json!(image.unwrap_or_default()),
                json!(url.unwrap_or_default()),
                json!(description.unwrap_or_default()),
            ];
            if let Some(flag) = flag {
                entry.push(json!(flag));
            }
            if let Some(link) = links.last_mut() {
                link.2.push(Value::Array(entry));
            }
        }
    }
    json!({ "links": links.into_iter().map(|(_, title, games)| json!({ "title": title, "games": games })).collect::<Vec<_>>() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "site-content-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("data")).unwrap();
        dir
    }

    fn open(dir: &std::path::Path) -> DataStore {
        DataStore::open(dir, &dir.join("data")).unwrap()
    }

    #[test]
    fn built_in_content_is_seeded_once() {
        let dir = temp_dir("builtin");
        let store = open(&dir);
        let stories = madlibs(&store);
        assert_eq!(stories.len(), crate::site_content_seed::MADLIBS.len());
        for story in &stories {
            // A story is its text segments with one blank between each pair.
            assert_eq!(
                story["text"].as_array().unwrap().len(),
                story["blanks"].as_array().unwrap().len() + 1
            );
        }
        let cats = game_categories(&store);
        assert!(cats.len() >= crate::site_content_seed::GAME_CATEGORIES_BASE.len());
        assert_eq!(cats["games/retro/"], "retro");

        // Reopening must not duplicate anything.
        drop(store);
        let store = open(&dir);
        assert_eq!(madlibs(&store).len(), stories.len());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn later_game_category_sources_override_earlier_ones() {
        let dir = temp_dir("cats");
        let store = open(&dir);
        {
            let conn = store.conn();
            conn.execute(
                "INSERT INTO game_categories (source, path, category) VALUES ('base', 'x/', 'a'), ('local', 'x/', 'b'), ('external', 'x/', 'c'), ('base', 'y/', 'a')",
                [],
            )
            .unwrap();
        }
        let cats = game_categories(&store);
        assert_eq!(cats["x/"], "c");
        assert_eq!(cats["y/"], "a");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_files_are_imported_once_and_never_overwrite_edits() {
        let dir = temp_dir("legacy");
        let data = dir.join("data");
        std::fs::write(
            data.join("moderators.json"),
            r#"["Mod@Example.com","b@x.test"]"#,
        )
        .unwrap();
        std::fs::write(data.join("prox_blocklist.json"), r#"["test.com"]"#).unwrap();
        std::fs::write(
            data.join("game_categories_external.json"),
            r#"{"https://e.test/g/":"puzzle"}"#,
        )
        .unwrap();
        let store = open(&dir);
        assert_eq!(moderators(&store), ["Mod@Example.com", "b@x.test"]);
        assert_eq!(prox_blocklist(&store), ["test.com"]);
        assert_eq!(game_categories(&store)["https://e.test/g/"], "puzzle");

        // Edits made through the API survive a reboot even though the old file is still there.
        set_moderators(&store, &["only@x.test".to_string()]);
        prox_block_add(&store, "new.test");
        drop(store);
        let store = open(&dir);
        assert_eq!(moderators(&store), ["only@x.test"]);
        assert_eq!(prox_blocklist(&store), ["test.com", "new.test"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_emptied_moderator_list_stays_empty_after_reboot() {
        // Removing the last moderator must not re-import the old file's list.
        let dir = temp_dir("emptied");
        std::fs::write(dir.join("data/moderators.json"), r#"["a@x.test"]"#).unwrap();
        let store = open(&dir);
        assert_eq!(moderators(&store).len(), 1);
        set_moderators(&store, &[]);
        drop(store);
        let store = open(&dir);
        assert!(
            moderators(&store).is_empty(),
            "removed moderator came back from the legacy file"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn portal_catalog_round_trips_including_absent_flag() {
        let dir = temp_dir("portal");
        let catalog = json!({"links": [
            {"title": "A", "games": [["One", "img1", "url1", "d1", "IgnoreIframe"], ["Two", "img2", "url2", ""]]},
            {"title": "Empty", "games": []},
            {"title": "B", "games": [["Three", "", "url3", "d3", ""]]}
        ]});
        std::fs::write(dir.join("data/portal_catalog.json"), catalog.to_string()).unwrap();
        let store = open(&dir);
        assert_eq!(portal_catalog(&store), catalog);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
