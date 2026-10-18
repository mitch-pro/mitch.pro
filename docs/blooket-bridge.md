# Shared Blooket dashboard

`https://mitchdog.com/blooket-bot/` serves the Woodcreek dashboard through an
authenticated backend bridge. Older Blooket page aliases redirect to this path.
Homepage Blooket links and the app navigation open this dashboard.

The Rust server relays allowlisted HTTP routes and the `/socket` WebSocket to
`https://woodcreek.site/`. Both sites use Woodcreek's existing shared bot session;
this is one running service, not two independent copies of its bot state.
Commands and live updates travel in both directions. The existing legacy
`/api/blooket-bot/*` premium queue remains separate.

Mitch checks signed, non-revoked sessions and bans for every request, and
rechecks WebSocket authentication every 15 seconds. Mutating HTTP requests
require the site's CSRF header and trusted origin; WebSocket upgrades also
require a trusted origin. Mitch does not forward its cookies or Authorization
headers, and does not accept upstream cookies or redirects. URLs are restricted
to the configured service, paths and methods are allowlisted, command traffic
is limited per account, and responses have a size limit.

The dashboard reads `data-app-base` to route API requests and WebSockets at
either origin. Mounted HTML rewrites local asset links and loads Mitch's
presence tracker. Woodcreek's own public URL and subdomains keep their existing
routing. This integration does not share accounts or login cookies between
the domains.

`WOODCREEK_BACKEND_URL` optionally overrides the service origin for deployment
or isolated integration tests. It must be an HTTP(S) origin without credentials,
query, or fragment; production defaults to `https://woodcreek.site/`.

After building the Rust server, `node tests/test_blooket_bridge_browser.mjs`
starts an isolated local backend and temporary Mitch database. It verifies the
mounted page, HTTP and WebSocket commands, updates from both sites, CSRF and
origin checks, cookie isolation, path restrictions, and revoked sessions.
It never launches bots or sends commands to the production service.
