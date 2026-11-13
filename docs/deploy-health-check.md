# Deploy health check and git rollback

`tools/deploy.sh` builds the inactive blue/green slot, waits for it to answer, then
switches Caddy to it. This covers the wait (step 4) and what happens when it fails.

## Health check

- The new slot is probed with `GET /enroll/` from inside the still-serving slot (the path
  Caddy will use), every 2 seconds, for up to **60 seconds** of wall-clock time
  (`HEALTH_WINDOW_SECS`, default 60; the env override is only for tests).
- If the window expires it **retries once**: `docker compose up -d webserver-<slot>`
  (starts the slot if it exited; nothing is rebuilt) and waits a second 60-second window.

## When both attempts fail

1. The failed slot is stopped, so a reboot cannot bring the bad build back up.
2. The git checkout is moved back to the commit it was on before this deploy's pull
   (`git reset --keep`). The webroot HTML on disk then matches the backend that is still
   serving, and a reboot starts the old code. `--keep` refuses rather than discarding local
   edits made on the server; if it refuses, the owner gets an urgent ntfy and the checkout
   stays on the new commit.
3. `caddy/Caddyfile` is pointed back at the live slot, the static cache is refreshed on the
   running containers, and the owner gets a "Deploy Rolled Back" notification.
4. The script exits 1. The live slot was never touched, so traffic is unaffected.

If the deploy did not move the checkout (no new commit), there is nothing to roll back.

## Limits

- Only the webserver slot and the git checkout are protected. `conduit`, `livekit`,
  `mail-rs` and `sql-sync` are rebuilt before the health check and are not rolled back.
- A failure after the Caddy reload (the warm-up check in step 7) is not covered: it still
  aborts without rolling back.

## Tests

`bash tests/test_deploy_rollback.sh` runs the real script against stand-in `docker`/`curl`
commands and a throwaway repo: never healthy (retry, stop, rollback, exit 1), healthy on
the retry, and healthy at once.
