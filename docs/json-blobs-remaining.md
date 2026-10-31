# JSON blobs still in the database

Hand-off for the agent continuing the JSON-to-table migration. Snapshot of the code at the commit that adds this file; verify against `rg read_document` before relying on any single row. There is no production DB copy here, so sizes and row counts are unknown. Ask for a copy of `mitchpro.db` and run the queries at the bottom to rank by size.

## The rule

`AGENTS.md` (Data storage): no new JSON-blob storage. Anything new gets a real SQL table with typed columns and an index on anything looked up by something other than the primary key. Existing blobs are legacy and are being migrated away.

## How the blobs work

- `mitch_lib::data::DataStore::{read_document, write_document, write_document_raw}` (`rust/crates/mitch-lib/src/data.rs`). A path under `data/` or `mail/` ending in `.json` is stored as one row in `json_documents(path TEXT PK, content TEXT, updated_at)`; `.jsonl` goes to `jsonl_documents`. `should_store_in_db` decides; `PRESERVED_DATA_FILES` stay real files on disk (not blobs; listed below).
- Each blob is read and rewritten whole, so every write serializes the whole file and two writers can lose each other's update. That is the main reason to migrate.
- `chat_cleanup::filter_document` is applied on read and write to `dms.json`, `pickle_dms.json`, `e2e_attachments.json` and `gif_favorites.json` (ban cleanup hides removed accounts' messages). Moving those means moving that filter to SQL.
- Callers usually reach a blob through `data_file(state, "x.json")` or `state.data_dir().join("x.json")`; some use `base_dir.join("data/x.json")` (e.g. `testers.json`). Search both forms.

## Migration pattern to copy

Look at how `coins`, `daily_logins`, `profiles`, `tokens`, `friend_lists` were done:
1. `CREATE TABLE IF NOT EXISTS ...` in `data.rs` next to the others, with real columns and an index.
2. A `backfill_<name>_from_json()` in `DataStore::open` that reads the old `json_documents` row once, inserts into the table, and is idempotent (cheap no-op after the first run). Never delete the old row in the same release.
3. A small module in `mitch-lib` (see `daily_logins.rs`, `tester.rs`) with typed get/set functions; switch every caller; keep the JSON shape at the HTTP boundary unchanged so the frontend doesn't break.
4. Stop writing the blob, add a unit test with a temp `DataStore`, and update the matching doc under `docs/`.
5. Watch for read-modify-write sequences: the point of the table is `BEGIN IMMEDIATE` on one row, not load-all/save-all.

## Still blobs in `json_documents`

Files and the non-test modules that read or write them. Grouped by what they hold; roughly priority order within the document (user data that grows first).

### Chat and DMs (retired main-site chat; Pickle Club Cellar still writes some)

| File | Used by |
| --- | --- |
| `dms.json` | `account.rs`, `notifications.rs`, `members.rs` |
| `groups.json` | `notifications.rs` |
| `chat_expiry.json` | `chat.rs`, `dm.rs` |
| `e2e_attachments.json` | mitch-lib/src/chat_cleanup.rs (tracked doc; also written by routes/dm/attachment.rs) |
| `pickle_chat.json` | `pickle.rs` |
| `pickle_bulletin.json` | `pickle.rs` |
| `premium_chat.json` | `account.rs` |
| `public_chat.json` | `account.rs` |

### Matrix glue

| File | Used by |
| --- | --- |
| `matrix_users.json` | `accounts.rs`, `matrix.rs` |
| `matrix_notifications.json` | `matrix.rs`, `notifications.rs`, `workers_email.rs` |
| `matrix_email_sent.json` | `matrix.rs`, `workers_email.rs` |
| `matrix_room_settings.json` | `moderation.rs`, `matrix.rs` |
| `matrix_official_rooms.json` | `moderation.rs` |

### Sessions, auth and security

| File | Used by |
| --- | --- |
| `auth_sessions.json` | `auth.rs`, `vm.rs` |
| `sessions.json` | `school.rs`, `data.rs`, `economy.rs`, `account.rs`, `proxy.rs`, `workers_email.rs` |
| `generations.json` | `auth.rs` |
| `invalidated.json` | `auth.rs`, `proxy.rs` |
| `passkeys.json` | `accounts.rs`, `webauthn.rs` |
| `signup_codes.json` | `accounts.rs`, `auth.rs` |
| `last_known_ips.json` | `handler.rs`, `moderation.rs`, `matrix.rs` |
| `admin_passphrase.json` | `admin.rs` |
| `pending_security_actions.json` | `email.rs` |

### Email and newsletter

| File | Used by |
| --- | --- |
| `unsubscribe_tokens.json` | `send.rs`, `handler.rs`, `email.rs`, `legacy.rs`, `newsletters.rs`, `auth.rs` |
| `unsub_requests.json` | `data.rs`, `workers_email.rs` |
| `email_log.json` | `workers_email.rs` |
| `newsletter_extra.json` | `data.rs`, `auth.rs` |
| `referral_sources.json` | `auth.rs` |

### Social and clubs

| File | Used by |
| --- | --- |
| `friend_requests.json` | `friends.rs`, `account.rs`, `members.rs` |
| `pickle_members.json` | `pickle.rs` |
| `pickle_votes.json` | `pickle.rs` |
| `pickle_owners.json` | `pickle.rs` |
| `pickle_crunch.json` | `pickle.rs` |
| `chatroom_bans.json` | `pickle.rs` |

### Economy-adjacent (apply the economy-integrity rules in AGENTS.md)

| File | Used by |
| --- | --- |
| `marketplace.json` | `marketplace.rs` |
| `premium_gifts_sent.json` | `economy.rs`, `account.rs` |
| `unlocked_ai.json` | `shop.rs`, `cosmetics.rs` |
| `applications.json` | `auth.rs`, `data.rs`, `economy.rs`, `auth.rs`, `account.rs`, `members.rs`, `workers_site.rs` |
| `vm_applications.json` | `vm.rs` |
| `vm_upgrades.json` | `vm.rs` |
| `vm_apps.json` | `vm.rs` |
| `vm_credentials.json` | `vm.rs` |

### Content

| File | Used by |
| --- | --- |
| `blog_posts.json` | `blog.rs` |
| `blog_comments.json` | `blog.rs` |
| `blog_subscribers.json` | `blog.rs` |
| `blog_contributors.json` | `admin.rs`, `moderation.rs`, `blog.rs`, `account.rs` |
| `blog_delete_log.json` | `moderation.rs`, `blog.rs` |
| `backgrounds.json` | `backgrounds.rs` |
| `suggestions.json` | `data.rs`, `auth.rs` |

### Moderation and audit

| File | Used by |
| --- | --- |
| `appeals.json` | `bans.rs`, `data.rs`, `auth.rs` |
| `chat_reports.json` | `admin.rs`, `moderation.rs`, `manage.rs`, `marketplace.rs`, `matrix.rs` |
| `profile_reports.json` | `admin.rs`, `moderation.rs` |
| `canvas_reports.json` | `legacy.rs` |
| `canvas_banned.json` | `legacy.rs` |
| `byo_os_reports.json` | `byo_os.rs` |
| `cheat_logs.json` | `admin.rs` |
| `admin_actions.json` | `admin.rs`, `moderation.rs`, `matrix.rs` |
| `shadow_bans.json` | `moderation.rs`, `matrix.rs` |
| `moderator_requests.json` | `admin.rs` |

### Games and stats

| File | Used by |
| --- | --- |
| `global_game_stats.json` | `mod.rs`, `misc.rs`, `proxy.rs` |
| `heatmap.json` | `admin.rs`, `misc.rs` |
| `chess_vs.json` | `chess_vs.rs`, `state.rs` |
| `blooket_bot_failures.json` | `blooket.rs` |
| `search_intent.json` | `dashboard.rs` |
| `madlibs.json` | `madlibs.rs` |
| `jeopardy_kids_clean.json` | `jeopardy.rs` |

### Misc

| File | Used by |
| --- | --- |
| `ntfy_topics.json` | `notif.rs`, `push.rs` |
| `notif_prefs.json` | `notifications.rs` |
| `team_handled.json` | `team.rs` |
| `team_tokens.json` | `legacy.rs`, `team.rs` |
| `valid_hashes.json` | `proxy.rs` |
| `sebastians_claims.json` | `account.rs` |

### Small admin-edited config (low priority; fine as a key/value table)

| File | Used by |
| --- | --- |
| `testers.json` | `auth.rs`, `tester.rs`, `moderation.rs`, `misc.rs` |
| `site.json` | `data.rs`, `chess_vs.rs`, `misc.rs`, `webauthn.rs`, `workers_email.rs` |
| `shop_catalog.json` | `shop.rs` |
| `featured_game.json` | `moderation.rs`, `proxy.rs` |
| `soft_maintenance.json` | `legacy.rs` |
| `moderator_panel.json` | `admin.rs`, `moderation.rs` |

### Not yet categorised

| File | Used by |
| --- | --- |
| `bad_passwords.json` | `account.rs` |
| `blacklist.json` | `bans.rs` |
| `byo_os_bans.json` | `state.rs` |
| `test-job.json` | `newsletters.rs` |
| `vm_bans.json` | `state.rs` |

`.jsonl` blobs (`jsonl_documents`): `canvas_history.jsonl` and the files under `mail/`.

## Per-row JSON inside real tables

These were migrated to tables but each row still keeps a serialized JSON object in a `data TEXT` (or `content TEXT`) column. Lower priority than the blobs above; split into columns only if you need to query inside them.

`mini_game_sessions`, `e2e_keys`, `dm_cleared`, `coin_gift_notices`, `push_subs`, `cosmetics`, `user_stats`, `profiles`, `tokens`, `revoked_ids`, `friend_lists`, `companion_chat_messages` (`content`), `companion_memories` (`content`).

## Not blobs (real files, `PRESERVED_DATA_FILES`)

`admins.json`, `moderators.json`, `bell_overrides.json`, `game_categories.json`, `game_categories_external.json`, `game_categories_local.json`, `bad_words.json`, `prox_blocklist.json`, `logic_words.json`, `payloads.json`, `emojis.json`, plus the `games*` directories. Leave these alone unless asked.

## Already migrated (only referenced by backfill code)

`coins`, `names`, `daily_logins`, `passwords`, `achievements`, `friends`, `invite_*`, `newsletter_unsub`, `banned_ips`, `blacklist`, `profiles`, `user_stats`, `tokens`, `revoked`, `push_subs`, `cosmetics`, `coin_gifts`, `e2e_keys`, `dm_cleared`, `pickle_dm_cleared`, `clicker_sessions`, `logic_sessions`, `richard_sessions`. `atomic.json`, `legacy.json`, `outside.json` and `order_check.json` appear only in `data.rs`; confirm they are dead before deleting.

## Things to know before you start

- **Retired chat:** `/encrypt/`, `/public-chat/` and `/premium-chat/` were removed and `/api/dm/*` and `/api/e2e/*` return 410 on the main site. They still serve the Sexy Pickle Club Cellar (`is_pickle_host`), so `dms.json`/`pickle_*` are live for that site. The main-site branches are dead code and `dms.json`'s main-store content can be archived or dropped once the owner agrees. Do not delete stored messages without asking.
- **Economy:** anything touching coins, the shop, marketplace or casino payouts must pass the economy-integrity checklist in `AGENTS.md`. Sandbox testers (`tester_settings`) must stay blocked from paying other accounts.
- **Never SSH to production** or change `deploy.yml`/`deploy.sh` without explicit approval (see `AGENTS.md`). Test locally; CI gates the deploy.
- Run `cargo test --workspace`, `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and the relevant `tests/test_*.mjs` browser tests before pushing.

## Ranking by real size (needs a DB copy)

```sql
SELECT path, length(content) AS bytes, datetime(updated_at/1000,'unixepoch') AS updated
FROM json_documents ORDER BY bytes DESC;
SELECT path, length(content) AS bytes FROM jsonl_documents ORDER BY bytes DESC;
```
