# Movies shortcut

The signed-in homepage has four equal shortcuts in `webserver/index.html`: Cloud Linux, Matrix chat, Play games, and Movies. `home.css` places them in two columns and two rows above 760px; narrower screens use one column. Homepage preferences still control their visibility and launch behavior.

Movies opens `/proxy/rivestream/watch?type=movie&id=969681`, preserving the supplied watch parameters. The fixed-origin proxy in `rust/crates/mitch-server/src/routes/proxy.rs` forwards GET and HEAD requests only to `https://www.rivestream.app`. It does not forward Mitch cookies or authorization headers, and server redirects stay on the Rivestream origin.

The proxy rewrites HTML/CSS assets, Next's asset prefix and compiled router base-path helpers, and browser fetch/XHR/DOM URLs. Internal page names such as `/watch` stay unchanged so hydration, navigation, and refresh work under the proxy prefix. `rivestream.js` is injected only into proxied Rivestream HTML. It prevents the upstream PWA from registering a service worker on Mitch's origin. The document's script policy permits its proxied bundles and blocks unrelated external scripts.

Rivestream's own player providers remain external. Provider availability and upstream errors are controlled by Rivestream. If its Next build changes, check the router adaptations against the new bundles.

Verification covers both themes at 320, 390, 768, 1024, and 1440px, equal shortcut sizes, row placement, keyboard launch, the exact movie query, real upstream hydration and client navigation, and returning to Mitch routes without interception. Rust regression tests cover scoped route resolution and preservation of Next page/query data. Run the usual formatting, strict Clippy, and workspace tests before deployment.
