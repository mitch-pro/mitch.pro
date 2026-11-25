# Administration

There is no `/admin/` site any more. Everything lives in the staff dashboard at
`/staff-dashboard/` (and `/moderator/`, the same app; see
[staff-dashboard.md](staff-dashboard.md)). `/admin`, `/admin.html` and
`/admin/<page>` redirect to `/staff-dashboard/<page>` so old links keep working.

Tool pages that stay separate live at `/staff-dashboard/<tool>/`: `bans`, `vms`
(and `vms/fleet`, owner only), `email`, `updates`, `companion`,
`special-schedule` and `passphrase-reset`. The server checks authentication and
staff roles for each of them (`STAFF_TOOLS` in `handler.rs`); VM, ban, email,
update and passphrase-reset pages require full administrator access, while
`companion` and `special-schedule` are open to staff the API allows. The dashboard
itself checks the viewer's role and only builds the tabs and tools that viewer can
use. Static CSS/JS contain no privileged data. API authorization remains
independent of page access.

Administrative API requests use the signed account session, full role checks,
and proof of the admin passphrase. A successful page load does not grant API
access or bypass the passphrase gate. The global CSRF gate rejects foreign
origins on mutations; clients send `X-Mitch-Requested-With: 1`.

## Unlocking once, re-entering for large actions

Full admins enter the passphrase once. `POST /api/admin/unlock {passphrase}`
verifies it and sets the HttpOnly `mitch_admin_unlock` cookie (`<expiry>.<mac>`,
8 hours, stateless: the MAC binds the session id, the current passphrase hash
and the expiry, so logging out or changing the passphrase revokes it).
`POST /api/admin/lock` clears it. `GET /api/admin/passphrase-status` also
reports `unlocked` and `unlockExpiresAt`. The cookie is shared by every tab, so
all staff pages are unlocked together.

`admin_gate` (`routes/admin/mod.rs`) accepts a request when either the
`X-Admin-Passphrase` header verifies or the unlock cookie is valid. Actions
listed in `mitch_lib::admin::is_sensitive_admin_action` (bans, role changes,
passphrase changes, VM deletion and power control, coin and premium changes,
broadcasts, newsletter sends, maintenance mode, ...) ignore the cookie: without
the real passphrase in the header they answer `403 reauth_required`. Add new
destructive endpoints to that list.

`webserver/admin-auth.js` is the single client piece. Load it first in `<head>`
of every staff page. It
- turns `reauth_required` / `invalid_passphrase` responses into a passphrase
  dialog and retries the request once (the passphrase is never stored);
- exposes `MitchAdminAuth.remember(pass)`, which page scripts call after
  verifying a passphrase: it trades it for the unlock cookie, keeps only a
  placeholder in `sessionStorage['admin_passphrase']`, and sets a
  `localStorage` marker so a new tab skips its lock screen. Never write the
  passphrase to storage directly; a `sessionStorage.setItem` override routes
  any such write through `remember()` as a backstop.

So a tool's frontend needs no passphrase code of its own: call the API and let
`admin-auth.js` prompt when needed. Moderators are exempt from the passphrase,
and a local dev-superuser session on localhost bypasses it, which can mask a
missing `admin-auth.js` include during testing.

## Forgotten passphrase

An admin who is signed in but has forgotten their passphrase uses
`/staff-dashboard/passphrase-reset/` (every passphrase prompt links there; the link comes
from `/staff-dashboard/passphrase-help.js`). It emails a one-time code to the account's own
address, then sets a new passphrase when the code is entered.

- `POST /api/admin/passphrase-reset/request` sends the code. `POST
  /api/admin/passphrase-reset/confirm` takes `{code, passphrase}` (8+ characters).
  Both need an admin session and the usual CSRF header, but not the passphrase
  header, so `handler.rs` exempts them from the passphrase gate.
