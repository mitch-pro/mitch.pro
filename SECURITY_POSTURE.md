# Security Posture

Last updated: 2026-11-18

This document summarizes the technical security controls enforced by the server
and deployment stack. For vulnerability reporting, see
[SECURITY.md](SECURITY.md). Detailed exploit write-ups are not published here.

## Controls

- **Authentication & sessions:** Passwords are hashed with Argon2id. The
  HttpOnly `mitch_session` cookie (32-byte random token, stored only as its
  SHA-256 hash in `auth_sessions`) is the sole session bearer; legacy
  client-visible identity cookies (`studentId`, `id`, `adminId`) are stripped
  and re-derived server-side. Accounts also support WebAuthn passkeys, TOTP
  2FA, and magic-link sign-in.
- **Staff & admin authorization:** Role checks (`owner`, `co-owner`, `admin`,
  `moderator`) are enforced server-side on every staff API route. Requests to
  `/api/admin/*` additionally require an Argon2id-verified `X-Admin-Passphrase`
  header (see [docs/admin-panel.md](docs/admin-panel.md)).
- **CSRF protection:** Mutating `/api/*` requests require a same-origin
  `Origin` or `Referer` header and `X-Mitch-Requested-With: 1` (attached by
  `webserver/api.js`), with a small explicit allowlist of non-cookie /
  Bearer-authenticated endpoints (`CSRF_EXEMPT_PATHS` in
  `rust/crates/mitch-server/src/state.rs`).
- **Rate limiting & abuse controls:** Sliding-window rate limits per IP and
  account run in memory alongside timing-based bot detection. Persistent IP
  bans (`banned_ips`) store only a keyed HMAC of the address, never raw IPs.
- **Encryption at rest:** User save files (`/api/userdata`) are encrypted on
  disk using AES-256-GCM with HKDF-SHA256 per-account keys derived from
  `DATA_ENCRYPTION_KEY`. The SQLite database itself is not encrypted (see
  [docs/encryption-at-rest.md](docs/encryption-at-rest.md)).
- **Cloud desktop isolation:** Proxmox console sessions require session
  authentication and ownership (or an explicit 24-hour owner consent grant for
  admins; owner-locked VMs never honor grants). Each connection uses a
  single-use 75-second handle proxied through `/api/vm/desktop/ws`; Proxmox API
  tokens and port `8006` are never exposed to browsers (see
  [docs/computer-portal.md](docs/computer-portal.md) and
  [docs/vm-access.md](docs/vm-access.md)).
- **Fixed-origin proxies only:** Arbitrary URL proxying is disabled. Only fixed
  allowlisted upstreams remain: game-host proxies under `/proxy/*`, the
  captcha relay, and movie streams routed through the host-network VPN egress
  sidecar (`egress-proxy/`, see [docs/movies.md](docs/movies.md)).
- **Privacy & minimal retention:** Live presence and browsing activity remain
  in memory only and are never written to disk or SQLite. Staff views do not
  expose raw IP addresses or browsing history (see
  [docs/privacy.md](docs/privacy.md)).
- **Client IP resolution:** Caddy sets `X-Mitch-Client-IP` from upstream nginx
  (`X-Mitch-Client-IP`, then `X-Real-IP`) or the immediate peer address and
  strips client-supplied copies. A `CF-Connecting-IP` fallback remains in
  `caddy/Caddyfile` from an earlier topology (see [Network path](#network-path)).
- **Security headers:** Caddy sets `X-Content-Type-Options: nosniff`,
  `Referrer-Policy: strict-origin-when-cross-origin`,
  `X-Frame-Options: SAMEORIGIN`, and a pragmatic CSP (inline scripts are still
  used by some pages), and strips the `Server` header. For rotating URL aliases
  (`/<uuid>/<uuid>/...`), the server sets these headers directly
  (`rust/crates/mitch-server/src/alias.rs`).
- **Secrets management:** Runtime credentials live in Infisical
  ([docs/secrets-providers.md](docs/secrets-providers.md)) or host environment
  variables and are never committed to git.

## Reporting

See [SECURITY.md](SECURITY.md).

## Operator Checklist Before Production

1. Set `NODE_ENV=production` and `SESSION_COOKIE_SECURE=1` (enforces `Secure`
   cookies and disables local-dev-only flags such as `GOD_MODE` and
   `DEV_TEST_ACCESS`).
2. Keep `GOD_MODE=0` and `DEV_TEST_ACCESS=0` (or unset).
3. Configure `DATA_ENCRYPTION_KEY` in Infisical, remove any bootstrap
   `data/encryption.key` file after verifying startup, and keep an offline
   backup of the key ([docs/encryption-at-rest.md](docs/encryption-at-rest.md)).
4. Set a strong `SECRET_KEY`, and replace the default `LIVEKIT_API_KEY` /
   `LIVEKIT_API_SECRET` values in `docker-compose.yml` before exposing LiveKit.
5. Scope the Proxmox API token (`PROXMOX_TOKEN_ID` / `PROXMOX_TOKEN_SECRET`) to
   the customer pool and templates only, keep `PROXMOX_VERIFY_TLS=true`, and
   keep Proxmox port `8006` private.
6. Confirm that upstream nginx strips untrusted `X-Mitch-Client-IP` /
   `X-Real-IP` headers and matches `caddy/Caddyfile`.
7. Verify Infisical CLI access on the host
   ([docs/secrets-providers.md](docs/secrets-providers.md)) before relying on
   automated deploys.
8. If publishing a fork or public mirror, verify git history excludes any
   secret-bearing commits.

## Network Path

Visitors reach an upstream nginx instance, which proxies to Caddy and then to
`mitch-server`. Cloudflare is used for **DNS only**: there is no Cloudflare
tunnel or proxy in front of the site, and Cloudflare never sees visitor
traffic. Any leftover header handling (`CF-Connecting-IP` in
`caddy/Caddyfile`) or bot-detection scripts in vendored pages are remnants from
an earlier setup and are not part of the active traffic path.
