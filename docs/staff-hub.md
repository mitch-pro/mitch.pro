# Staff hub

`/staff/` — a plain directory of links to every staff-facing page (admin
dashboard and its sub-tools, the moderator panel, the beta tester hub),
linked from the homepage footer for admins/moderators/testers only. New
subsystem, no `server.js` equivalent.

## Why its own endpoint

`GET /api/staff-hub/status` returns `{isAdmin, isModerator, isTester,
isStaff}` for the caller. Deliberately not added to `/api/me` (a
`server.js`-parity-tracked response shape not worth risking for this) and
deliberately not relying on `/api/tester/status` — that endpoint is
referenced by `webserver/tester/index.html` but was never actually
implemented anywhere in this server; a pre-existing gap, not something
this change takes on fixing. `/staff/` still links to `/tester/`
(it's a real, existing staff page) even though that page's own backend is
currently broken.

## Gating

No server-side HTML gate on `/staff/` itself (same pattern `/moderator/`
and `/tester/` already use, as opposed to `/admin/*`'s hard gate in
`handler.rs`) — the page fetches its own status and shows "Staff access
required" if `isStaff` is false. The homepage footer link
(`webserver/index.html`, near `_agree_footer`) does the same check and
stays `hidden` unless `isStaff`.

Each linked card is shown locked (greyed out, no `href`) rather than
hidden entirely when the viewer doesn't have that specific role — e.g. a
tester who isn't an admin still sees the Admin dashboard card exists, just
can't open it. Hiding it outright would make the directory look
incomplete depending on who's looking at it.
