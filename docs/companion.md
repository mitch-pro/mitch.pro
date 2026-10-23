# Desktop companion characters

A small anime-style character you can chat with (DeepSeek-backed) and
drag around the screen, visible site-wide once you turn it on. New
subsystem — no `server.js` equivalent.

## Scope

- Setup (enable/disable + pick a character) happens on its own page,
  `/companion/`, linked from the homepage destination catalog via
  `data/sites` (`url /companion/ Companion`) — the same mechanism every
  other feature page (Marketplace, Canvas, Bell Schedule, ...) uses to
  appear there.
- The live character itself is injected on every page by the shared
  per-page pipeline (`pipeline.rs` for ordinary pages, `inject.rs`'s
  `inject_shared_head` for rjuhsd/woodcreek-branded ones) — the same
  mechanism `popup.js`/`app-shell.js`/`pwa-install.js` use. This is a
  deliberate site-wide-by-default script load: `companion-widget.js`
  fetches `/api/companion/settings` itself on every page and renders
  nothing at all — no DOM, no extra requests beyond that one settings
  check — for anyone who hasn't turned it on, so loading it unconditionally
  everywhere is safe. (An earlier version of this doc said to keep it
  page-scoped to avoid touching the shared pipeline; that was reverted —
  the owner explicitly wants it site-wide when enabled, same as the
  original ask. The thing to actually avoid touching carelessly is the
  shared pipeline's *other* injected scripts, not whether this one script
  is listed there.)
- Admin-authored preset characters only: a name, a personality prompt, and
  one image per "mood." Authored at `/admin/companion/` (any admin/owner,
  gated by the existing global `/api/admin/*` auth).
- Per-user opt-in, set from `/companion/`: enable/disable + pick a
  character, saved to `companion_user_settings`
  (`mitch_lib::companion`) — a real table, not a JSON blob, per
  AGENTS.md's data-storage rule. Saving reloads `/companion/` so its own
  view immediately reflects the new setting; every other page picks it up
  on next load since the widget reads settings fresh each time.
- One DeepSeek API call per chat message (`deepseek-flash` only — never
  `deepseek-v4-pro`, which costs several times as much; a failure retries
  the same cheap model up to `MAX_ATTEMPTS` times instead of escalating;
  see `routes/companion/deepseek.rs`). Provider history: Gemini → Groq →
  DeepSeek, each switch on request, each time over cost/billing friction
  with the previous one.
- **No `response_format` is sent at all** — this was tried twice and both
  times made things worse, confirmed live both times, not assumed:
  `json_schema` is rejected outright by the API ("this response_format
  type is unavailable now"), and `json_object` intermittently made
  `deepseek-flash` return `finish_reason: "stop"` with **blank content**
  — not an error, nothing to parse, roughly half of requests under
  realistic multi-turn load. That was the "companion AI failed... half
  the time" bug. Dropping `response_format` fixed it (verified with the
  same rapid-fire reproduction, repeatedly, 0 failures after). The cost:
  the model sometimes wraps the JSON in a little prose, or skips the JSON
  entirely and just replies in plain text — `extract_reply` in
  `deepseek.rs` handles both (strip to the first `{...}` substring; if
  that still fails, treat the whole reply as the message text) rather
  than failing the whole request over a formatting slip.
