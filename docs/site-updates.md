# Homepage update log

A short "Updates" list on the signed-in homepage, written by hand. Entries are
not generated from commit messages and are not tracked by git.

- **Storage:** the `site_updates` SQL table (`id`, `title`, `body`, `posted_at`,
  `updated_at`, `author`, indexed on `posted_at`). Rules and queries are in
  `mitch_lib::site_updates`: a title of 1–120 characters and details of up to
  2000.
- **Reading (public):** `GET /api/updates?limit=5` (max 20) returns
  `{updates: [{id, title, body, postedAt}]}`, newest first, cached for a minute.
  `webserver/index.html` (`loadUpdates`) shows the section only when there is at
  least one entry. It sits between "Continue where you left off" and Shortcuts as
  one quiet line (the newest title and date, no details); clicking it lists the
  recent entries with their details. Text is inserted with `textContent`, so entries are plain text
  (line breaks are kept).
- **Writing (admins):** `/admin/updates/` (linked from the admin page's
  shortcuts) lists every entry and posts, edits and deletes them through
  `GET/POST /api/admin/updates` and `POST /api/admin/updates/delete`
  (`routes/admin/updates.rs`). Full admins only: moderators are refused even
  though they pass the passphrase gate, and the page itself needs an admin
  session. The usual admin passphrase gate and CSRF header apply. Each post, edit
  and delete is recorded in the admin log.
- **Dates:** the date field is optional (default now); a given day is stored as
  noon UTC so it reads the same in every time zone.

## Automatic entries

When a holiday theme starts (`mitch_lib::seasons`, `docs/holiday-themes.md`), one
entry is posted from an editable template (`mitch_lib::site_updates::run_auto`,
checked every ten minutes by a worker in `workers_site.rs` against the Pacific
date). The entry is an ordinary row: author `automatic`, dated the day it was
posted, and editable or deletable like any other.

- **Templates:** `auto_update_templates` has one row per season (title, details,
  on/off), created with default wording on first use. Admins edit them under
  "Automatic entries" on `/admin/updates/` (`GET/POST /api/admin/updates/auto`).
- **Once per season and year:** `auto_update_posted` records each season/year that
  was handled. Deleting an automatic entry therefore does not bring it back that
  year, and a season whose template is switched off is recorded without posting.
- **Never retroactive:** the first time the worker runs (nothing recorded yet) it
  only records the season already under way and posts nothing. When this was
  introduced, Thanksgiving 2026 was already running, so it was skipped and posted
  by hand; the first automatic entry is Christmas 2026 (Nov 27).

