# People search (`/members/`, `GET /api/members`)

There is deliberately no way for a signed-in user to list every account.

`GET /api/members` (`routes/members.rs`, requires a session):

- `?q=<text>`: accounts whose **handle or display name** contains the text
  (case-insensitive), at most 25, sorted by the usual ladder. `q` must be at
  least 2 characters (`MEMBER_SEARCH_MIN`); shorter queries return an empty
  list. Emails are never searched.
- No `q`: only people the viewer already has a relationship with: themselves,
  friends, pending friend requests in either direction, and existing DM
  partners. The chat client and the homepage People rail use this.

Everything else about each result is unchanged (presence detail only for self,
friends and admins; see `docs/presence.md`). Admins get no wider list from this
endpoint. The role lists (`/api/admin-members`, `/api/moderator-members`,
`/api/owner-members`, `/api/premium-members`) are small public staff/premium
lists, not the roster. `GET /api/friends/list` returns the caller's own friends.

## Clients

- `/members/` fetches only after 2+ characters are typed (250 ms debounce) and
  shows "Search for someone by name or handle" before that.
- `/friends/` fetches matches per keystroke for the add-friend autocomplete.
- The homepage "Find friends" button opens `/members/`.

A test in `routes/members.rs` covers: default scope, one-character queries,
email-domain queries, and name search.

## Block profile lookups

An account can turn this on in Preferences > Privacy (`GET/POST /api/me/profile-privacy`,
table `profile_privacy`, `mitch_lib::profile_privacy`). Then:

- `GET /api/profile/<slug>` answers 404 (the same as a missing profile) to everyone
  except the account itself and staff. By slug, username, uid or email.
- `GET /api/members?q=` leaves the account out of search results for everyone except
  itself and staff.
- **Staff** means admins (owner, developer, admin) and moderators. Testers get no
  exception (`routes/me/account.rs::sid_is_staff`).
- Not covered on purpose: the account's own contacts (friends, pending requests and
  people it has messaged) still appear in each other's friend and chat lists, and
  friend requests by exact username still work, so people can connect. Names and
  avatars shown inside chat and games are not profile lookups and are unchanged.

The setting moves with an email change and is erased with the account.
