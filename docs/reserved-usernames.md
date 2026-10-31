# Reserved usernames

The owner can reserve usernames so nobody can claim them. Manage them in the
admin panel under **Site operations -> Reserved usernames** (owner only).

## Storage

`reserved_usernames` SQL table (`mitch_lib::reserved_usernames`): `username`
(primary key, normalized), `holder_email` (normalized, empty = nobody),
`reserved_by`, `reserved_at`.

## Behavior

- A reserved name can't be chosen at signup (`routes/auth.rs`), in profile
  settings (`routes/me/account.rs`), or picked as an automatic default
  (`profile::ensure_profile_defaults`, so an email like `admin@...` gets
  `admin-2`).
- `holder_email` lets one account use the name (for example the owner's own).
- An account that already has the name when it is reserved keeps it.
- Removing a reservation frees the name immediately.

## API (owner only)

| Endpoint | Body | Effect |
| --- | --- | --- |
| `GET /api/admin/reserved-usernames` | | `{reserved: [{username, holderEmail, reservedBy, reservedAt}]}` |
| `POST /api/admin/reserved-usernames` | `{username, email?, active}` | reserve (`active` true or omitted) or free (`false`); returns the list |

Changes are written to the admin action log as `reserve_username` /
`release_username`.
