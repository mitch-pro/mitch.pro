# Community homepage update

- Friends use the existing friends list, profile privacy handling, and presence. Only friendly activity labels are shown; raw page URLs and query strings are not displayed.
- Guest previews last 60 seconds per browser/origin. The server signs an HttpOnly cookie so reloads and normal navigation do not restart the timer. Clearing cookies or using another browser starts a new preview; this is onboarding, not a security boundary. Private account APIs still require authentication. Guests can open the local games portal without being forced through the cross-domain login bridge.
- Signup still verifies email, minimum password length, common passwords, consent, and CAPTCHA. The external breached-password lookup has been removed.
- Owner tools are under **Owner tools → Accounts & Mitch Coins**. They require a current owner/co-owner session, the existing admin passphrase, same-origin requests, exact typed confirmation, and a one-minute mutation cooldown. Owners cannot remove their own or other owner registrations.
- Registration removal clears passwords, passkeys, pending signup codes, application sessions, names-to-session mappings, and recovery/claim tokens; it advances the session generation. Existing profiles, content, purchases, MFA settings, and Matrix sessions are retained. It is not a full personal-data erasure tool.
- Coin reset sets current balances to zero, including staff. Purchases, achievements, lifetime statistics, and future earning remain intact.
- Both changes write through the existing SQLite document store in a transaction and update the application's in-memory caches. No bulk action runs during deployment.
- Recovery snapshots are in `DATA_DIR/owner-action-backups/*.enc`, outside the webroot, mode 0600. They are AES-256-GCM encrypted: 12-byte nonce, 16-byte tag, then ciphertext. The key is HMAC-SHA256 of `owner-backup-v1` using the existing `id_secret.key`. Decrypted JSON contains the action, timestamp, and pre-change documents. Recovery requires a server operator and a deliberate database restore; there is no public download or automatic restore endpoint. Keep the existing ID secret backed up securely.

Verification: `cargo test --manifest-path rust/Cargo.toml --workspace`.
