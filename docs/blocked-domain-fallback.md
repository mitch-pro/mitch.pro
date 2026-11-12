# Blocked-domain fallback

When a school filter blocks one of the site's domains, a browser that has
visited before is sent to the other domain instead of the offline page.

## How it works

- Backup domains live in the SQL table `backup_domains` (module
  `mitch_lib::site_domains`): a pool of origins, at most one active. Only the
  active one is returned by `GET /api/site-info` (as `backup`, next to
  `primary`); the pool is visible to admins only. With no active entry,
  `backup` is `alternate` from `data/site.json`.
- `data/site.json` is not tracked by git (it is per-machine; see
  `data/site.json.example`). It supplies `primary`, `alternate` and `name`.
  When it is missing the defaults in `site_domains.rs` apply.
- Admins manage the pool in the "Backup domains" card on `/admin/` (full
  admins; owner and developer see the card): add, remove, make one active, or
  "Switch to a random one", which picks a random *other* entry. API:
  `GET|POST /api/admin/backup-domains` with `{action: add|remove|activate|rotate, origin}`;
  each change is written to the admin log. Origins must be plain `https://host`
  (no path, port or credentials).
- `webserver/sw.js` copies them into its own cache (`mitch-pro-domains`, kept
  across `CACHE_NAME` bumps) on activation and at most every 6 hours after a
  page load.
- If a page navigation fails on the network, the worker picks the stored
  domain that is not the current origin and probes `/favicon.ico` on it
  (no-cors, 5 s). If it answers, the navigation is redirected to the same path
  there with `?_moved_from=<old host>`. If not, the old "Connect to the
  internet" page is shown.
- `webserver/app-shell.js` sees `_moved_from`, removes it from the address bar
  and shows a dialog: the old domain was blocked, replace bookmarks, sign in
  again.
- **Soon to be blocked (owner only).** In the "Soon to be blocked" list of the
  same admin card, the owner can mark any listed domain (primary, secondary or
  a backup). Marks live in the SQL table `redirecting_domains` (API actions
  `redirect_on` / `redirect_off` on `/api/admin/backup-domains`, audited).
  `GET /api/site-info` then returns `redirectTo` for requests made on a marked
  domain: the active backup, else the other backups, the secondary, the
  primary, skipping the current host and any other marked one (`null`
  otherwise; the pool itself is never exposed). The redirect is client-side:
  `app-shell.js` asks `/api/pass` and only moves visitors with a valid session,
  sending them to the same path on the next domain with `?_moved_from=<old host>`.
  Signed-out visitors are not moved.
- The moved notice keeps the old host in `sessionStorage` until the visitor
  clicks "Got it". It has to: on a first visit to a new domain the page reloads
  once when the service worker takes control, and that reload no longer has
  `_moved_from` in its URL.
- `rjuhsd.school` shares the worker but is a separate site, so it never fails
  over.

## Limits

- Only browsers that loaded the site after this shipped have the worker and the
  cached domain. A first visit to a blocked domain cannot be helped.
- "Check which domains load on this device" (admin card) loads `/favicon.ico` as an image from the primary, secondary and every pool domain and reports each as loads, blocked or timed out; run it on the network you care about, e.g. a school Chromebook. A filter block page is not a valid image, so it counts as blocked.
- After a check, "Move blocked backups to the removed list" takes every backup that did not load out of the pool and into a plain "Removed domains" list (table `removed_domains`; API actions `retire` and `forget`). The list does nothing else; it's a record for deleting those domains at the registrar, and × deletes an entry. Only pool backups move, never the primary or secondary. If the active backup moves, none is active until an admin picks one (site-info then uses `alternate`).
- Every domain in the pool must serve this site. Browsers pick up a change on a
  page load after their cached copy is 6 hours old, so rotating after a block
  helps people who haven't loaded the site in the meantime only if the old
  active domain is still reachable for them.

## Why there is no automatic login

Session cookies are per-origin, and the old origin is unreachable when the
redirect happens, so a session cannot be handed over at that moment. Doing it
would mean pre-minting a credential on the old domain and keeping it in the
browser until a block, where any script on the page (or anyone with the
device) could use it. The visitor signs in again on the new domain with the
normal flow, including two-factor codes.
