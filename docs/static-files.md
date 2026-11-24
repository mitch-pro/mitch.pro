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
- **What is held in memory**: code files (html, js, css, json, svg, wasm...) up to
  32 MB each, then other files up to 2 MB each, smallest first, until
  `MITCH_STATIC_SNAPSHOT_MB` (default **512**) is used. Everything else (large images,
  audio, video, game archives) stays on disk and is read when asked. That is safe: those
  files don't change what the backend expects, and their names don't get reused. The
  budget is small on purpose, because the old and the new container both run during a
  deploy and each holds its own copy. Startup logs one line with how many files are in
  memory, how many stay on disk and how long loading took (it reads on 8 threads).
  The first version loaded everything, including the game folders (32,900 files, 4 GB on
  production), which took longer than the deploy health-check window on a cold disk, so
  the first deploy attempt failed and the retry (files now cached) passed.
- **Game folders are skipped**: the folders inside `games/` (`games/<game>/...`, the third-party bundles that can be tens of thousands of files and several GB on the server) are never held in memory. They are read from disk when asked, as they always were. Files directly in `games/` (the library page) and everything in `game-portal/` are part of the frontend and are held.
- **Live folders** are the exception: `backgrounds/` and `blog/uploads/` are written
  by the server and admins while it runs, so they are read from disk as they change
  (`LIVE_DIRS`).

## Development: reading from disk

For development there is a switch that turns the snapshot off, `MITCH_STATIC_LIVE=1`
(or the all-in-one `MITCH_DEV=1`, which includes it): every file is read from disk on
every request, so edits show up on reload. See [dev-mode.md](dev-mode.md).

## The one deliberate reload

`POST /api/cache/refresh` rebuilds the snapshot from disk and empties the live-folder
cache. `tools/deploy.sh` calls it for a static-only update (its fast path: no backend
change, so no new container) and after a rollback. Nothing else re-reads the files. Do
not call it during a deploy that also changes the backend.

Tests: `crates/mitch-server/tests/frontend_snapshot.rs`.
