# Staff dashboard

The dashboard is where moderators and admins start. It lists everything waiting
on staff in one place and handles most of it inline. It is served at
`/moderator/` (the moderator entry point, and where the homepage pill sends
moderators and admins) and at `/staff-dashboard/` (testers and `/staff/`, which
redirects there). Both pages share `webserver/staff-dashboard/` CSS and JS, so
they cannot drift. The sub-pages (`/moderator/support/`, `/feedback/`,
`/teachers/`) link back with "Dashboard".

Moderator shortcuts that admins set (`/api/admin/moderator-panel`) appear in the
Tools section. The old `/moderator/` page loaded the legacy `theme.js` and sent
approvals without the CSRF header, so they failed; it is gone.

## Queues

The **Needs attention** tab stacks every non-empty queue; each queue also has its
own tab with a count. Items older than 24 hours get an amber edge. The page
refreshes every minute and when the tab regains focus.

| Queue | Source | Inline actions |
| --- | --- | --- |
| Matrix chat reports | `matrix_reports` table | delete message, ban sender, resolve, dismiss (with note), reopen |
| Chat and DM reports | `chat_reports.json` (non-Matrix) | resolve, delete |
| Profile reports | `profile_reports.json` | resolve, dismiss |
| Chat access | `matrix_outside_approvals` | approve, deny |
| Support tickets | `support_tickets` (status `open`) | links to `/moderator/support/` |
| Email changes (admin) | `support_email_requests` (pending) | links to `/moderator/support/` |
| Feedback | `feedback_entries` (new) | mark handled |
| Teacher notes | `teacher_comments` (pending) | approve, reject |
| Moderator requests (admin) | `moderator_requests.json` | approve, reject |

Admin-only queues are omitted from the API response for moderators. Each action
calls that queue's existing endpoint, so its rules (role checks, audit log) are
unchanged. A **Tools** section links every other staff page, greyed out when the
viewer lacks the role. Admins unlock with their passphrase on first use, like
every `/api/admin` tool; the page re-prompts on `invalid_passphrase`.

## Matrix reports

Reports arrive from the Matrix client (`POST /_matrix/client/v3/rooms/{room}/report/{event}`,
intercepted in the gateway) and from `POST /api/matrix/report-room`. Both write to
`matrix_reports` (`mitch_lib::matrix_reports`): reporting the same message twice
keeps one row. The gateway only knows the room and event ids, so it looks the
message up with the system admin account in the background and stores the sender
and text. Staff-side listing retries that lookup for open reports still missing
them (best effort; the admin account must be in the room). Stored text is only
ever filled once, so a later deletion doesn't erase it.

"Delete message" calls `/api/matrix/moderation/redact` and "Ban sender" calls
`/api/matrix/moderation/ban`, then the report is resolved. Status changes are
written to the admin log as `matrix_report_status`.

Matrix reports that were already in `chat_reports.json` are moved into the table
the first time the summary, list, or Matrix moderation overview runs
(`migrate_legacy_matrix_reports`). The old admin page's chat report list no longer
shows Matrix reports.

## API

All require moderator or higher (and the passphrase for full admins).

- `GET /api/admin/staff-dashboard/summary`: counts, the viewer's role, and up to 50
  items per queue (the count is always the full number).
- `GET /api/admin/staff-dashboard/matrix-reports?filter=open|closed|all`
- `POST /api/admin/staff-dashboard/matrix-reports/status {id, status, note}` with
  status `open`, `resolved` or `dismissed`.

Code: `routes/admin/staff_dashboard.rs`, `mitch-lib/src/matrix_reports.rs`,
frontend in `webserver/staff-dashboard/`.

## Sections

Each tab is a module in `webserver/staff-dashboard/sections/` registered with
`SD.register({key, label, access, render})` (core: `sd-core.js`). A section is
only listed, and a tool inside it only built, when the viewer has the access level
(`mod`, `admin`, `dev` = owner or developer, `owner`). Hiding is a convenience;
every endpoint still enforces its own role server-side.

| Tab | Access | Contains |
| --- | --- | --- |
| Overview | staff | queues above, plus links to standalone pages |
| Moderation | mod | account lookup, ban/restore, VM and BYO-OS bans, chat access approvals, featured game, blog deletion log |
| Matrix chat | mod | Conduit health, slowmode, lockdown, mute/kick/ban/unban, delete message, staff roles and channels (admin), moderation log |
| People | mod | notifications, emergency popup, roles (moderators, administrators, developers, testers, blog contributors), moderator shortcuts, owner account registrations |
| Economy | mod (requests) / admin | wealth audit, give/set/burn/reset coins, multiplier, casino switch, premium; owner: reset all coins |
| Shop | admin | catalog overrides and custom cosmetics |
| Computers | admin | VM requests (approve, deny, delete, restore, purge); link to `/staff-dashboard/vms/` |
| Logs | admin | server load, admin activity with undo, sent notifications, bans, coin and anti-cheat logs; developer: application and Docker logs |
| Site | admin | public maintenance switch, domains and mirrors, reserved usernames, tester mode, admin passphrases |

**Moderators and approvals.** Moderators can do some things directly (bans,
notifications, chat access, report handling). For a set of economy, premium and
role actions they see the form but it sends a request to the admins instead
(`SD.post`, `/api/moderator/action-request`); admins approve it from the Overview
queue "Moderator requests". Everything else is hidden from them.

**Removed.** The old page's "service activity" live feed was already always empty
and is gone, in line with the rule that staff don't see what pages people visit.
