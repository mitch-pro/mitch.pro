# Support portal

For people who can't sign in, think their account was taken over, or need their
email changed. Anyone can open a request without an account; moderators chat
with them; email changes need an admin's approval.

Pages: `/support/` (request form), `/support/ticket/#<id>.<token>` (the
requester's conversation), `/moderator/support/` (staff queue, linked from
`/staff/`). The sign-in page links to `/support/`.

## Tickets

Tables: `support_tickets`, `support_messages`, `support_email_requests`
(`mitch_lib::support`). Resolved tickets (with messages and requests) are
deleted 90 days after their last activity. Each ticket stores a keyed hash of the IP
that opened it (never the IP itself) for the hourly per-IP opening limit; the
hash is cleared after a day.

- **Opening** (`POST /api/support/open`): category, a username or email, an
  optional contact email, a message. Protected by reCAPTCHA, a hidden honeypot
  field, 5 tickets/hour per network address (stored only as an HMAC) and 300/hour
  overall.
- **Which account:** the typed text is resolved to an account server-side and
  stored on the ticket. The requester is never told whether it matched, and the
  form has no username picker (a list of usernames shown to signed-out visitors
  would leak the user list). Staff see the matched account's username and a
  masked email, or a warning that nothing matched.
- **Access:** the creator gets a secret token once. Only its SHA-256 hash is
  stored. Every later call (`view`, `reply`, `confirm-email`) sends `id` and
  `token` in the POST body, so the token never appears in a URL or log. The page
  keeps it in the URL fragment and in `localStorage`; losing both means losing
  the conversation (no email link is sent).
- **Chat:** requester replies are limited to 1 per 3 seconds and 200 per ticket;
  staff identities are never shown to the requester.

## Staff API (`/api/admin/support/*`)

Moderators and above (`AdminCtx::is_any_admin`); admins also pass the admin
passphrase gate like every `/api/admin` route.

| Endpoint | Who | Effect |
| --- | --- | --- |
| `GET tickets?status=` | mod+ | queue |
| `GET ticket?id=` | mod+ | messages, matched account, email requests |
| `POST reply` / `status` / `flag` / `assign` | mod+ | chat, open/pending/resolved, compromised flag, claim |
| `POST email-request` | mod+ | ask admins to change the linked account's email |
| `GET email-requests` | admin+ | pending requests |
| `POST email-request/decide` | admin+ | approve or deny |

## Email change rules

Admins don't change emails on their own; every change goes through a ticket:

1. A moderator (or admin) opens a request on a ticket linked to an account and
   must write how they verified the person. One open request per ticket.
2. **Nobody decides their own request.** A different admin or the owner
   approves or denies it (enforced in `decide_email_request`). A denial needs a
   note; notes are staff-only.
3. **Never allowed** (checked when requested and again when approved/applied):
   owner/developer/admin/moderator accounts (they hold their role by email and
   the owner changes those by hand), banned accounts, and any address that
   already has an account.
4. Approving does not change anything. It emails a 6-digit code to the **new**
   address; the requester enters it on their ticket page within 30 minutes
   (5 attempts). Staff never see the code. Only then is the change applied with
   the same routine as a user's own change (`apply_email_change`), which also
   keeps the Matrix account, computers and other data (see
   `mitch_lib::account_email`).
5. The old address gets a "your email was changed" notice.
6. If a moderator marked the ticket **compromised**, applying also replaces the
   password with a random one and deletes the account's passkeys; the person uses
   "Reset password" on the new address. All sessions are signed out in every
   case.

Everything staff do (flag, request, approve, deny) is written to the admin audit
log with ticket and request ids, not addresses.

## Users changing their own email

Preferences -> Account -> Email address, using `/api/me/change-email` and
`/confirm`. Staff and banned accounts get a clear refusal.
