# Rotating URL aliases

Homepage links point at paths like `/<daily-uuid>/<section-uuid>/run-3/` instead
of `/games/run-3/`, so a school URL filter never sees the same path twice.

## Format

| URL | Serves |
| --- | --- |
| `/<root>/<section>/rest` | `/<section name>/rest` |
| `/<root>/<real path>` | `/<real path>` |
| `/<root>` | `/` |

- `root` = UUID for the UTC day: `HMAC-SHA256(id_secret, "alias-root:<day>")`.
- `section` = UUID for one section on that day:
  `HMAC-SHA256(id_secret, "alias-section:<day>:<name>")`. A section UUID only
  resolves under the root of the same day. A section is a top-level webroot
  directory, `proxy` / `proxy/movies`, or any local link listed in
  `data/sites` cut to its directory (`games/adrian-clicker`, up to three
  segments). The homepage picks the longest matching section, so
  `/proxy/movies/` becomes `/<root>/<movies uuid>/`.
- Both are formatted as version-4 UUIDs. Nothing is stored in the database.
- A day's UUIDs work for 365 days (`alias::VALID_DAYS`), then stop. The real
  paths always keep working.

The part after the section is kept as-is on purpose: pages and games use
relative URLs, so those keep resolving under the prefix.

## Where it lives

- `rust/crates/mitch-server/src/alias.rs`: token derivation, `resolve`, tests.
- `handler::handle` rewrites the request URI before anything else, so bans,
  auth, CSRF and routing all run on the real path.
- `GET /api/alias` (public, `no-store`) returns today's `root` and a
  `sections` map. `webserver/index.html` fetches it and rewrites its links;
  recent visits are stored with the real path.

## Limits

Aliases are not access control. Anyone can fetch `/api/alias`, and revoking
them means changing `id_secret`, which also signs sessions. Tails such as a
game name or asset file are still visible to a filter, and absolute links
inside other pages go to the real path.

The Movies page is a proxied single-page app. `movies.js` notices when the
page was opened through an alias and sets `window.__mitchMoviesBase`; the
router's default basename is that prefix instead of `/proxy/movies`, so the
address bar keeps the alias. Matrix chat is never aliased: it needs
`/matrix/`, so aliased Matrix URLs redirect there.

What else the portal does about school filters: `docs/portal-and-school-filters.md`.
