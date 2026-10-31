# Saves (MitchSync) and the save manager

## What is stored

Each signed-in account has one JSON blob on disk at
`<data dir>/userdata/<sha256(session id)[0..32]>/data.json` (the data dir is the `./data` mount; set `USERDATA_DIR` to put it elsewhere),
capped at 1 GiB of serialized JSON. The background sync (`webserver/sync.js`)
writes the browser's localStorage there as `_snapshot` (+ `_snapshot_ts`) every
5 minutes and on page close. It never restores on its own: when the server copy is newer
it shows a corner notice (Restore / Keep this device / Don't show again) and pauses saving
until one is chosen, so an older device can't overwrite it first. "Don't show again" and the
Preferences → Sync "Restore notice" toggle set `_prefSync.restoreNotice = false`.
The blob has two save slots: the **auto slot** (`_snapshot`/`_snapshot_ts`, owned by
`sync.js`) and the **manual slot** (`_snapshot2`/`_snapshot2_ts`), which is written
only by the Save button on `/saves/` and is never touched by background sync or
"Sync now". Both are scrubbed of sensitive keys.
Some games (chess-bot) write other top-level keys into the same blob.

## Game progress in IndexedDB

Many games (Emscripten/Unity builds such as Drive Mad, which mounts IDBFS at
`/sandbox`) keep progress in IndexedDB, not localStorage. `webserver/idb-sync.js`
(`window.MitchIDB`) dumps every IndexedDB database of the origin to JSON
(`dump`), writes them back (`restore`) and hashes them (`hash`). Values are
tagged JSON (`{"$t":"bytes"|"date"|...}`); a database holding Blobs/Files or
over 150 MB is left out whole rather than restored partially. Restoring a
database with a matching schema replaces its records in place; otherwise it is
deleted and recreated at the snapshot's own version (games open with a fixed
version and fail on a higher one). An open game in another tab can block this;
the error says so.

Stored beside the localStorage snapshots as top-level `_idb`/`_idb_ts` (auto)
and `_idb2` (manual), keyed by database name. Auto-sync (`sync.js`) uploads them
only when the dump changed since the last upload (`_sync_idb_hash`, device-local,
never synced), merged per database over the server copy so a device that only
played one game doesn't erase the others, and never uploads an empty set. It
checks 8 seconds after a sync page opens and when the tab is hidden, since games
write while the player is on pages that don't run `sync.js`. All games on the
origin share one IndexedDB namespace, so two games using the same database name
(e.g. `/sandbox`) overwrite each other; that predates sync.

## What is never synced

Matrix login and encryption keys are excluded everywhere (IndexedDB database names are checked with `is_sensitive_idb_name` / `isSensitiveName`, which also drop `cinny`, `olm` and `crypto` stores): `cinny_*` (the Matrix
access token, device and user ids), `mx_*`, anything containing `matrix`, the
site's own E2E key material (`_e2e_private_jwk*`, `_e2e_key_history_v2`), anything
containing `e2e`, `passphrase`, `access_token` or `private_key`, and
`admin_passphrase`. Three layers enforce it, kept in step:

- `webserver/sync.js` and `/saves/` never put them in a snapshot and never write
  them back on restore or import.
- `routes/members.rs::is_sensitive_sync_key` strips them from every POST and GET,
  and rewrites a stored blob that an older client already filled with them.

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

Has an Auto slot panel (sync status, sync now, restore everything, backup download/import,
delete all) and a Manual slot panel (save this browser, restore, download, delete).
Below them, lists the union of the server snapshot and this browser's localStorage, grouped
into Settings (`_pref*`), Game data and top-level server-only entries, each with
a Synced / Differs / Server only / This browser only badge. Actions: sync now,
restore all, per-entry upload / restore / download / delete-from-server,
download or import a backup file, delete all server saves. Deleting from the
server never touches localStorage. Pushes set `_sync_ts` locally so `sync.js`
doesn't immediately offer to restore what was just uploaded.

Linked from `data/sites` and Preferences → Sync. Test: `tests/test_saves_page.mjs`.

## Request size

`POST /api/userdata` bodies may be up to 64 MiB (`main.rs`, path-aware body cap);
the default cap for other API routes is 256 KiB, and a larger body is replaced
with zero bytes, which fails JSON parsing as `400 Bad JSON`. Game databases
uploaded from IndexedDB exceed 256 KiB, so this route needs the higher cap. The
1 GiB per-account quota still applies. A reverse proxy in front of the server
needs a body limit at least as large.
