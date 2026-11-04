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
  least one entry. Text is inserted with `textContent`, so entries are plain text
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
