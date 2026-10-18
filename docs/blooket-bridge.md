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
require a trusted origin. Mitch sends its own scoped Woodcreek grant to the
backend, never browser cookies or incoming Authorization headers. It does not
accept upstream cookies or redirects. URLs are restricted
to the configured service, paths and methods are allowlisted, command traffic
is limited per account, and responses have a size limit.

The dashboard reads `data-app-base` to route API requests and WebSockets at
either origin. Mounted HTML rewrites local asset links and loads Mitch's
presence tracker. Woodcreek's public entry redirects to the Mitch mount;
Mitch attaches its scoped grant to upstream HTML requests to avoid a redirect
loop. Subdomains keep their existing routing. The main application is branded **Tylerium Services**, using the logo
and fonts from tylerium.pro, with no decorative external-link arrows.

## Shared accounts

Woodcreek's `/auth/login` redirects to Mitch's `/api/woodcreek/auth/authorize`.
Existing Mitch sessions continue immediately; otherwise `/enroll/?next=...`
returns to authorization after sign-in. A fixed Woodcreek callback receives a
90-second, one-use code. The Woodcreek backend exchanges it with an S256 PKCE
verifier held in an HttpOnly flow cookie. SQLite atomically consumes the code,
including across blue/green deployments. Callback state must match the flow
cookie, and return paths are restricted to local paths.

The seven-day service grant uses AES-256-GCM with a purpose-separated key.
Only Mitch can open it or use the enclosed session proof. Woodcreek validates
it through the dedicated Mitch session endpoint, which checks the original
session, account revocation, and bans, then returns the account's saved name
and photo. No duplicate user database or cross-domain login cookie is created.
Woodcreek's `__Host-tylerium_session` cookie is Secure, HttpOnly, SameSite=Lax,
and host-only; subdomains receive no account cookie.

Woodcreek requires an account for APIs and WebSockets. Cookie-authenticated
writes require its own origin and CSRF header; the Mitch bridge supplies a
scoped grant for HTTP and WebSocket connections. Session validation caches for
at most ten seconds, WebSockets recheck every fifteen seconds, and failures
close access. Woodcreek sign-out revokes its grant; Mitch logout invalidates the
original session and therefore all linked grants. The account menu reads the
existing Mitch profile, including the chosen photo, name color, badge, and profile effect.

`MITCH_AUTH_ORIGIN` overrides Woodcreek's account provider for local fixtures;
production defaults to `https://mitchdog.com`. Deploy the Mitch provider before
the Woodcreek authentication middleware. No new shared secret or writable
container directory is needed.

`WOODCREEK_BACKEND_URL` optionally overrides the service origin for deployment
or isolated integration tests. It must be an HTTP(S) origin without credentials,
query, or fragment; production defaults to `https://woodcreek.site/`.

After building the Rust server, `node tests/test_blooket_bridge_browser.mjs`
starts an isolated local backend and temporary Mitch database. It verifies the
mounted page, HTTP and WebSocket commands, updates from both sites, CSRF and
origin checks, cookie isolation, path restrictions, and revoked sessions.
It never launches bots or sends commands to the production service.

`node tests/test_woodcreek_accounts.mjs` exercises both backends with a real local
Mitch server in production authentication mode. It checks the profile, PKCE,
state, code persistence and concurrent consumption, cookies, CSRF, token
integrity, grant revocation, and linked Mitch logout without production data.

## Blooket connection updates

`integrations/woodcreek/backend/blooket-connection.mjs` centralizes the browser
join selectors and validates 6–9 digit Game PINs. It finds visible fields, submits
each form once, and distinguishes a full/ended game from a temporary connection
failure. Browser creation is shared across concurrent joins. Stops cancel queued
joins and late contexts; failed joins release their contexts; retries cannot
start a second lifecycle for the same connection. Browser disconnects preserve
failed entries so they can be retried. Blooket's own client resources are allowed
to load, and a refreshed navigation uses the new HTTP response status.

The dashboard reports command failures, avoids overlapping reconnects, and the
Node WebSocket server checks heartbeats. Unmeasured latency is no longer shown
as random data. `node tests/test_blooket_connection.mjs` uses real Chromium with
fully intercepted Blooket pages to exercise these cases without joining games.

The browser tab is titled Clever Portal and uses Clever's favicon. The visible
interface remains Tylerium Services. Server status controls are removed.

Joining now uses a service-wide scheduler (two simultaneous joins, 400ms
spacing) shared by launch and retry operations. Cgroup process/memory admission
checks stop fresh browser contexts from exhausting the container. The app's
process limit was raised live from 512 to 1024 after its process limit counter
reported 39 failures, without restarting the current session. The Compose app
limit persists that change. CPU and memory limits were not increased.

Answering starts off and is controlled by the visible Start answering / Stop
answering button. Each bot submits its own current question once, respects a
pause during its configured delay, and never performs blind mouse/keyboard
bursts on the next screen. The duplicate 40ms page daemon was removed. Feedback
advances once through its visible control. `node tests/test_blooket_answers.mjs`
checks answering and queue limits with local pages only.