- Codes are 8 characters (about 40 bits), valid for 15 minutes, single use, locked
  after 5 wrong tries, and a new one can be requested once a minute. Only a keyed
  hash is stored, in the SQL table `admin_passphrase_resets` (one row per admin).
  The rules live in `mitch_lib::passphrase_reset`.
- Requests and resets are recorded in the admin log, and the admin gets an email
  after a reset. The code goes to the address on the account, never one supplied
  in the request.
- The passphrase itself is still stored in `admin_passphrase.json` (see
  `json-blobs-remaining.md`).

The special bell schedule uses `/staff-dashboard/special-schedule/`, open to moderators as well as
admins; see [bell-overrides.md](bell-overrides.md).

Account bans use `/staff-dashboard/bans/`: searchable records, a visible reason, explicit
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

`rust/crates/mitch-server/tests/staff_access.rs` starts a local server and checks
the owner/co-owner/admin/member gates on the VM owner-resources and account-list
endpoints. It does not contact production or invoke delivery endpoints.

## Quiet deployment

A push whose head commit includes `[quiet]` skips the AI push-summary notification
workflow. The normal production workflow waits for the complete Rust CI
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
  log. Both the Moderation tab and the Overview queue of `/moderator/` and `/staff-dashboard/` provide one-click approval
  and denial queues for moderators and administrators.
  The same gate (`outside_account_gate`) covers joins, `createRoom` and invites and
  fails closed if the sender can't be identified. Matrix SSO login joins accounts
  to the official rooms server-side (bypassing the gateway), so it skips the join
  for uncleared outside accounts (`outside_account_cleared`); approval then joins
  them (`join_approved_outside_account`). Profile sync also applies `name_blocked`
  before writing a display name, since that write skips the gateway filter.
- **No chat client or SSO before approval.** The official rooms are
  `world_readable`, so any Matrix token can read them without joining. An
  unapproved outside account therefore never gets one: `/api/matrix/sso-login`
  returns 403 (`needsApproval: true`), `/api/matrix/sso-status` reports it as
  signed out, and `/matrix/` (the client and its routes, not its static assets)
  serves a "Chat needs approval" page linking to `/support/`, or a "not
  approved" message for a denied account. Visiting either queues the account as
  `pending` (`matrix_cookie_access` in `routes/matrix.rs`). Students, staff,
  approved accounts and signed-out visitors are unaffected.
- **No Matrix registration.** mitch.pro creates every Matrix account itself (SSO
  login calls Conduit directly, not through the public gateway), so the gateway
  refuses all `/_matrix/client/*/register` endpoints with 403, including guest
  registration. Without this, anyone could register on the homeserver and read the
  `world_readable` official rooms without a mitch.pro account.
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

Owner > developer (stored as `coOwners` in `data/admins.json`; "Root (in an LXC)" is stored as `rootLxc`, has identical developer permissions, and only differs in the displayed role) > admin >
moderator. Developers and admins are otherwise equal, with these additions for
developers:

- Add and remove admins (`/api/admin/admins`); only the owner may change an
  owner or another developer, and only the owner manages developers
  (`/api/admin/co-owners`).
- View System Logs (see [system-logs.md](system-logs.md)).
- Toggle themselves as a tester (`/api/admin/tester-self`).
- Use the VM owner-customization limits (`can_customize_owner_vm`).

## Changing an account's email

`rename_email_references` (`routes/me/account.rs`) moves an account's data to
its new email but does **not** touch the staff lists: `data/admins.json`
(`owners`, `coOwners`, `admins`), the `moderators` table, `data/testers.json`,
or `virtual_machines.owner_email`. Staff status is keyed by email, so changing a
staff member's email drops their role and detaches their computers until the
lists are updated to the new address. This is deliberate for now: migrating
roles automatically would let a hijacked session move an owner or admin role to
an attacker's address. Update the lists first, then change the email. Regular accounts are handled by the support portal, see [support.md](support.md). See also
[reserved-usernames.md](reserved-usernames.md).
