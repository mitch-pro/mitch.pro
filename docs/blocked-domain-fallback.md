# Blocked-domain fallback

When a school filter blocks one of the site's domains, a browser that has
visited before is sent to the other domain instead of the offline page.

## How it works

- `data/site.json` has `primary` and `backup` (an older file without `backup`
  falls back to `alternate`). `GET /api/site-info` returns both.
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
- `rjuhsd.school` shares the worker but is a separate site, so it never fails
  over.

## Limits

- Only browsers that loaded the site after this shipped have the worker and the
  cached domain. A first visit to a blocked domain cannot be helped.
- The two domains must both be real hosts of this site; changing the domain
  pair means editing `data/site.json` and waiting for the cache refresh.

## Why there is no automatic login

Session cookies are per-origin, and the old origin is unreachable when the
redirect happens, so a session cannot be handed over at that moment. Doing it
would mean pre-minting a credential on the old domain and keeping it in the
browser until a block, where any script on the page (or anyone with the
device) could use it. The visitor signs in again on the new domain with the
normal flow, including two-factor codes.
