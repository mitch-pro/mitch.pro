# Feedback inbox

What people send from `/feedback/` (general feedback, broken things, page or game
requests, ideas) and where staff read it.

- **Submitting:** `POST /api/suggest` (`routes/auth.rs`), with the reCAPTCHA check.
  Each submission is a row in the `feedback_entries` SQL table: type, text (up to
  2000 characters), the sender's account email when signed in (never the raw
  session id), time and status. Nothing writes to the old `data/suggestions.json`
  any more.
- **Reading:** `/moderator/feedback/`, linked from the staff page (`/staff/`).
  Moderators and admins can read it and mark entries handled or reopen them; only
  full admins can delete (recorded in the admin log). Admins unlock with their
  passphrase like every admin page; moderators do not need one. The page
  filters New, Handled and All and shows the newest 300.
- **API:** `GET /api/admin/feedback?status=new|handled|all`,
  `POST /api/admin/feedback/status {id, status}`, `POST /api/admin/feedback/delete
  {id}` (`routes/admin/feedback.rs`, rules in `mitch_lib::feedback`).
- **Old entries:** the first time anyone opens the list, `suggestions.json` is
  imported into the table (idempotent: an entry is unique on time, type and text).
  Only an email is kept as the account; the file's raw session ids are not copied.
  The legacy `/api/admin/data` type `suggestions` now reads the table.
