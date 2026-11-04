# The portal and the school filter

How the game portal got to be reachable on a school network, what we learned
about what the filter reacts to, and what to keep doing.

Everything under "What we think the filter does" is inference from tests run at
school, not documented behaviour of the filter. The tests were: load a page,
watch whether and when it gets blocked. We cannot see the filter's side.

## What happened

The portal (`/game-portal/`) was blocked at school, sometimes after playing a
while and later at a fixed, repeatable time after the page loaded. Other pages
on the same site (`/privacy/`, most others) were never blocked.

Things we tried, in order, and what each told us:

| Step | Result |
| --- | --- |
| Rotating URL aliases (`/<daily uuid>/<section uuid>/…`, `docs/url-aliases.md`) | Hid the section name in the address. Not enough on its own. |
| Dropped the word "games" from titles, headings, labels and metadata ("Entertainment Portal") | Still blocked. |
| Drew every piece of page text on `<canvas>` (`ui/canvas-text.js`) | Still blocked, so it is not only reading the page's text. |
| Moved the 8 MB catalog off the web root and served it a page at a time (`/api/portal/*`) | Still blocked. This also removed an 8 MB download and a long parse, so the page loads much sooner. |
| Short `/prox/` proxy paths instead of `/proxy/` | Still blocked. |
| Wrote a second page from scratch (`/portal/`) | Blocked too, so it is not the old page's markup. |
| `/portal/?probe=i` (no thumbnails, titles shown) and `?probe=n` (thumbnails, titles hidden) | Both still blocked: hiding either channel alone is not enough. |
| `/privacy/` and most other pages | Not blocked. It is the portal's content, not the domain. |
| Thumbnail addresses made name-free (`/prox/i/<id>`) | **Worked.** |

## What we think the filter does

Before the last step, a game's name was in the page in two places: as title
text, and inside the address of its thumbnail (`/prox/luma/tiny-fishing/thumb.png`,
`/game-portal/icons/<name>.png`). Hiding one without the other left the other,
which is why `?probe=i` and `?probe=n` both still blocked. Making the images
name-free was the last change, and then it worked.

The most likely reading:

- The filter looks at more than the visible text. It also sees the addresses of
  the things the page loads (images and other requests), and matches game names
  in them.
- It does this some time after the page loads, once the page has pulled in its
  resources, which is why the block came at a repeatable moment and not
  instantly.
- Domain and page-title checks on their own did not trigger it (other pages on the
  same domain are fine; titles were already neutral).

We do not know which of the earlier changes also mattered. Each was in place when
the last one made it work. Before removing any of them, test again.

## What to keep doing

Treat any game name as something that must not appear in an address or in the
page:

- **No names in image addresses.** `/api/portal/*` gives each thumbnail as
  `/prox/i/<id>`; `portal_catalog::image_uri` maps the id back, and the request is
  served as if it had asked for the real address. Art on another host is left as
  is (rare).
- **No names in the DOM.** The main portal draws text on canvas
  (`webserver/game-portal/ui/canvas-text.js`) and uses neutral ids and classes
  (`item-grid`, not `game-grid`). Keep new attributes, ids and `data-*` values
  free of game names.
- **Page at a time.** The catalog is `data/portal_catalog.json`, outside the web
  root; the browser only gets the 48–60 titles on screen.
- **Short proxy names.** `/prox/gamemonetize/`, `/prox/gm-icon/`, `/prox/luma/`,
  `/prox/calculated2/` serve the same proxies as `/proxy/…`, and a page opened
  through one keeps that prefix in its links. Caddy treats `/prox/*` like
  `/proxy/*`.
- **Aliases hide the path.** `docs/url-aliases.md`.
- **Neutral wording.** "Entertainment Portal", "titles", "Saved", not "games".

## Known gaps

- `/api/portal/*` responses contain the titles. A filter that reads response
  bodies would see them. The fix would be to encode those responses and decode
  them in the page.
- Once a game is opened, its address is in the frame (for example
  `/prox/luma/run-3/`). That only matters if the block fires after pressing play.
- The `/game-portal/` path, and the tab title "Entertainment Portal — mitch.pro".
- The sitewide guest-preview dialog says "play games" after 60 seconds for signed
  out visitors. On the main portal it is drawn on canvas, but it is still visible.
- Accessibility: canvas text is not read by screen readers and cannot be searched
  or copied. Button labels on the main portal carry no titles.

## Finding what trips it next time

`/portal/` has `?probe=` switches (`portal.js`) for stripping the page down:
`i` no thumbnails, `n` hide titles, `l` no list or API calls (`?probe=inl` leaves
only the top bar, search and tabs). Load each variant on the school network and
note whether it blocks and after how many seconds. Compare against a plain page
such as `/privacy/`. Change one thing at a time; the order above shows how much
a single change can hide.

## Where things live

- `rust/crates/mitch-server/src/alias.rs`: rotating URL aliases and the headers
  Caddy cannot add for them.
- `rust/crates/mitch-server/src/routes/portal_catalog.rs`: catalog, paging,
  `/prox/i/<id>`.
- `rust/crates/mitch-server/src/routes/proxy.rs`: the `/prox/` and `/proxy/` game
  and icon proxies.
- `webserver/game-portal/` (main portal) and `webserver/portal/` (second page).
- `docs/game-discovery.md`: popularity order and the catalog API.
