# Movies (`/proxy/movies/`)

The signed-in homepage has four equal shortcuts in `webserver/index.html`: Cloud Linux, Matrix chat, Play games, and Movies. Movies opens `/proxy/movies/`, which serves [movy.sx](https://www.movy.sx/) through `rust/crates/mitch-server/src/routes/movies.rs`. The old `/proxy/rivestream/` links redirect there.

## What goes through the server

| Request | Upstream | Methods |
| --- | --- | --- |
| `/proxy/movies/<path>` | `https://www.movy.sx/<path>` | GET, HEAD |
| `/proxy/movies/__api/<host>/<path>` | `https://<host>/<path>` | GET, HEAD, POST |

`<host>` must be one of `backend.movy.sx`, `db.wecollege.net`, `api.wecollege.net`, `trailers.wecollege.net` or `ipwho.is` (the hosts the movy.sx app calls from the browser; `ipwho.is` is its country lookup, so it sees the VPN exit instead of each visitor). Anything else is a 404, so this is never an open proxy. No Mitch cookies or Authorization headers go upstream; upstream cookies, CSP and frame headers are dropped. Redirects are only followed within the same host. Third-party scripts the site tries to load (ad networks and similar) are blocked by the page's CSP, which only allows `'self'`.

Not proxied: video segments and subtitles. The player pulls them from CDNs whose hosts are only known at playback time, so they load directly in the visitor's browser and don't use the VPN interface.

## VPN interface (eth1)

Every upstream connection is bound to one interface (`SO_BINDTODEVICE`), set with `MOVIES_EGRESS_INTERFACE` in `.env` (default `eth1`). Everything else the container does keeps using its normal interface.

- It **fails closed**: if the interface doesn't exist in the container, Movies returns 503 "Movies are unavailable right now" and never falls back to the normal connection.
- The interface needs its own route to the internet (a default route, which can have a higher metric than eth0). Binding only forces the interface; it doesn't create a route.
- The process runs as a non-root user, so binding needs Linux 5.7+ or `CAP_NET_RAW`.
- DNS lookups use the container's normal resolver. Only the connections are bound.
- A failed connection is logged as `[movies] request to <host> failed: ...` on the server's stderr (host and error only).

## Cloudflare

movy.sx and the APIs sit behind Cloudflare. `api.wecollege.net` (the stream-source lookup the Play button calls) challenges non-browser requests: from a datacenter or VPN address it can answer 403 "Attention Required" to the server. If pages load but nothing plays, check the logs and the browser's network tab for `403 /proxy/movies/__api/api.wecollege.net/seed`. The fix is `MOVIES_DIRECT_API_HOSTS=api.wecollege.net` in `.env`, which makes the visitor's browser call that host itself (that call then doesn't use the VPN interface). Other hosts are unaffected.

## How the site is adapted

movy.sx is a Next.js (pages router, Turbopack) app built for the root of its own origin. Under `/proxy/movies/` the proxy:

- rewrites root-relative `src`, `href`, `action`, `poster`, `srcset`, `imageSrcSet` and CSS `url()` values and absolute `https://www.movy.sx/` links to the mount;
- sets a `<base>` tag and `__NEXT_DATA__.assetPrefix`;
- patches the compiled router: `this.basePath`, `addPathPrefix`/`pathHasPrefix` (`addBasePath`/`hasBasePath`) and `removeBasePath`, so page names such as `/movie/155` stay unchanged while URLs carry the mount;
- patches the Turbopack runtime's chunk base from `"/_next/"` to `"/proxy/movies/_next/"`. The runtime names every chunk by stripping that prefix from its `<script src>`; without the patch nothing matches and the app silently never starts;
- injects `movies.js` first in `<head>`. It wraps `fetch`, XHR, `sendBeacon`, `setAttribute` and the `src`/`href`/`srcset`/`poster`/`action` setters so URLs built at runtime (including calls to the four API hosts) go through the mount, and it stops the PWA service worker from registering. Mitch's own `/presence.js` and `/api/presence/*` are left alone.

If movy.sx ships a new build, the patterns in `rewrite_js` (and the runtime regex) are the first thing to check. Symptoms: pages render but links do full page loads, `window.next` is undefined in the page, or images stay blank (`data-gate-src` never replaced).

## Checks

Rust tests cover path resolution and the API allowlist, traversal refusal, redirect mapping, HTML/asset rewriting including the `srcset` double-prefix case, the router and runtime patches, interface-name validation, and the fail-closed behavior. Beyond that, verify in a browser at 320, 390, 768, 1024 and 1440px in both themes: the homepage, a movie page and client-side navigation inside `/proxy/movies/`, then Play. Run the usual formatting, strict Clippy and workspace tests before deploying.
