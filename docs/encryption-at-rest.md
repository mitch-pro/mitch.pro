# Encryption at rest

Saves (`/api/userdata`, see [saves.md](saves.md)) are stored encrypted with a
master key kept in the secrets manager, not in the data folder. The database is
not encrypted; see "Database" below.

Code: `mitch_lib::keyring` (key handling, format), `mitch_lib::userdata`
(file read/write/sweep). Where secrets come from is a host concern:
[secrets-providers.md](secrets-providers.md).

## The key

One 32-byte master key, written as 64 hex characters or base64. Where it comes
from, in order:

1. `DATA_ENCRYPTION_KEY` in the environment. The host's secrets provider
   (Doppler, or Infisical when `INFISICAL=True` is set in `.env`) puts it there
   when it starts the containers; the app itself knows nothing about either. See
   [secrets-providers.md](secrets-providers.md).
2. `data/encryption.key`, the bootstrap file.
3. None of those: the server generates a key into `data/encryption.key`
   (mode 0600) and sends an ntfy notice (`NTFY_TOPIC`) asking you to copy the
   key into the secrets manager as `DATA_ENCRYPTION_KEY` and delete the file. The
   notice never contains the key, and repeats at start and every 12 hours until the
   key is configured and the file is gone.

The container must be given the variable. `docker-compose.yml` lists the
environment variables each service receives. `DATA_ENCRYPTION_KEY`,
`DATA_ENCRYPTION_KEY_PREVIOUS` are listed for `webserver-blue` and `webserver-green` (a bare name passes the host's value
through, and is simply unset until the secrets manager provides it). Add them
to any other service that starts to read encrypted data.

### Moving the key out of the data folder

1. Start the server without a key. It generates `data/encryption.key` and sends the notice.
2. Copy the file's contents into the secrets manager as `DATA_ENCRYPTION_KEY`.
3. Restart so the container receives it. The server then notices the file is
   redundant and says to delete it.
4. Delete `data/encryption.key`. Keep an offline copy of the key: without it the
   encrypted saves can't be read.

### Rotation

Put the new key in `DATA_ENCRYPTION_KEY` and the old one in
`DATA_ENCRYPTION_KEY_PREVIOUS` (comma-separated for several). On start the
server re-encrypts every save to the new key. Once the log shows no files left
under old keys (`saves under key <id>: ... failed: 0`), the previous key can go.

### Refusing to lose data

`data/encryption.keyid` records the id (a fingerprint) of the key in use. If the
server starts without a key whose id matches (the secrets manager is
unreachable and the file is deleted, or a rotation dropped the old key too
early), it refuses to start instead of generating a new key that would orphan
everything. A save that can't be decrypted returns 503 and is never treated as
empty, so it can't be overwritten.

## Format

`MPENC\x01 | key id (4) | nonce (12) | AES-256-GCM ciphertext and tag`. The cipher
key is HKDF-SHA256 of the master key with a purpose (`userdata`) and a context
(the account's folder name); both are authenticated, so a file can't be copied
into another account's folder. Files are replaced atomically (temp file, then
rename) under a process-wide lock shared with the startup sweep.

Files written before this existed are plain JSON. They stay readable, and the
startup sweep (and any later save) encrypts them. The sweep is idempotent.

## What this protects

A copy of the data folder, a backup, or a stolen disk without the key. It does
not protect against someone with access to the running server, which holds the
key in memory, or to the secrets manager. The key shares the data folder only
until it's moved.

## Database

Not encrypted. SQLCipher would encrypt the file, but it's unclear Litestream (the
`sql-sync/` sidecar) can replicate an encrypted database, and when the database
lives on the ramdisk the copies on disk are the replica and backups, not the live
file. Encrypting where the replica is stored fits that setup better. Unresolved;
nothing in the code encrypts the database.
