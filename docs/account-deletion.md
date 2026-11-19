# Account deletion and data export

`POST /api/delete-account` (button in Preferences) deletes an account **immediately**. There is no
waiting period.

## Request

Body: `{password}`. Accounts with no password send `{confirmEmail}`; accounts with an authenticator
app also send `{totpCode}`. The server answers `{needs: "email" | "totp"}` when it needs one, and
the Preferences page asks and retries. Staff (admins, owner, co-owner, moderators) are refused: the
owner removes those accounts. Deletion is also refused while the account has an open marketplace
listing, because escrow holds other people's coins.

## What happens, in order (`routes/me/delete_account.rs`)

1. **Computers.** Any VM the account owns is destroyed on Proxmox first. If that fails, nothing is
   deleted and the user is told to retry.
2. **Deleted marker.** A keyed hash of the address is stored in `deleted_account_marks`
   (`account_deletion::mark_deleted`). It exists so deleting and re-registering cannot collect the
   2000-coin referral bonus again; new accounts on a marked address get no referral reward. It holds
   no address, only an HMAC and a timestamp.
3. **Chat tombstone.** `chat_cleanup::begin_erasure` records the cutoff and the account's Matrix ids.
   From then on the gateway hides everything the account wrote (same mechanism as a ban).
4. **Data.** `account_deletion::erase_account` deletes every row in `EMAIL_TABLES` and scrubs every
   JSON document (`scrub_value`): matching keys, array entries and records are removed. App log
   lines that quote the address are deleted. This is synchronous.
5. **Memory and sessions.** Relay keys, queued relay messages, running casino hands, sockets and
   presence are dropped; the login cookies are cleared.
6. **Chat server.** `chat_cleanup::run_job` redacts every message in every room, then deactivates
   the Matrix account (Conduit user-interactive auth, `erase: true`). The request waits up to 12 s;
   a large history finishes in the background worker (every minute, restart-safe because the
   tombstone row is flagged `erase`). `chatPending` in the response says which. The tombstone rows
   are removed once deactivation succeeds; media stays blocked but is no longer tied to an address.

## Kept on purpose

- `blacklist` and `banned_ips`: otherwise deleting an account would lift a ban. Neither holds an address (see below).
- `deleted_account_marks`: see step 2.
- Records about other people that name staff (ticket replies, report handling).
- Chat tombstones until the chat server has finished (step 6). If the chat account can no longer be
  reached they stay, and history stays hidden.

## Adding a table that stores an email

Add `(table, column)` to `EMAIL_TABLES` in `mitch-lib/src/account_deletion.rs`, or to
`KEPT_EMAIL_COLUMNS` with a reason. A test (`every_email_column_in_the_schema_is_erased_or_deliberately_kept`)
fails until you do. The same list drives the export, so what a person can download is what deletion
removes.

## Export

`GET /api/download-information` (the Download Information page) returns the account's rows and
document entries as JSON, minus password hashes, sessions, push keys and encryption keys
(`EXPORT_SKIP_TABLES`).

## Ban records are hashed

`blacklist.email` stores `h:` + an HMAC of the normalized address (`bans::record_key`, keyed with the
site's ID secret), never the address. Signup and login hash the address they are given and match
it; `POST /api/admin/unban-account` hashes the address staff type and deletes the matching row, and
reports that no ban exists when nothing matches. The ban list on `/staff-dashboard/bans/` therefore
shows reason, date and expiry but not whose ban it is; restore from the "Account" form by typing the
email. Plaintext rows from before the change are rewritten at startup (`bans::hash_plaintext_rows`).
Chat-cleanup tombstones for banned accounts (`account_chat_removals`, `moderated_media`) still carry
the email until the chat server finishes.
