# Email login link

One-click sign-in by email, alongside password and passkey on `/enroll/`.

- `POST /api/magic-link/request` `{email, recaptcha_token}`: mails a link. Always returns `{success:true}` for a valid request so account existence isn't revealed. Skipped silently for banned accounts and for accounts using an authenticator-app (TOTP) second factor, since the link would bypass it.
- The token is random, stored as a `type: "magic_login"` row in `tokens`, valid 15 minutes, single use (deleted on first redemption).
- The emailed link is `<origin>/magic-link/#<token>`. The token is in the fragment so it stays out of server logs, and the page redeems it with `POST /api/magic-link/verify` so mail scanners that prefetch links can't consume it.
- The link origin is the request host only if it is a known site host; otherwise the configured alternate origin (prevents Host-header poisoning).
- Rate limits live in `rate_limit_for` (`mitch-lib/src/auth.rs`); both paths are in `PUBLIC_API_PATHS`.
