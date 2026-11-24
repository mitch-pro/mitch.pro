# Development mode

Two environment flags for running the server on a development machine.

## `MITCH_STATIC_LIVE=1`: no frontend snapshot

By default the frontend is read into memory once at startup and never re-read
([static-files.md](static-files.md)). With `MITCH_STATIC_LIVE=1` nothing is loaded:
every file is read from disk on every request, and the cache of unchanged files
re-checks the file's modification time each time, so an edit shows up on the next
reload without restarting the server. Meant for development only; production keeps
the snapshot.

## `MITCH_DEV=1`: everything a dev run wants

Fills in these settings, each **only if it is not already set**, so you can still
override any of them (`MITCH_DEV=1 PORT=9000 cargo run -p mitch-server`):

| Setting | Dev value | Why |
| --- | --- | --- |
| `MITCH_STATIC_LIVE` | `1` | edits show up on reload |
| `GOD_MODE` | `1` | every visitor is signed in as the dev admin |
| `NODE_ENV` | `development` | |
| `PORT` | `8080` | the documented local port |
| `LISTEN_HOST` | `127.0.0.1` | the server (default: all interfaces) listens on loopback only, so God mode isn't reachable from the network. Set `LISTEN_HOST=0.0.0.0` to test from a phone, knowing what that exposes |
| `SESSION_COOKIE_SECURE` | `0` | cookies work over plain http |
| `RECAPTCHA_SITE_KEY`, `RECAPTCHA_SECRET_KEY` | empty | no captcha |
| `MOVIES_EGRESS_INTERFACE` | `direct` | Movies traffic uses the system's default route instead of the production VPN interface (`eth1`) |
| `PROXMOX_VM_BRIDGE` | `vmbr0` | Proxmox's default bridge. Production deliberately has no default, so a guest can't land on the wrong network |
| `MITCH_DEV_NO_MAIL` | `1` | the email digest workers (weekly digest, chess clock warnings, DM digest) do not run, so a dev server holding a copy of real data can't email anyone |

On startup it prints a banner listing what is on. **`MITCH_DEV=1` refuses to start
when `NODE_ENV=production`**, the same guard as `GOD_MODE`.

The Movies egress sidecar (`egress-proxy/proxy.py`, Docker) takes the same idea:
`EGRESS_INTERFACE=default` (or `direct`) skips binding to a device and uses the
system's default route. With compose: `MOVIES_EGRESS_INTERFACE=default docker compose up`.

Not changed by dev mode: outbound services that need real credentials (Spotify,
YouTube, push, Matrix) still use whatever is configured, and the in-process
workers other than the email ones still run.

Tests: `dev_mode.rs` (the defaults and that an explicit value wins) and
`tests/frontend_snapshot.rs` (live mode).
