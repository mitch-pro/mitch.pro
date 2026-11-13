# Site content tables

Content that used to be JSON files under `data/` lives in SQL tables. Code:
`rust/crates/mitch-lib/src/site_content.rs` (tables, seeding, typed accessors) and
`site_content_seed.rs` (built-in content). The HTTP responses keep their old JSON shapes.

| Table | Was | Written by | Source of first rows |
| --- | --- | --- | --- |
| `bad_words` | `bad_words.json` | nothing yet (empty) | none |
| `moderators` | `moderators.json` | `POST /api/admin/moderators`, admin actions | imported once from `data/moderators.json` |
| `prox_blocklist` | `prox_blocklist.json` | admin action `prox_block` | imported once from `data/prox_blocklist.json` |
| `game_categories` (`source` = base, local, external) | `game_categories*.json` | nothing | base/local built in; external imported from `data/game_categories_external.json` |
| `madlibs`, `madlib_parts` | `madlibs.json` (twice) | nothing | built in |
| `portal_catalog_groups`, `portal_catalog_games` | `portal_catalog.json` | nothing | imported from `data/portal_catalog.json` |
| `bell_overrides`, `bell_override_periods` | `bell_overrides.json` | `/admin/special-schedule/` (see `docs/bell-overrides.md`) | imported once from `data/bell_overrides.json` |

Deleted as unused (nothing read them): `logic_words.json`, `payloads.json`,
`shop_catalog_overrides.json` (the shop reads `shop_catalog.json`, a different file).

## Seeding

`site_content::seed` runs on every `DataStore::open` and never overwrites edits.

- **Built in** content is inserted only when its table is empty. Change it by editing the
  rows (or `site_content_seed.rs` for a fresh database).
- **Staff-edited lists** (`moderators`, `prox_blocklist`) are imported from the old file
  **once**, recorded in `metadata` as `site_content_imported:<table>`. Do not change this to
  "import when empty": removing the last moderator would then bring the old list back.
- **Large catalogs** (`game_categories` external, portal catalog) are imported from their
  file when their table is empty. They are crawled data, too big to embed in code, so the
  files stay in the repo as the only source until an importer or crawler writes the tables.

`data/moderators.json`, `data/prox_blocklist.json`, `data/game_categories_external.json` and
`data/portal_catalog.json` are import sources only; nothing writes them. Do not delete the
two staff-edited ones from git until every server has booted this version: `git pull` would
delete the live copy (or fail on local changes) before the import runs.

## Reads

The portal catalog is loaded from the tables once per process into memory
(`routes/portal_catalog.rs`); the rest are read per request. `game_categories` merges
base, then local, then external, later sources winning.

## Not migrated

`admins.json` and `site.json` (untracked, per-machine, read directly from disk by several crates).
See `docs/json-blobs-remaining.md` for the legacy blob documents.
