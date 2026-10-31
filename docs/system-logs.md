# System logs

The admin panel's **System Logs** section (`/admin/#logs-tools`) shows the
`app_logs` SQL table and the `docker compose logs` output. Only the owner and
developers can open it: `GET /api/admin/app-logs` and
`GET /api/admin/docker-logs` return 403 for admins and moderators, and the
section and its nav link are hidden for them
(`AdminCtx::is_owner_or_developer`).

## What gets logged

Everything the server writes through `tracing` (the console log) is also
written to `app_logs` by `mitch-server/src/app_log_layer.rs`:

- Events go into a channel and a dedicated writer thread inserts them, so
  logging never blocks a request. The writer reports its own failures with
  `eprintln!` so a failing write cannot feed back into the log.
- The row's `category` is the module path without the crate/`routes::` prefix
  (`mitch_server::routes::vm` -> `vm`); `level` is the tracing level; extra
  structured fields go into `details`.
- Code that used `println!`/`eprintln!` now uses `tracing::info!`/`warn!`, so
  it reaches both the console and the table. New server code should do the same;
  a bare `println!` only reaches the console.
- `mitch_lib::log::append_app_log` still works for rows with an explicit
  category.
- Filtering follows `RUST_LOG` (default `info,tower=warn,hyper=warn`), so debug
  events only appear if the server is started with a more verbose filter.

## Retention

Rows older than 30 days (`APP_LOG_RETENTION_MS`) are deleted at startup and on
every 250th write. `APP_LOG_MAX_ROWS` (1,000,000) is a backstop against
something logging in a tight loop.
