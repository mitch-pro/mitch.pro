# Personal data

What the site keeps about people, why, and for how long. The default is to
collect less: if a feature doesn't need a piece of data to work, it isn't
stored. When code stops storing something, the data older versions wrote is
deleted too — `mitch_lib::privacy::run_startup_cleanup` runs on every server
start and removes it (every step is idempotent).

## Not kept

- **Browsing history.** No record of which pages an account opens, request
  paths tied to an account, referrers or query strings. The per-ping session
  log (`sessions.json`) and the admin views built on it are gone.
- **IP addresses tied to accounts.** No "last known IP", no IP or user agent
  on login sessions, no IP in cheat logs, staff action logs or application
  logs. Requests are rate-limited by IP in memory only (cleared within an hour,
  gone on restart).
- **Presence.** Online/offline, current page, activity ("Chatting in Matrix"),
  and per-tab state live in server memory only. After a restart everyone shows
  as offline until they're seen again. See [presence.md](presence.md).
- **Login activity times.** Login sessions store creation and expiry only, not
  a last-used time.
- **Search queries.** The old search log and its admin endpoint are removed.
- **Third-party trackers.** No analytics, ad or social scripts on site pages.
  The vendored Cookie Clicker page ships with its upstream AdSense, Facebook
  Pixel, Playsaurus ads and cookie banner switched off (`THIRD_PARTY=false`).
  reCAPTCHA verification doesn't forward the visitor's IP to Google.

## Kept, and why

| Data | Where | Why | How long |
| --- | --- | --- | --- |
| Last active **day** | `user_stats.last_active_at` (start of the UTC day) | Premium expires after 7 days of inactivity | Until the account is deleted |
| Explicit network bans | `banned_ips` — keyed HMAC of the address, never the raw IP | Ban enforcement | Until unbanned |
| Support ticket IP hash | `support_tickets.ip_hash` (keyed HMAC) | Hourly per-IP limit on opening tickets | Cleared after 1 day |
| Anti-cheat flags | `cheat_logs` table (account, game, reason) | Moderation | 90 days, newest 1,000 |
| Staff actions | `admin_actions.json` (actor, action, parameters) | Audit trail of privileged changes | Newest 1,000 |
| VM audit log | `vm_audit_logs` | VM security and abuse reports | 180 days |
| VM usage samples | `vm_usage_samples` | Daily VM time limits and usage charts | 30 days |
| Application logs | `app_logs` | Debugging; no IPs, user agents or account-linked paths ([system-logs.md](system-logs.md)) | 30 days |
| Aggregate game plays | `global_game_stats.json` | Popular games list; counts per game, no account | Kept |
| Per-account playtime counters | `user_stats.playtime` (count per category) | Profile stats | Until the account is deleted |

## Who can see what

- Other members see whether someone is online. Current page/activity and how
  recently they interacted are shown only to the member, their friends, and
  admins — and only when the member turned on **Share activity** (off by
  default). **Hide online** hides them from everyone, including admins. Both
  are enforced in the server's presence resolver, not just the UI.
- The admin "Active users" list reads the same in-memory presence, so it
  respects both settings.
- Staff never see raw IP addresses: none are stored where a staff view could
  read them.

## Adding something new

Before storing data about a person, answer: what feature breaks without it,
what's the coarsest form that still works (a day instead of a timestamp, a
keyed hash instead of an IP, a count instead of a list), and when it gets
deleted. Write that down in the table above in the same change. Presence and
current-page data must never be persisted (AGENTS.md, "Privacy and telemetry").

## Validation

`cargo test -p mitch-server --test privacy` starts a real server, sends a
signed-in request with a forwarded IP, a ping and a presence report, then
checks that neither the IP nor the page appears anywhere in the database or the
data directory. It also seeds the retired files and checks the startup cleanup
removes them while an explicit network ban still works.
`mitch_lib::privacy` has a unit test for each cleanup step.