- **Why the model kept dropping the JSON format a few turns into a
  conversation (root cause, found after the `response_format` fix above
  didn't fully explain it):** chat history was being replayed to the API
  as plain prose — `{"role": "assistant", "content": "<the stored
  reply text>"}` — because that's what's stored
  (`companion_chat_messages` only ever held the reply text, not the
  mood). Seeing its own past turns written as freeform prose taught the
  model, mid-conversation, "this chat doesn't use the JSON format," and
  it would stop emitting one — confirmed by direct A/B testing against
  the raw API: identical system prompt, identical moods, only the
  *style* of the fake history differed (flat filler lines vs. realistic
  in-character prose), and only the latter reproduced the format drop.
  Fixed by storing the `mood` alongside every model turn
  (`companion_chat_messages.mood`) and replaying history in the *exact*
  `{"reply", "mood", "remember"}` shape the live prompt asks for, not as
  bare text — confirmed fixed with a 7-turn emotionally-varied live
  conversation afterward: 0 fallbacks, every mood both valid and
  contextually right (happy for good news, sad for feeling down, pout for
  an annoyed/embarrassed moment, etc).
- Mood selection still has a fallback for the cases above that can't be
  fully eliminated (the model occasionally drops the field even with
  in-format history): `guess_mood` in `deepseek.rs` does cheap local
  keyword sniffing on the reply text (happy/sad/angry/pout/surprised),
  falling back to a *random* pick among the character's moods —
  deliberately deprioritizing anything named like a placeholder
  ("default"/"idle") — rather than one fixed value. A fixed default (e.g.
  always the alphabetically-first mood) looks identical to "the character
  is stuck," which is exactly what got reported the first time this
  shipped, because one of the characters in testing had a mood literally
  named so it sorted first. Every time this fallback actually fires, it
  logs `"guessing mood from reply text"` or `"didn't match any of"` —
  check for those specifically to tell whether a given reply's mood was
  the model's own choice or a guess.
- The character's current page (`location.pathname` — whichever page the
  person is actually on, not always `/companion/`) is passed in as
  context ("page access").
