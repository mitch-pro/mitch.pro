# Movies (`/proxy/movies/`)

The signed-in homepage has four equal shortcuts in `webserver/index.html`: Cloud Linux, Matrix chat, Play games, and Movies. Movies opens `/proxy/movies/`, which serves [cno.to](https://cno.to/) through `rust/crates/mitch-server/src/routes/movies.rs`. The older `/proxy/rivestream/` links redirect there. (movy.sx was tried first and dropped: its stream-source API refuses requests from servers and VPN addresses. cno.to's pages, API and streams all work from the VPN.)

## What goes through the server

| Request | Upstream | Methods |
| --- | --- | --- |
| `/proxy/movies/<path>` | `https://cno.to/<path>` | GET, HEAD; POST under `/api/` |
| `/proxy/movies/__api/<host>/<path>` | `https://<host>/<path>` | GET, HEAD, POST |

`<host>` must be `v.cno.to` (the stream mirror the player races against the main site) or `api.theintrodb.org` (skip-intro data). Anything else is a 404, so this is never an open proxy. That includes the video: playlists (`/hls/...`) and the segment requests the player makes go through here and out the VPN.

Not proxied, loaded by the visitor's browser directly: poster art (TMDB), YouTube trailers, and image-CDN redirects (`/api/ytthumb/...` redirects to `i.ytimg.com`).

## How requests are made

Upstream requests use the `curl` binary (installed in the production image), not an in-process HTTP client. Cloudflare answers 403 to some clients on the stream endpoints; this combination is known to pass: HTTP/2, curl's TLS handshake, and a desktop-Chrome User-Agent. The proxy always sends its own fixed User-Agent (the visitor's is not forwarded: `node` and headless-browser agents get a 403, and there's no reason to pass a visitor's browser details upstream). Redirects are not followed server-side; a redirect on the same host is mapped back into the mount and the browser follows it. Response bodies that aren't rewritten (images, playlists, segments, JSON) are streamed, and the curl process is killed if the visitor goes away.

If Movies loads but something returns 403, the server log has `[movies] cno.to answered 403 (cf-ray ...)`. Reproduce with `curl --interface eth1 --http2 -A "<a Chrome UA>" https://cno.to/...` from inside the container.

## VPN interface (eth1)

Every upstream connection is bound to one interface (`curl --interface`), set with `MOVIES_EGRESS_INTERFACE` in `.env` (default `eth1`). Everything else the container does keeps using its normal interface.

- It **fails closed**: if the interface doesn't exist in the container, Movies returns 503 "Movies are unavailable right now" and never falls back to the normal connection. The log says which interface it wanted and which are present: `[movies] egress interface "eth1" not found in this container (present: eth0, lo)`. That line is the first thing to check when Movies shows the 503.
- The interface needs its own route to the internet (a default route, which can have a higher metric than eth0). Binding only forces the interface; it doesn't create a route.
- DNS lookups use the container's normal resolver. Only the connections are bound.

## Cookies

The site hands out a host-only pass cookie (`__Host-sv`). A proxy can't store that name under a path, so upstream cookies are namespaced: `Set-Cookie: name=value` becomes `mv_name=value; Path=/proxy/movies` in the browser, and only cookies with the `mv_` prefix are sent upstream (prefix stripped). Mitch's own cookies never go upstream.

## How the site is adapted

cno.to is a Vite/React single-page app built for the root of its origin. Under `/proxy/movies/` the proxy:

- rewrites root-relative `src`, `href`, `action`, `poster`, `srcset`, `imageSrcSet` and CSS `url()` values and absolute `https://cno.to/` links to the mount, and sets a `<base>` tag;
- gives React Router a default `basename` of `/proxy/movies` (it patches `createBrowserRouter`'s `basename:t?.basename` to `basename:t?.basename??"/proxy/movies"`), so route paths like `/movie/6977-...` stay unchanged inside the mount;
- removes the Cloudflare bot-detection loader the edge injects into pages;
- injects `movies.js` first in `<head>`. It wraps `fetch`, XHR, `sendBeacon`, `setAttribute` and the `src`/`href`/`srcset`/`poster`/`action` setters so URLs built at runtime (`/api/...`, `/hls/...`, the `v.cno.to` mirror) go through the mount, and it stops the PWA service worker registering. Mitch's `/presence.js` and `/api/presence/*` are left alone;
- sets a CSP that only allows the page's own scripts, which also blocks the site's third-party ad loaders (the `cno:ad` and `cno:vault` hosts in its page `<meta>` tags).

If cno.to ships a new build, the router patch in `rewrite_js` is the thing to check. Symptoms: pages render but show "not found" or never leave the loading state, or `/proxy/movies/...` shows up as an unmatched route. The pass step (`/api/p`) currently needs no Turnstile token; if cno.to turns that on (a `cno:pass` meta appears on movie pages), playback will need a real Turnstile solve on the cno.to domain and won't work through any proxy.

## Checks

Rust tests cover path resolution and the host allowlist, traversal refusal, method limits, redirect mapping, cookie namespacing (including that Mitch cookies are never forwarded), the curl command line (interface, `--proto =https`, no redirect following, header-injection refusal), response parsing, HTML/asset rewriting, the router patch and the fail-closed behavior. Beyond that, verify in a browser: the homepage, a movie page and client-side navigation inside `/proxy/movies/`, then Play. Run the usual formatting, strict Clippy and workspace tests before deploying.
