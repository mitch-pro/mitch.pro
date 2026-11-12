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
