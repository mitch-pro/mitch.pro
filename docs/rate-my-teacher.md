# Rate My Teacher

Students rate their teachers at `/teachers/`. Stars show at once; a written note
shows only after a moderator approves it. Ratings are anonymous.

- **Who can use it:** student accounts (`@student.rjuhsd.us`) can rate. Moderators
  and admins can open the page but not rate. Everyone else gets a "students only"
  message. Under `GOD_MODE` (local development only) the dev admin may rate too.
- **What is rated:** an overall score (required) and three optional category
  scores: teaching, homework (5 is very little) and chill (5 is very relaxed).
  All are 1 to 5 stars. Teachers are listed per school (the six `RJUHSD_SCHOOLS`
  campuses). A note is up to 500 characters.
- **No edits:** a rating can't be changed or withdrawn by the student.

## How anonymity and one-per-teacher work

Code: `mitch_lib::teachers` (read its module comment first).

- `teacher_ratings` holds teacher, stars and a **day number**. No account,
  session, or exact time.
- `teacher_rating_marks` holds one row per (account, teacher): the hex of
  `HMAC(key, normalized email, 0x00, teacher id)`. The key is derived from the
  server's id secret for this purpose only. It is the only thing consulted to
  refuse a second rating, it has no link to a rating row, and the table is
  `WITHOUT ROWID` so it keeps no insertion order to line up with ratings.
- `teacher_comments` holds the note, teacher, a day number and the review status.
  It does not reference the rating or the writer.
- The email is normalized first, so alias addresses count as one person.
- Insert into the marks table is the atomic check, so two simultaneous requests
  can't both succeed. If saving the rating fails, the mark is removed again.
- Limits: someone with both the database and the id secret could test a guess
  ("did this email rate that teacher?"). Changing an account's email gives it a
  new mark, so it could rate that teacher again.

## Notes and moderation

`/moderator/teachers/` (linked from the moderator panel and `/staff/`) has two tabs.

- **Notes:** Pending, Approved, Rejected, All. Moderators and admins approve,
  reject, or send back to pending. Only admins delete (logged in the admin log
  as `delete_teacher_note`). Staff see the note, teacher, school and date; they
  can't see who wrote it because that isn't stored.
- **Teachers:** pick a school, add a teacher, paste a list (one per line,
  `Name` or `Name | Department`; `Last, First` is turned around), or run
  **Sync from school sites**. Only admins remove a teacher, which also deletes
  their ratings and notes (`remove_teacher` in the admin log).

## Where the teacher list comes from

Teacher names are never kept in the repository. The server reads them from each
school's own site (`RJUHSD_SCHOOLS`, e.g. `woodcreek.rjuhsd.us`) and stores them
in the `teachers` table. Students pick their school on the page and see only
that school's teachers.

- **Automatic:** a worker (`workers_site.rs`) runs 30 seconds after boot and then
  hourly. It reads a school when it was never read, last read over a day ago,
  or failed over six hours ago (`teachers::sync_due`). `teacher_sync_status`
  records when, how many were found, and any error.
- **Manual:** the Teachers tab has *Sync from school sites* and shows each
  school's last read. Pasting a list or adding one teacher also works, for a
  school whose directory can't be read.
- **What is read:** each school's staff directory (`/about-us/staff-directory`,
  `/staff-directory`, `/about/staff-directory`, `/our-staff`, paged with
  `?const_page=N`), Finalsite markup: `fsConstituentItem` blocks with
  `fsFullName` and an optional `fsDepartment`/`fsTitle`.
- **Subject:** taken from the title ("Teacher - Mathematics" or "Science
  Teacher" give Mathematics / Science). People whose title is clearly not
  teaching (principal, counselor, clerk, and similar, unless it also says
  teacher) are skipped; a person with no title is kept.
- A page without that markup yields nothing, never junk. **The parser was written
  against that markup from memory and has only been run on a test fixture: the
  district sites were unreachable when this was built.** If a school's directory
  loads by script or uses other markup, extend `parse_finalsite_directory`.

## API

Student (`routes/teachers.rs`, signed in, students or staff):

- `GET /api/teachers?school=KEY&q=TEXT` list with averages and `rated` for the viewer.
- `GET /api/teachers/one?id=N` one teacher, averages, approved notes.
- `POST /api/teachers/rate {teacherId, overall, teaching?, homework?, chill?, note?}`
  returns 409 when already rated.

Staff (`routes/admin/teachers.rs`, moderators and admins; admins pass the usual
passphrase gate): `GET /api/admin/teachers?status=`, `GET .../list?school=`,
`POST .../note-status`, `.../note-delete` (admin), `.../add`, `.../import`,
`.../remove` (admin), `.../sync`.

Tests: unit tests in `mitch_lib::teachers`; `teacher_ratings_follow_student_and_staff_rules`
in `crates/mitch-server/tests/staff_access.rs`.

Before pushing, run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace`; CI fails on clippy warnings.
