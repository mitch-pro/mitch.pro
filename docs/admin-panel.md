# Administration

`/admin` and `/admin.html` redirect to `/admin/`, preserving the query string.
The server checks authentication and staff roles for every admin HTML page,
including `index.html` aliases. VM, ban, and newsletter pages require full
administrator access; moderators retain the review workspace. Static CSS/JS
contain no privileged data. API authorization remains independent of page access.

The dashboard uses `webserver/admin/admin.css` over the base page controls,
replacing the old competing command-center styles. Its shortcuts cover
newsletters, account bans, virtual machines, and public rankings. Newsletter and
VM shortcuts stay hidden until `/api/me` confirms `isAdmin`. The newsletter API
also requires full admin status; manually opening a URL cannot bypass that check.
The newsletter's Mitch & Tyler signature is retained.

Administrative API requests use the signed account session, full role checks,
and `X-Admin-Passphrase`. Passphrases are verified on the server and kept only in
session storage on the client. The global CSRF gate rejects foreign origins on
mutations; clients send `X-Mitch-Requested-With: 1`. A successful page load does
not grant API access or bypass the passphrase gate.

Account bans use `/admin/bans/`: searchable records, a visible reason, explicit
duration, confirmation, expiry, restoration, and chat-cleanup status. Usernames
and registered addresses resolve through the existing account directory. VM and
BYO-OS restrictions are separate from account bans. New clients submit `banned`
explicitly, making retries idempotent. Historical toggle requests remain accepted
for old clients. Only full admins can change these restrictions, protected staff
accounts cannot be targeted, and failed storage writes leave the live set intact.

Shadow bans are retired: both legacy endpoints return 410, queued applications
cannot reactivate them, startup ignores historical flags, and canvas/chat no
longer suppress actions based on them. Historical records remain available for
audit cleanup. The VM usage leaderboard and background usage sampling were
removed; operational VM quota enforcement remains.

After building the Rust server, `node tests/test_admin_modernization.mjs` runs
local API and responsive browser regressions with a fake HTTPS Proxmox service.
It does not contact production or invoke delivery endpoints.

## Quiet deployment

A push whose head commit includes `[quiet]` skips the AI push-summary notification
workflow. The normal production workflow waits for the complete Rust/browser/Blooket CI
workflow for the same revision to succeed before it can deploy. It uses a commit-pinned copy of
`tools/deploy.sh` with `MITCH_DEPLOY_QUIET=1` for that rollout. This bootstrap is
necessary because the installed script may predate quiet support. Its usual
locking, blue/green deployment and health checks still run. Quiet mode suppresses
only deployment notifications; it does not change application email settings or
send newsletter jobs. Never print deploy secrets or call production delivery
endpoints as a health check.