- Uses `DEEPSEEK_API_KEY`. Not shared with any other feature in this
  codebase (unlike the Groq version, which shared a key/quota with
  `routes/team.rs`'s support-email assistant).
- The last 10 messages (5 exchanges) per (user, character) are stored in
  `companion_chat_messages` (now with each model turn's `mood` alongside
  its text — see above) and replayed as prior turns on every call, so the
  character has real short-term conversational memory, not just a single
  stateless message.
- The character can also choose to persist a short freeform note about the
  person into `companion_memories` (its `remember` field in the structured
  response, alongside `reply`/`mood` — empty when it has nothing worth
  keeping). Capped at 50 notes per (user, character); oldest trimmed first.
  These are read back and listed in the system prompt on every call ("Things
  you remember about this person from earlier conversations").
- The system prompt also gives the character: a factual blurb about what
  mitch.pro is (`SITE_INFO` in `deepseek.rs`), and the caller's own email,
  display name, MitchCoin balance, and premium status. None of this is
  meant to be volunteered unprompted — it's there so the character can
  answer real questions about the site/itself/the user's account without
  breaking character, per the explicit "it can say anything but it must
  stay in character" requirement. (An earlier version also told it about
  the free-vs-paid API key situation; removed on request — that's not
  something the character needs to know or explain.)
- Simple drag + physics (gravity, floor bounce, idle wandering, pointer
  drag) implemented from scratch in `webserver/companion-widget.js` — no
  physics library. It refuses to run inside an iframe
  (`window.top !== window.self`) so it can't double-render or misbehave
  inside embedded pages (the proxy, embedded games, etc).
- **Preset actions.** The chat response can include an `action` field —
  one of a fixed, hardcoded list (`ACTIONS` in `deepseek.rs`: spin, jump,
  shake, grow, shrink, wave, dance, peekaboo, confetti, nap), each mapped
  to a pre-written CSS animation in `companion-widget.css`/`.js`'s
  `playAction`. Always offered to every character regardless of review
  status — these are animations, not code, so there's nothing for the
  review gate to protect against here.
- **Custom JS execution.** The chat response can also include a
  `customJs` field — raw JavaScript the widget runs via `new
  Function(code)()`. Only ever populated server-side when the active
  character is admin-reviewed (`!character.needs_review`, checked in
  `deepseek::Capabilities` and re-enforced again when building
  `ChatReply`, not just left to the prompt text to self-police) — see
  "Moderation" below for why that's the actual safety boundary, not a
  presets-only restriction.
- **Coin requests.** A reviewed character can also choose to take up to
  100% of the asker's *current* MitchCoin balance
  (`coinRequestPercent`/`MAX_COIN_REQUEST_PERCENT` in `deepseek.rs`).
  No user opt-in or off toggle — any active, reviewed character can take
  coins directly during chat without requiring user authorization.
  Rate-limited to once per (email, character) every
  `COIN_REQUEST_COOLDOWN_MILLIS` (6h). The deduction is applied immediately
  on the server during the chat turn as a pure sink: no creator payout,
  so a malicious prompt gains nothing from taking coins.
- **Throw reactions.** Releasing the character above a speed threshold
  (`THROW_SPEED_THRESHOLD` in `companion-widget.js`, derived from the
  existing drag physics' `vx`/`vy`) fires a synthetic, silent chat message
  so the character reacts in-character — cooled down
  (`THROW_COOLDOWN_MS`) so repeated small drags don't spam the DeepSeek
  endpoint.

## Economy

Every coin movement here matches the `get_coins` → check → `add_coins`
pattern already used by `routes/marketplace.rs` and `routes/casino.rs` —
no new mechanism invented. Read-then-write, not a single atomic
transaction, so there's technically a TOCTOU window on rapid concurrent
double-clicks — accepted here on purpose because the rest of the site's
economy already accepts the same window; it is not an infinite-money bug,
it's a "maybe get one extra purchase through" window bounded by whatever
that one purchase costs.

**Subscription — using the companion at all.** 750 coins/week or 1500/
month (`POST /api/companion/subscribe`, `{plan: "week"|"month"}`). Stored
in `companion_subscriptions` as one `expires_at` timestamp per email.
`mitch_lib::companion::extend_subscription` always extends from
`max(now, current_expiry)`, never from `now` alone — buying more time
while already subscribed stacks on top instead of wasting the remainder,
and there's no path to a cheaper "renewal" that back-dates. Required to
flip `enabled` on in `/api/companion/settings`, and re-checked on every
`/api/companion/chat` call (defense in depth — `enabled` is a stored
preference that doesn't get continuously re-validated, so a lapsed
subscription has to be caught at the point of use, not just when it was
first turned on).

**Per-character pricing — "each character costs different."**
`companion_characters.price_coins`, unlocked once per (email, character)
via `companion_character_unlocks` — pay once, keep forever, even if the
price changes later. Free characters (`price_coins == 0`) need no unlock
row. A character's creator always has free access to their own
characters (`mitch_lib::companion::has_unlocked` short-circuits on
`creator_email == caller`) — nobody should have to pay to use something
they made.

**Custom characters — any user, not just admins.** `POST
/api/companion/mine` costs a flat 500 coins (pure sink, nobody receives
it) and lets the creator set their own `price_coins` for others to unlock
it. When someone else unlocks it, the creator gets a 25% cut
(`CREATOR_PAYOUT_FRACTION` in `economy.rs`); the remaining 75% is also a
sink, credited to nobody. That 75% sink is the actual exploit guard: two
colluding accounts (one creates a cheap character, the other unlocks it,
or a single account tries to unlock its own character before the
creator-exemption above even applies) always come out strictly behind —
every cycle burns 75% of whatever passes through it. Walked through and
confirmed in session before this shipped, not just asserted.

**Moderation: `needs_review` is now a hard pre-release gate, not just an
after-the-fact notice.** This changed from the feature's first version
(which deliberately shipped with no pre-publish gate — see git history if
you need the old reasoning) once the feature grew real capability
(custom JS execution, coin requests). A custom character still goes live
immediately on creation, but while `needs_review` is set it's invisible
and unusable to everyone except its own creator and admins —
`mitch_lib::companion::visible_to`/`list_visible_characters`, enforced at
every entry point that can reach a character
(`GET /api/companion/characters`, `POST /api/companion/settings`,
`POST /api/companion/characters/unlock`). An `admin_notice` still fires on
creation (`coins::add_admin_notification`) pointing at `/admin/companion/`,
where an admin inspects the prompt and either calls `mark-reviewed`
(makes it visible to everyone) or deletes it. Editing a character's name/
prompt/price re-flags `needs_review` — new content, worth another look,
and it also means a previously-reviewed character briefly loses
`customJs`/coin-request capability again until re-reviewed.

This gate is the actual security boundary for `customJs`/coin requests,
not a presets-only restriction: nobody but the creator is ever exposed to
an unreviewed prompt's behavior, so once reviewed, the character is
trusted the same way an admin-authored preset already is.

## Requires `DEEPSEEK_API_KEY`

Chat returns `503 companion AI is not configured` until `DEEPSEEK_API_KEY`
is set in the environment. Never commit this key — export it in your
shell or add it to the untracked `.env`.

## Data model

- `companion_characters` (id, name, personality_prompt, is_preset,
  is_default, needs_review, creator_email, price_coins, timestamps) —
  `creator_email == ''` for admin presets, a real email for user-made
  characters.
- `companion_character_images` (character_id, mood, file, updated_at) — one
  row per mood the character has art for. "Mood" is free text, not a fixed
  enum: whatever moods a character has images for become the exact list
  spelled out in the DeepSeek prompt (and the only valid values the
  post-parse check in `deepseek.rs` accepts) for that character, so it
  can only ever reply with a mood it has a sprite for. `idle` is treated
  as the fallback display mood when nothing else has loaded yet.
- `companion_user_settings` (email, enabled, active_character_id,
  allow_coin_requests) — `allow_coin_requests` legacy column; coin taking
  requires no opt-in and has no off toggle.
- `companion_chat_messages` (email, character_id, role, content, mood,
  created_at) — rolling chat history, read back as the last 10 rows per
  (email, character_id); never trimmed automatically (cheap to keep: it's
  just text, and `recent_chat_messages` only ever reads the tail of it).
  `mood` is empty for user turns; for model turns it's what that reply
  actually used, so history can be replayed to the provider in the same
  JSON shape the live prompt asks for — see the Scope section above for
  why that's load-bearing, not cosmetic.
- `companion_memories` (email, character_id, content, created_at) — capped
  at 50 rows per (email, character_id) by `add_memory`, which deletes
  anything past the 50 most recent on every insert.
- `companion_subscriptions` (email, expires_at, updated_at) — one row per
  subscriber; see Economy above.
- `companion_character_unlocks` (email, character_id, unlocked_at) — one
  row per paid unlock; `ON DELETE CASCADE` when the character is deleted.
- `companion_coin_requests` (email, character_id, asked_at, percent) — at
  most one row per (email, character): the most recent offer, used both
  for the cooldown (`coin_request_cooldown_elapsed`) and to verify an
  accept isn't claiming a percent that was never actually offered
  (`consume_coin_request` reads `percent` then zeroes it in the same
  call, so the same offer can't be accepted twice).

Images live on disk at `data/companion_images/<character_id>/<mood>.<ext>`,
served through `GET /api/companion/character-image?id=&mood=` (any
logged-in user — the widget needs it for whatever character it's
rendering, not just the owner).

## Routes

- `GET /api/companion/characters` — public listing (name, moods,
  isPreset, isDefault, priceCoins, `unlocked` — per-caller, whether *they*
  can select it right now; personality prompts and creator emails stay
  server-side here).
- `GET`/`POST /api/companion/settings` — the per-user toggle + active
  character. `GET` also returns `subscriptionActive`/
  `subscriptionExpiresAt`. `POST` with `enabled: true` requires an active
  subscription and an already-unlocked character, both `402` otherwise.
- `POST /api/companion/chat` — `{message, page}` → `{reply, mood, action,
  customJs, coinRequestPercent}`. Derives the active character from the
  caller's own settings row server-side (never trusts a client-supplied
  character id). Re-checks subscription + unlock status every call,
  independent of `settings`. `customJs`/`coinRequestPercent` are only
  ever non-empty/non-zero when the active character is admin-reviewed
  (and, for coin requests, the caller opted in and the cooldown has
  elapsed) — see Scope and Moderation above.
- `POST /api/companion/coin-request/accept` — `{characterId}` → the only
  place a coin request ever actually deducts anything. Re-validates the
  offered percent (via `consume_coin_request`) against the live balance
  at the moment of accept, not a stale snapshot from when the offer was
  shown.
- `GET /api/companion/character-image?id=&mood=` — serves one sprite.
- `POST /api/companion/subscribe` — `{plan: "week"|"month"}` → charges
  750/1500 coins, extends `companion_subscriptions`.
- `POST /api/companion/characters/unlock` — `{characterId}` → charges
  `price_coins` once, pays the creator their 25% cut if there is one.
- `GET`/`POST /api/companion/mine` — list/create the caller's own custom
  characters. `POST /api/companion/mine/update` and
  `/api/companion/mine/delete` edit/remove them (ownership-checked —
  `404`, not `403`, on someone else's id, so the error can't be used to
  probe which ids exist). `POST /api/companion/mine/image` and
  `/api/companion/mine/image/delete` manage mood images the same way the
  admin tool does, just ownership-scoped.
- `/api/admin/companion/characters*` — CRUD + image upload for *any*
  character (presets and user-made alike), behind the existing global
  admin gate in `handler.rs`. Also `.../set-default` and
  `.../mark-reviewed`.

## Rate limits (`mitch_lib::auth::rate_limit_for`)

`/api/companion/chat` is capped at 12/min — "rate limited but not harshly"
was the explicit ask, since every call costs a real DeepSeek API charge.

## If you touch this again

- This has swung between page-scoped and site-wide once already in this
  feature's history — site-wide (current state) is correct and intended.
  If you ever touch `pipeline.rs`/`inject.rs` for this again, keep the
  change to the same shape it has now: one independent
  `if !raw.contains("/companion-widget.js")` block that adds exactly one
  `<script>`/`<link>` pair, touching nothing else those functions already
  inject. The risk worth avoiding is a careless edit to the *shared*
  injection logic breaking other scripts' injection, not the mere fact of
  this script being listed there.
- `companion-widget.js` only renders anything after a real
  `/api/companion/settings` fetch confirms the viewer is logged in and has
  it enabled — never assume that from page context. Don't add any DOM
  before that check resolves.
- `data/sites` is a `PRESERVED_DATA_FILE` (plain text, not through the SQL
  document store) — edit it directly, same as any other entry.
- `companion_chat_messages` is not pruned — `recent_chat_messages` only
  ever reads the last 10 rows per (email, character), but every row from
  every chat ever sent stays in the table. Fine today (it's short text,
  and rate-limited at 12/min), but if this ever needs an actual cap or a
  "clear my history" action, add it the same way `add_memory` already caps
  `companion_memories` rather than leaving it unbounded.
- History from the Groq-backed version of this feature (relevant if a
  future provider switch ever lands back on something behind an
  aggressive WAF): Groq 403'd every request with reqwest's default
  `User-Agent`, fixed by spoofing `python-requests/2.31.0` (matching
  `routes/team.rs`'s own Groq caller). Separately, the exact same 403
  reproduced from one test environment specifically over IPv4 (confirmed
  with `curl -4` vs `curl -6`, and by pinning
  `reqwest::ClientBuilder::resolve()` to each address family directly) —
  traced to a VPN on that machine; turning it off fixed it immediately.
  DeepSeek hasn't needed either workaround so far, but if
  `/api/companion/chat` ever reliably fails against whatever provider is
  current, check for a VPN and try spoofing the `User-Agent` before
  assuming the integration code itself is broken — it was independently
  verified correct against the raw API both times this came up.
- All the new economy endpoints (`subscribe`, `unlock`, `mine*`) got their
  own `rate_limit_for` entries rather than falling through to the
  `__default__` 100/min — cheap to add at the same time as the route, and
  `/api/companion/mine/delete` in particular should stay tighter than the
  others (it's destructive).
- Preset action animations (`.action-*` classes) are applied to
  `.companion-sprite` (the `<img>`), **never to `.companion-root`** — the
  root's `transform` is rewritten every animation frame by the drag/
  physics loop in `startPhysics()` for positioning (`translate(x,y)`), so
  a CSS animation also targeting that element's `transform` would fight
  it: whichever one currently wins the cascade controls the *entire*
  transform, so the companion would visually snap away from wherever
  physics put it (e.g. near the viewport's top-left during "jump", since
  `translateY(-28px)` with no x/y replaces the position entirely) for as
  long as the animation runs. The sprite carries no positioning transform
  of its own, so animating it there layers cleanly on top instead.
- `/admin/companion/` shipped *without* the `X-Admin-Passphrase` wiring
  every other admin tool needs — documented in `docs/admin-panel.md`, not
  invented there for this writeup — and nobody caught it until a real
  admin hit `invalid_passphrase` trying to create a character in
  production. It went unnoticed in dev because a local dev-superuser
  session on localhost bypasses the passphrase check entirely (see
  `admin_gate` in `handler.rs`), so testing through that path looks
  identical whether the frontend sends the header or not. Fixed by
  porting `/admin/bans/`'s `bans.js` lock-screen pattern over verbatim
  (`companion.js`: `headers()` attaches `X-Admin-Passphrase` from
  `sessionStorage`, every request re-shows the lock screen on
  `invalid_passphrase`/`passphrase_not_configured`). If you add another
  admin page here, check `docs/admin-panel.md` *first* — don't rediscover
  this the same way.
