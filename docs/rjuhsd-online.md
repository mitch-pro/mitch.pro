# rjuhsd.school online count

The footer on every rjuhsd.school page shows "N active", the number of people on
rjuhsd.school right now. Every other host shows the signed-in account count from
`/api/presence/count` in the same spot. `webserver/site-status.js` (loaded by the
footer band on every page) decides by host: on rjuhsd.school it loads `online.js`
and never shows the account count. On localhost, `<html data-visitor-count>` (set on
`/rjuhsd/`) opts in so the school page can be tested locally.
Code: `rust/crates/mitch-server/src/routes/rjuhsd_online.rs` (server) and
`webserver/rjuhsd-assets/online.js` (client).

It only exists on rjuhsd.school. The server returns 404 for the endpoint on every
other host (localhost is allowed outside production for testing), and `online.js`
does nothing elsewhere. This is separate from `/api/presence/count`, which counts
signed-in accounts on the main site.

## Protocol

- `GET /api/rjuhsd/online` returns `{count, challenge: {token, bits}}`.
- `POST /api/rjuhsd/online` with `{vid, token, n}` counts the visitor and returns the
  count plus the next challenge. It needs the usual `X-Mitch-Requested-With: 1`
  header and a same-origin request, like other mutating endpoints.

`vid` is a random 16 to 40 character id the page keeps in `sessionStorage`, so it
is per tab. It is not tied to an account.

## Proof of work on every ping

A token is `exp.bits.nonce.mac`. The MAC is HMAC-SHA256 over the expiry,
difficulty, nonce and the caller's network (keyed from `id_secret`), so tokens
can't be forged, made easier, or used from another network. To ping, the client
finds `n` where `SHA-256("{token}:{vid}:{n}")` has at least `bits` leading zero
bits. The proof is bound to the token and the visitor id, so one solve counts one
visitor once. Each token works one time and expires after 2 minutes. The token is
only spent once the proof checks out, so wrong guesses can't burn someone else's
challenge.

Difficulty starts at 16 bits (about 65k hashes, well under a second in a browser).
It follows how fast *new* visitor ids arrive, not how many people are online: it
goes up by one bit for each doubling of new visitors from one network past 128 a
minute (up to +3), and by 2 while the whole site is taking in over 3000 new
visitors a minute. Pings from people already counted stay at the base cost, so a
school with thousands of students behind one IP isn't penalised for being large,
while a flood of fresh ids from one place gets steadily more expensive. Ids past
the caps below still count toward that rate.

## Other limits

- A visitor id counts at most once every 20 seconds (`TooSoon`); the client pings
  every 30 seconds, and only while the tab is visible.
- A visitor drops off 75 seconds after its last ping.
- A network gets at most 12,000 challenges a minute. Networks are the IPv4 address
  or the IPv6 /64. This is sized for a school behind one IP: 5,000 visitors pinging
  twice a minute fit under it (a compile-time check keeps the two numbers in step).
- One network can hold at most 5,000 counted visitors, and the site 50,000. Pings
  past a cap succeed but aren't counted.

No IP is whitelisted. The per-network numbers are the knobs: they have to be high
enough for the busiest real network (the school) and so also set how far one
machine on a shared IP can push the count. Someone on the school network can't be
told apart from a student, so the count is approximate; proof of work makes spam
cost CPU, not impossible, and a large botnet could still move the number.

## Privacy

Everything is in process memory (a visitor id, its network bucket and last-seen
time) and is never written to disk. A restart resets the count until pages ping
again. No account, cookie or page location is involved.

## Testing

`cargo test --workspace` covers the board logic: solved pings, expiry, forged and
replayed tokens, binding to IP and visitor id, ping spacing, difficulty scaling by
new-visitor rate, a school-sized network staying at the base cost, the per-network
cap, and issuance limits. The widget has no browser test; check it by hand with
`/rjuhsd/` on localhost, and a map of rjuhsd.school to 127.0.0.1 for the host check.
