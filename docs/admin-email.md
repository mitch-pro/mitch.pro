# Admin email tools

Open `https://mitchdog.com/admin/#email-tools` as a full administrator and unlock
the admin passphrase. `https://mitch.tylers-tools.com` redirects to this section,
so sign-in and permissions remain on mitchdog.com.

The connection check calls the existing private `mitch-mail-rs` service. It shows
which sender credentials are configured; it does not prove SMTP delivery. The
test button waits for the service's SMTP acceptance response. Acceptance does not
guarantee inbox delivery. Timeouts are uncertain; check the inbox before retrying.
Tests send only to `tyler.thompson3725@gmail.com`, with no CC or BCC, and are logged
in the admin audit history. They never create a valid security code or change an
account. The shared API rate limit allows three test requests per minute.

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
