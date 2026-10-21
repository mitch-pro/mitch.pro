# Notifications

`broadcast.js` owns the shared bell on every page. Both WebSocket invalidation
names (`refresh_notifications` and `notifications_refresh`) refresh the same
inbox. Initialization is guarded, GET requests are serialized and uncached, and
hidden tabs defer polling. Notices are deduplicated by type and ID. The badge
uses the server's total, including items outside the 25-item display limit.
Same-origin tabs share read invalidations through BroadcastChannel. Failed read
requests retain their notices instead of silently removing them.

Read mutations require POST, an authenticated, unrevoked, password-enabled
session, and the global CSRF gate. A displayed email is separate from the DM
sender's account key. Reading one DM/group notice includes its timestamp, so
newer messages arriving while a user clicks remain unread. Matrix notices are
marked by notice ID with a timestamp bound rather than clearing a whole room.
Matrix delivery retries are deduplicated by their actual upstream event IDs;
grouped notices update their destination to the latest event using the installed
Cinny room/event route. Explicit **Mark all read**
clears the current inbox. Existing chat read receipts still apply independently.

DM/group destinations include the conversation and latest message ID. The secure
chat client highlights a target once rendered. Matrix notices retain room/event
links. Coin notices open the wallet, and friend alerts open Friends. Destinations
are validated and kept on the user's current site origin. The service worker
navigates an existing window to that destination or opens one; it no longer puts
unrelated destinations into a generic in-app browser.

Visible alerts in the active matching conversation are suppressed. A different
room or conversation can still alert. Stable conversation tags replace an
existing system notification silently (`renotify: false`), while repeated event
IDs do not redisplay an existing notification. Incoming calls keep their call
behavior. In-page toasts appear only in a focused tab and claim a bounded local
set of message IDs under Web Locks to avoid duplicate tabs. No message body or
browsing location is stored in that set. Friends-online alerts honor hidden
presence and preferences, with a per-account five-minute cooldown in memory.

Local tests use disposable data and a mocked service worker; they do not send
messages or push alerts to anyone.

Message appends, read receipts, and Matrix notification changes use SQLite write
transactions over the existing documents. A read operation cannot overwrite a
concurrent append. Concurrent retries with the same DM client ID return the
stored message and do not award coins or fan out a second alert. Atomic edits
still apply account-ban chat-cleanup filtering before commit.

The first atomic edit imports an existing on-disk legacy document if SQLite has
no row yet. Corrupt/unreadable input fails without replacing it with an empty
inbox. Subsequent edits use the committed database row.
