# Serving the frontend

The site's own files (`webserver/`) are read into memory **once, when the server
starts** (`static_files::webfs::init`, called from `main`), and every page and asset
is served from that snapshot for the life of the process. Nothing re-reads them from
disk, not on a timer and not on a cache miss. Why: `webserver/` is bind-mounted from
the host checkout, so a deploy updates the files under the old container while it is
still serving. A server that re-read the disk would then serve the new frontend
against the old backend until traffic switched. Now the old container keeps serving
the frontend it started with, and the new container starts with the new one.

What this means in practice:

- A file added, changed or deleted on disk after startup does not affect the running
  server. A new file answers 404 until the server restarts; a deleted file keeps being
  served.
- Everything that asks "does this page exist?" (directory redirects, `index.html`
  lookups, the `.html` to folder redirect, the matrix SPA fallback, the daily alias
  section list) is answered from the snapshot too, so it cannot disagree with the
  files that are served.
- Loading takes well under a second for the current ~230 MB. Code files (html, js,
  css, json) load first, then smaller files before bigger ones, until
  `MITCH_STATIC_CACHE_MB` (default 4096) is used. A file over 256 MB, or past the
  budget, stays on disk and is read when asked (logged at startup).
- **Live folders** are the exception: `backgrounds/` and `blog/uploads/` are written
  by the server and admins while it runs, so they are read from disk as they change
  (`LIVE_DIRS`).

## The one deliberate reload

`POST /api/cache/refresh` rebuilds the snapshot from disk and empties the live-folder
cache. `tools/deploy.sh` calls it for a static-only update (its fast path: no backend
change, so no new container) and after a rollback. Nothing else re-reads the files. Do
not call it during a deploy that also changes the backend.

Tests: `crates/mitch-server/tests/frontend_snapshot.rs`.
