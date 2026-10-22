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

## Controller and dashboard

The main dashboard is a document plus `woodcreek.js` and `woodcreek.css`.
It sends mutations over authenticated HTTP with a 12-second acknowledgement
limit. A timed-out command is never retried automatically: the operation may
already have reached the service. Buttons are released on success, rejection,
network failure, and timeout. Only the edited settings are submitted; loading
or reconnecting does not write settings. Existing WebSocket command clients
remain supported. The WebSocket supplies shared state and logs, checks a
heartbeat, and reconnects with bounded backoff. State snapshots have an ephemeral
service ID and revision so an older WebSocket update cannot overwrite a newer
HTTP response. No new state or activity is written to storage.

The interface keeps Tylerium branding, saved account photos and cosmetics, and
the Clever Portal title/favicon. Join, start/pause, answer once, retry, stop, and
connection details are explicit controls. Each connection reports its actual
answering state, including waiting for an answer or an unsupported screen.
Diagnostics and screenshots are available in its Details dialog. Activity is
bounded and deduplicated in memory. Optional script files remain separate from
the core controller; known auto-answer scripts use the shared controller instead
of installing another answer daemon. This does not make arbitrary legacy scripts
compatible with every Blooket version.

### Joining

`blooket-connection.mjs` centralizes Game PIN/nickname selectors and form
submission. `blooket-join.mjs` owns the join stages. Each form submits once;
only an explicit nickname rejection permits another name. An uncertain response
after submitting a nickname is reported for manual retry instead of creating
another connection that could replace the first player. A fresh browser retry
is allowed only before nickname submission. Explicit host removals never cause
automatic rejoining.

A service-wide scheduler limits joins and retries to two concurrent operations,
400 ms apart. Cgroup process/memory admission checks and the existing 50-bot cap
remain in force. Reservation happens before asynchronous browser startup.
Blooket's own client resources can load; analytics, fonts, and media are skipped.
A cached verification session may be reused; launch no longer waits for a
preemptive clearance request. Navigation challenges retain the existing service
verification path. Failure releases the affected browser context. Stops cancel
pending work, including contexts created after cancellation.

### Answering

`blooket-controls.mjs` is the shared selector registry for observation and
interaction. `blooket-screen.mjs` reads visible controls and makes bounded,
read-only checks of matching React props, state, hooks, and question banks.
Rich question/answer HTML is normalized before exact matching. Question banks
are consulted only when the current component lacks a key. Ambiguous keys are
not reported as known answers. The snapshot contains only the visible question,
not the full question bank or arbitrary client state.

A question key identifies its document, live controls, wording, choices, and
available React question index. A repeated question with new controls is a new
occurrence. `blooket-answers.mjs` re-observes after a delay and checks the key
again in the browser before clicking. The click and last check share one browser
JavaScript task. Typing uses the visible input and a final question check before
submitting. Unknown multiple-choice keys can be guessed and do not affect known
answer accuracy; a typing question without a key waits.

`blooket-runner.mjs` serializes each connection's question, feedback, and
minigame processing. An AbortController interrupts a pending answer delay when
paused, stopped, retried, or disconnected. A dispatched answer is counted even
if pause arrives immediately afterward. Feedback takes precedence over retained
question nodes. Explicit continuation prompts work without a button element.
Only one action is sent for a persistent transition; changed controls or a new
question release that action lock. There are no blind coordinate/key bursts.
Ended games close their context, and closed pages become retryable failures.

### Verification and deployment boundary

Run `npm run test:blooket` for join, answer, scheduler, controller, and actual
Node service fixtures. After building the Rust server, run
`node tests/test_blooket_bridge_browser.mjs` and
`node tests/test_woodcreek_accounts.mjs`. These use isolated databases, loopback
services, intercepted player pages, and test accounts. They never join a real
game or send a production command. Coverage includes repeated questions, HTML,
hooks/banks, stale screens, immediate pause, unconfirmed joins, persistent
feedback, rejected commands, individual stop, responsive layout, and shared
accounts/CSRF. Live compatibility still requires a game hosted for validation;
public unauthenticated player requests may be rejected by Blooket.

A Mitch deployment updates the bridge, including the existing per-bot stop API's
POST allowlist. It does **not** publish `integrations/woodcreek` to the separate
Woodcreek container. That source must be copied to its existing application
build context and rebuilt separately, after checking for active sessions. Do
not change workflows or access the host without the specific approval required
by `AGENTS.md`. See `integrations/woodcreek/README.md` for the existing layout.
