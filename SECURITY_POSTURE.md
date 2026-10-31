# Security Posture

Last updated: 2026-09-06

This is the public security note for the open-source tree. Detailed exploit writeups are not published here.

## Controls in this release

- **Session auth:** HttpOnly `mitch_session` is the only authentication bearer. Visible `studentId` is display-only.
- **CSRF:** Mutating `/api/*` requires same-origin `Origin`/`Referer` and `X-Mitch-Requested-With: 1` (see `webserver/api.js`).
- **Client IP:** Caddy sets `X-Mitch-Client-IP` from Cloudflare connecting IP or the peer address; client-supplied copies are stripped.
- **Proxies:** Arbitrary-site `/prox/` and ultra/bare stacks are removed. Fixed game host reverse proxies (e.g. `/proxy/gamemonetize/`) remain.
- **Admin SSH:** The web SSH terminal and its `ssh-gateway` container are removed.
- **Admin simulation:** Removed.
- **Webhooks:** `/api/sms-reply` requires `SMS_WEBHOOK_SECRET`.
- **Headers:** Caddy sets `nosniff`, framing controls, referrer policy, and a pragmatic CSP (inline scripts still used by many pages).
- **Secrets:** Runtime credentials live in Doppler/env; not in git.

## Reporting

See [SECURITY.md](SECURITY.md).

## Operator checklist before production

1. `SESSION_COOKIE_SECURE=1` and `NODE_ENV=production`
2. `ENABLE_ADMIN_KEY_HEADER=0`, `ENABLE_CLIENT_CHESS_REWARDS=0`
3. Set `SMS_WEBHOOK_SECRET` if SMS inbound is enabled
4. Confirm Caddy/Cloudflare IP header path matches your edge topology
5. If publishing publicly, start from a history cut that excludes old secret-bearing commits
