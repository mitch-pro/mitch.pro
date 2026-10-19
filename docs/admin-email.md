# Admin email tools

Open `https://mitchdog.com/admin/#email-tools` as a full administrator and unlock
the admin passphrase. `https://mitch.tylers-tools.com` redirects to this section,
so sign-in and permissions remain on mitchdog.com.

The connection check calls the existing private `mitch-mail-rs` service. It shows
which sender credentials are configured; it does not prove SMTP delivery. The
newsletter sender waits for the service's SMTP acceptance response. Acceptance does not
guarantee inbox delivery. Timeouts are uncertain; check the inbox before retrying.
Verification previews never create a valid security code or change an account.
The Send email link opens the dedicated newsletter editor at `/admin/email/`.

Previews call the actual Rust security and authentication template functions with
sample code `123456`. Template changes are reviewed and deployed through this
repository's normal master/GitHub Actions workflow.

All backend email paths use `MAIL_RS_URL` if provided, otherwise
`MAIL_RS_HOST` (or `MAIL_SERVICE_HOST`) and `MAIL_RS_PORT` (default 6902).
Production Docker Compose sets the host to `mail-rs`. SMTP credentials stay in the
mail container's existing environment and mounted `.env`.

The Cloudflare configuration is recorded in
[`infra/cloudflare/mitch-mail-admin.json`](../infra/cloudflare/mitch-mail-admin.json).
It is applied separately through the Cloudflare API: create the proxied DNS
record and append the redirect rule to the existing dynamic-redirect ruleset.
Preserve unrelated rules. The documentation address `192.0.2.1` is a placeholder
for this redirect-only hostname; Cloudflare handles requests before an origin
connection. No new server, tunnel ingress or public `/send` endpoint is required.

## Newsletter editor

The editor starts with an example, or the current administrator's saved draft.
Customize the subject, inbox preview text, headline, introduction, up to eight
reorderable sections, cover image, button, sign-off, accent color, and light or
dark design. Desktop and phone previews use the same Rust renderer as delivery.
Download HTML exports the preview without a personal unsubscribe token.

Message, Appearance, and Recipients tabs sit beside the live preview. Arrow keys,
Home, and End navigate the tabs; reviewing an invalid field opens its tab. The
email uses a sans-serif layout with a dark masthead, unnumbered update cards,
and Mitch & Tyler branding and sign-off. The edition/date line is editable.

The default is the October 16–18, 2026 weekend update, based on this repository's
Friday-through-Sunday changes: site design and shortcuts, profiles and leaderboard,
sign-in and site tour, Matrix, presence and privacy, VM/admin controls,
Tylerium/Blooket, and email/build tooling. Existing saved drafts stay intact;
Load weekend update replaces the message with the new default while preserving
recipient choices. New drafts start with no recipients selected.

Choose everyone signed up, selected members, or additional individual addresses.
The registered audience comes from accounts, profiles, and password records;
unused invitation tokens and manually added newsletter addresses are not included
in everyone. Both audience modes exclude unsubscribed addresses and deduplicate
email addresses. Existing unsubscribes are also rechecked during delivery.

Saving, previewing, opening the editor, and reviewing recipients never send email.
Review email creates a snapshot of the exact message and audience for 15 minutes.
Only the final Send button submits that snapshot. The server atomically claims it
in SQLite, so repeated submissions cannot send it twice. Each message is sent
individually through mitch-mail-rs with its own unsubscribe link, without CC/BCC.

Delivery runs in the background with counts for SMTP acceptance, failure,
unconfirmed outcomes, and skipped unsubscribes. SMTP acceptance is not proof of
inbox delivery. Unconfirmed messages are never retried automatically. A stalled
job is shown as interrupted after two minutes without progress; a restart does
not resume or resend it. Drafts and delivery status are private to their author
and require a full administrator session and passphrase. No email open or link
tracking is added.

Local verification:

```sh
cargo test --manifest-path rust/Cargo.toml --workspace
cargo build --manifest-path rust/Cargo.toml -p mitch-server
node tests/test_admin_email_browser.mjs
```

The browser suite starts an isolated app and a local mock mail service. It never
sends real email. Set `NEWSLETTER_ARTIFACT_DIR` to a directory outside the repo to
export an example HTML file and preview screenshots during the test.
