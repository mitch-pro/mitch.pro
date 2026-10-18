# People and Friends presence

`webserver/presence.js` reports a random per-document ID, increasing sequence,
path, activity, focus, and visibility. It sends on navigation and focus changes,
with a 15-second heartbeat over the existing broadcast WebSocket or authenticated
HTTP fallback. The server strips queries and fragments. It does not receive
keystrokes, chat contents, or VM screen contents.

`rust/crates/mitch-server/src/presence.rs` resolves one account's current activity
across its tabs. Focused tabs take priority, then visible tabs, then the most
recent interaction or navigation. A routine background heartbeat cannot take
priority from the foreground tab. Background Matrix sync and game reward
requests cannot overwrite a document report. Visible but unfocused pages show
as away.

Same-origin embedded pages delegate to their parent reporter. The homepage's
game frames, game portal player, and in-app browser report the page actually
open inside them. The tracker is also injected into standalone proxied pages,
VM desktops, Matrix SPA fallbacks, and legacy content responses. Proxy injection
runs before upstream scripts replace fetch. HTML with conditional authenticated
injection is private and uncached.

People and Friends use the same resolved in-memory server snapshot. What page or
activity a user is on is only returned to that user's friends, the user
themselves, and site admins; non-friends only see whether the member is online or
offline. Users can also disable page location entirely in Preferences > Privacy
("Page Location" toggle), which prevents anyone—including friends and site
admins—from seeing their current page or activity. Activity changes broadcast a
global invalidation without an email or activity in the message; clients obtain
their viewer-specific directory or friends response. Visible lists refresh
within about a second and periodically poll as a fallback. Older fetch responses
cannot overwrite newer results.

Page exit ends only that document. Closing a tab falls back to another live
tab, while delayed requests cannot revive an ended document. A disconnected
WebSocket gets a three-second grace period for navigation, reconnection, or
HTTP fallback. A fresh fallback heartbeat survives cleanup of the old socket.
An abrupt browser crash or network loss expires activity after 45 seconds;
server ping/pong also expires dead transport connections. A connection without
a fresh document report does not claim an old page. Restoring a cached page
resumes its document with a higher sequence number.

Presence is strictly in-memory (process-local) and is never written to disk or
SQLite. A deployment or restart resets it until clients reconnect. Already-open
tabs need one refresh to load the new tracker. Cached legacy clients retain
compatibility, but cannot provide all the focus and ordering guarantees of the
current tracker.

Validation: `cargo test --workspace` includes resolver, privacy, and injection
tests. After building `mitch-server`, run `node tests/test_presence_browser.mjs`
for real-browser navigation, multiple tabs, iframe delegation, friend/admin vs.
non-friend visibility, disconnects, HTTP fallback, VM/Matrix routes, page
restoration, authentication, and stale-request checks. It uses isolated fixture
accounts and a temporary local database.
