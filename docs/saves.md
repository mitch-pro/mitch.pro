# Saves (MitchSync) and the save manager

## What is stored

Each signed-in account has one JSON blob on disk at
`$USERDATA_DIR/<sha256(session id)[0..32]>/data.json` (default `/opt/userdata`),
capped at 1 GiB of serialized JSON. The background sync (`webserver/sync.js`)
writes the browser's localStorage there as `_snapshot` (+ `_snapshot_ts`) every
5 minutes and on page close, and offers a restore when the server copy is newer.
Some games (chess-bot) and `/export/` (slot 2: `_snapshot2`) write other
top-level keys into the same blob.

## `/api/userdata`

Authenticated by session cookie; every call only touches the caller's own file.

| Method | Behaviour |
| --- | --- |
| `GET` | The stored blob plus `_quota: {used, limit}`. Never writes. |
| `POST` | Shallow-merges the body's top-level keys into the blob (a posted `_snapshot` replaces the whole snapshot). `413` over quota. A client echo of `_quota` is ignored. |
| `DELETE` | `{"all":true}` removes the blob; `{"keys":[...]}` removes those top-level keys (`_updated` is protected). |

The original `server.js` routed every method into the write path, so a GET
returned `{ok:true}` and sync restore silently never fired. GET now returns the
data; this is a deliberate deviation from the JS server.

## `/saves/` (save manager)

Lists the union of the server snapshot and this browser's localStorage, grouped
into Settings (`_pref*`), Game data and top-level server-only entries, each with
a Synced / Differs / Server only / This browser only badge. Actions: sync now,
restore all, per-entry upload / restore / download / delete-from-server,
download or import a backup file, delete all server saves. Deleting from the
server never touches localStorage. Pushes set `_sync_ts` locally so `sync.js`
doesn't immediately offer to restore what was just uploaded.

Linked from `data/sites` and Preferences → Sync. Test: `tests/test_saves_page.mjs`.
