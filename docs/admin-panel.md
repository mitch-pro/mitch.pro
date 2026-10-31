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

Every admin tool's frontend is responsible for wiring this itself — the
server-side gate in `handler.rs` enforces it regardless, but a tool that
never sends `X-Admin-Passphrase` just fails every write with
`invalid_passphrase` for any full admin (moderators are exempt, and a
local dev-superuser session on localhost bypasses it, both of which can
mask a missing frontend wire-up during testing). `/admin/bans/`'s
`bans.js` is the reference implementation: check
`GET /api/admin/passphrase-status` on load, show a lock screen if unset
or not yet entered this session, cache the entered passphrase in
`sessionStorage`, send it as `X-Admin-Passphrase` on every request, and
re-show the lock screen on any `invalid_passphrase`/
`passphrase_not_configured` response. `/admin/companion/` (see
`docs/companion.md`) follows the same pattern.

Account bans use `/admin/bans/`: searchable records, a visible reason, explicit
duration, confirmation, expiry, restoration, and chat-cleanup status. Usernames
and registered addresses resolve through the existing account directory. VM and
BYO-OS restrictions are separate from account bans. Banning an account also runs
`purge_banned_matrix_user_all_rooms` in the background: every Matrix identity of
the account is banned from the official and tracked rooms (removing it from member
lists) and its messages there are redacted by paging each room's history with the
system admin token (up to 200 pages of 100 events per room). Private rooms the
server doesn't track are not reached, redactions are permanent, and restoring the
account only lifts the room bans. New clients submit `banned`
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
workflow. The normal production workflow waits for the complete Rust/browser CI
workflow for the same revision to succeed before it can deploy. It uses a commit-pinned copy of
`tools/deploy.sh` with `MITCH_DEPLOY_QUIET=1` as the deployment account for that rollout.
The account must already have Docker access; the quiet path never invokes an
arbitrary privileged shell or prompts for a sudo password. This bootstrap is
necessary because the installed script may predate quiet support. Its usual
locking, blue/green deployment and health checks still run. Quiet mode suppresses
only deployment notifications; it does not change application email settings or
send newsletter jobs. Never print deploy secrets or call production delivery
endpoints as a health check.

## Matrix: outside accounts and names

- **Approval.** An *outside* account is any account whose normalized email isn't
  `@student.rjuhsd.us` and that isn't staff. Until a moderator (or admin) approves
  it, the Matrix gateway rejects its chat sends (messages, reactions, stickers)
  with 403; the first blocked attempt queues it as `pending` in the
  `matrix_outside_approvals` SQL table. `GET /api/admin/matrix/outside-accounts`
  lists the pending queue and `POST` with `{email, approved}` approves or denies;
  both require moderator or higher and the decision is written to the admin action
  log. Both `/admin/#approval-tools` and `/moderator/` provide one-click approval
  and denial queues for moderators and administrators.
  The same gate (`outside_account_gate`) covers joins, `createRoom` and invites and
  fails closed if the sender can't be identified. Matrix SSO login joins accounts
  to the official rooms server-side (bypassing the gateway), so it skips the join
  for uncleared outside accounts (`outside_account_cleared`); approval then joins
  them (`join_approved_outside_account`). Profile sync also applies `name_blocked`
  before writing a display name, since that write skips the gateway filter.
- **No DMs to students.** Outside accounts, approved or not, can't `createRoom`
  with, or `invite`, a student account (the gateway resolves the target Matrix ID
  to an email and checks the domain). Staff are exempt.
- **Names.** Profile saves (username, display name, nickname) and Matrix display
  name changes are rejected when `mitch_lib::matrix::name_blocked` matches: any
  chat-blocked word, or a severe slur anywhere inside the name after folding case,
  separators and common character swaps. The ban purge also redacts every
  `m.room.member` event about the banned user (their join and the "admin banned X"
  notice), which removes their name and the ban reason from the timeline. A new
  Matrix account never gets a blocked username as its localpart (it falls back to
  `user_<id>`). `POST /api/admin/matrix/purge-user {target}` (moderator or higher;
  `target` is an email or a Matrix ID) re-runs the purge for an account that is
  already banned, without unbanning.
- **Hidden membership lines.** Redaction can't remove a name that is the Matrix
  user ID itself (IDs are permanent, so a redacted join line still renders the
  ID). Instead the gateway drops `m.room.member` events showing a blocked name
  (user ID localpart, display name, or previous display name) from every
  successful GET response (`/sync`, `/messages`, `/members`, ...) via
  `mitch_lib::matrix::strip_blocked_name_events`, so no client we serve shows
  them. Responses that can't contain one aren't parsed. Clients that talk to the
  homeserver directly, bypassing the gateway, are not covered.

## Roles

Owner > developer (stored as `coOwners` in `data/admins.json`) > admin >
moderator. Developers and admins are otherwise equal, with these additions for
developers:

- Add and remove admins (`/api/admin/admins`); only the owner may change an
  owner or another developer, and only the owner manages developers
  (`/api/admin/co-owners`).
- View System Logs (see [system-logs.md](system-logs.md)).
- Toggle themselves as a tester (`/api/admin/tester-self`).
- Use the VM owner-customization limits (`can_customize_owner_vm`).
