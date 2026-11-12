# Encryption at rest

Saves (`/api/userdata`, see [saves.md](saves.md)) are stored encrypted with a
master key kept in the secrets manager, not in the data folder. The database is
not encrypted yet; see "Database" below.

Code: `mitch_lib::keyring` (key handling, format), `mitch_lib::userdata`
(file read/write/sweep), `mitch-server/src/secrets_manager.rs` (Infisical).

## The key

One 32-byte master key, written as 64 hex characters or base64. Where it comes
from, in order:

1. `DATA_ENCRYPTION_KEY` in the environment. Doppler (`doppler run -- docker
   compose ...`, as `tools/deploy.sh` already does) or `infisical run --` puts it
   there; nothing else is needed.
2. Infisical fetched by the server itself, when `DATA_ENCRYPTION_KEY` is unset
   and the `INFISICAL_*` variables are set (table in `secrets_manager.rs`). Useful
   to migrate without changing how the container starts. It uses a machine
   identity (universal auth) and needs HTTPS access to the Infisical host from the
   container.
3. `data/encryption.key`, the bootstrap file.
4. None of those: the server generates a key into `data/encryption.key`
   (mode 0600) and sends an ntfy notice (`NTFY_TOPIC`) asking you to copy the
   key into the secrets manager as `DATA_ENCRYPTION_KEY` and delete the file. The
   notice never contains the key, and repeats at start and every 12 hours until the
   key is configured and the file is gone.

The container must be given the variable. `docker-compose.yml` lists the
environment variables each service receives, so `DATA_ENCRYPTION_KEY` (and
`DATA_ENCRYPTION_KEY_PREVIOUS`, `INFISICAL_*` if used) must be listed for the
web server services.

### Moving the key out of the data folder

1. Start the server without a key. It generates `data/encryption.key` and sends the notice.
2. Copy the file's contents into Doppler (or Infisical) as `DATA_ENCRYPTION_KEY`.
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

Not done. SQLCipher would encrypt the whole database file, but the Litestream
sidecar (`sql-sync/`) replicates the plaintext file and Litestream v0.5 removed its
own replica encryption, so encrypting the database changes how it is backed up.
That decision is pending; until then the database and its replica are plaintext.
