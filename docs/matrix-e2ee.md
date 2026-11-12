# Matrix chat: encrypted-message backup and device transfer

Why: encrypted rooms were easy to lose. Room keys live in the browser's
IndexedDB, so clearing site data or signing in on a new device left old
messages unreadable. Chat accounts are created from the site login (the Matrix
password is derived from the email, see `get_matrix_password_for_email`), so
users never see or manage Matrix credentials either.

Three pieces make it recoverable:

| Piece | Where |
| :--- | :--- |
| Server-side key backup (Conduit `room_keys` API) | `routes/matrix.rs` (gateway forwards it; the old 404 stub for `room_keys/version` is gone) |
| First cross-signing upload authorised by the server | `routes/matrix.rs`: `auto_auth_first_cross_signing_upload` |
| Backup setup, device transfer, restore UI | `webserver/matrix/e2ee-backup.js`, styles at the end of `matrix-galaxy.css` |

## Client behavior (`e2ee-backup.js`)

The script is loaded in `webserver/matrix/index.html` before the Cinny bundle.
Cinny exposes its `MatrixClient` as `window.__mxClient`; the script defines a
setter on that property so it attaches its to-device listener before the first
sync. It also wraps `client.cryptoCallbacks.getSecretStorageKey` so keys the
user typed into this script are visible to the SDK.

On each page load, after the first sync:

| State | What the user sees |
| :--- | :--- |
| No secret storage on the account | Setup dialog (once a day, "Not now" snoozes) plus a "backup is off" card |
| Secret storage exists, this device isn't cross-signed | The device asks the account's other devices for a transfer; a card offers passphrase or recovery key instead |
| Same, but no other device exists | Dialog asking for the passphrase or recovery key |
| Verified, backup not running | Card to turn it on (needs passphrase or recovery key) |

### Setup

The user picks a passphrase (8+ characters). The script calls
`createRecoveryKeyFromPassphrase`, then `bootstrapCrossSigning` and
`bootstrapSecretStorage({ setupNewSecretStorage, setupNewKeyBackup })`, and shows
the encoded recovery key (copy, download, "I saved it" checkbox before Done).
The passphrase derives the same key, so either one unlocks the backup. Neither
is sent to the server.

### Cards, dismissal and adding a passphrase

Status cards have an x. Dismissing one hides that kind of card on this device
for a week (`mitch_e2ee_dismiss_<state>_<user>` in localStorage). Request cards
for a transfer have Decline instead.

A backup made without a passphrase (for example by Cinny's own setup, which
only produces a recovery key) is detected from the secret-storage key
descriptor having no `passphrase`. The user gets a "Your backup has no
passphrase" card and a dialog (once a day) to add one. It re-runs
`bootstrapSecretStorage({ setupNewSecretStorage })` with a key derived from the
new passphrase, which re-encrypts the cross-signing keys and backup key under it
and needs them on the device. If this device doesn't hold them, the dialog asks
for the current recovery key first. "I don't have my recovery key" replaces the
backup entirely (new cross-signing keys, new backup, no password prompt; warns that
history only in the old backup is lost).

### Device transfer

Message types (to-device, own user only):

| Type | Sent by | Encrypted | Content |
| :--- | :--- | :--- | :--- |
| `com.mitch.e2ee.transfer_request` | new device, to `*` of its own user | no | `request_id`, `device_id`, `client_ts`, `summary` ("Chrome on Windows") |
| `com.mitch.e2ee.transfer_response` | existing device | no | `request_id`, `status: "declined"` |
| `com.mitch.e2ee.transfer_keys` | existing device | yes (Olm) | `request_id`, `chunk_index`, `chunk_count`, `total_sessions`, `sessions[]`, first chunk also `backup: { version, key }` |

1. The new device sends a request (at most once per 6 hours while unverified).
   Requests queue on the homeserver, so a device that is offline gets the card
   when it next syncs. Pending requests are kept in localStorage until answered
   and expire after 14 days.
2. A device that is already set up shows a card: browser summary (reported by
   the new device, so display-only), plus the device name and last-seen IP from
   the homeserver when it has them.
3. On "Transfer messages" it cross-signs the new device (`crossSignDevice`),
   exports its room keys, and sends them in ~48 KB encrypted chunks along with
   the backup decryption key.
4. The new device accepts `transfer_keys` only if it was Olm-encrypted, sent by
   its own user, and matches the `request_id` it generated. It imports each chunk,
   stores the backup key, enables backup and then pulls anything else from the
   server backup.

Trust model: approval on an existing device is the user's proof, like emoji
verification but without the comparison step. The request text is untrusted
(a stolen access token could send one), so all fields are validated and
rendered as text only, and the card tells the user how to remove an unknown
device.

### Restore by passphrase or recovery key

The key is checked against the secret-storage key descriptor before use
(`secretStorage.checkKey`), then `bootstrapCrossSigning` imports the
cross-signing keys, the device is cross-signed, and the session backup is
downloaded with `restoreKeyBackup`. The derived key is held in memory for the
page load only.

## Server: cross-signing upload auth

Conduit requires password UIA on `POST /keys/device_signing/upload`, and users
don't know their Matrix password (and often forget their site password). For any
such upload that carries no `auth` block, the gateway adds the derived-password
block itself (`auto_auth_cross_signing_upload`), for first-time setup and for
resets alike. The request is still authenticated by the user's own Matrix access
token. A client-supplied `auth` block is never overwritten. Consequence: anyone
holding a user's access token can replace that account's cross-signing keys,
which the password prompt used to prevent.

## Encrypted DMs by default

The new-DM dialog in the Cinny bundle (`webserver/matrix/assets/index-E2eeDm01.js`)
starts with encryption on. The bundle is a patched build: everything under
`/matrix/assets/` is served `immutable`, so changing it means giving it a new
hashed filename and updating the references in `index.html` and the other
chunks (and regenerating the `.gz` files). Private rooms still default to off.

## Related behavior

- `index.html` calls `/api/matrix/devices/prune-stale` with `maxAgeDays: 30`
  (was 7) so a device that hasn't been opened for a week can still approve a
  transfer. The server also prunes devices that report no `last_seen_ts`; if
  Conduit doesn't fill that in, other devices will not survive a day.

## Tests

- `tests/test_e2ee_backup.mjs`: recovery-key encoding, request validation,
  chunking (`node tests/test_e2ee_backup.mjs`).
- `routes::matrix::tests`: upload path matching and `auth` injection.
- The dialogs and transfer flow have no automated end-to-end test; check them on
  the dev server with two browsers signed in to the same account.
