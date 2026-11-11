# Voice calls in the chat client

Calls in `/matrix/` (the chat client, Cinny) run on our own backend. Voice rooms and call buttons in DMs use the client's normal UI; there are no separate "voice channels" to join. Audio goes over a WebSocket on the site's own origin, so it works on every mirror domain and any network that can load the page. No WebRTC, SFU or TURN. (Element Call + LiveKit was tried first and failed with "could not establish pc connection" because only 443/80 are reachable and TURN would need a subdomain on every mirror domain.)

## How a call starts

1. The client opens a call for a Matrix room (a voice room, or the call button in a DM or small room) by loading `/matrix/public/element-call/index.html` in an iframe, exactly as it did for Element Call. That page is ours (`call.js`, `call.css`, `voice-client.js` next to it); the client's `config.json` still points at that URL.
2. The client drives the page with the Matrix **widget API** (postMessage). `call.js` answers the handshake and asks only for: send its own `org.matrix.msc3401.call.member` state event, receive that type, and send/receive `org.matrix.msc4075.rtc.notification`. The client's allowlist approves these.
3. The page reads the user's Matrix token from `localStorage.cinny_access_token` (same origin) and calls `POST /api/voice/call {room_id, mx_token}`.
4. The server (`routes/voice.rs`) checks that token with Conduit: `whoami` must map (via `email_for_user_id`) to the signed-in site account, and `joined_members` must include them. It returns a one-time `ticket` (60 s, bound to account and channel).
5. The page opens `GET /api/voice/ws?channel=room:<room id>&ticket=…`. **The backend room is created here**, the first time anyone joins, and removed when the last person leaves. So any voice room or DM the client calls gets its own backend room without any setup.
6. The page publishes the membership state event (`_<user>_<device>_m.call`, re-sent every 20 s with a 90 s expiry so a closed tab drops out) so the client lists who is in the call, and sends `io.element.join`. When it is the one starting a call (`sendNotificationType` param) it also sends an `rtc.notification` referencing that membership event, which makes the client show its built-in incoming-call banner (with ring tone, Answer/Reject) to the other members.
7. Mute and hang-up come from the client's own controls over the widget API (`io.element.device_mute`, `im.vector.hangup`); the page replies, and on hang-up sends an empty membership event and `io.element.close`.

Remote audio plays through an `<audio>` element in the page because the client's deafen button works by muting those.

The `#voice-general` room (see `OFFICIAL_ROOMS` / `official_room_create_payload` in `routes/matrix.rs`) is created as a Matrix call room (type `org.matrix.msc3417.call`, no guests, call events open to every member), so it shows up as a voice room in the client's space. Anyone can also create their own voice room from the client.

## Wire format

16 kHz mono signed 16-bit little-endian PCM, 20 ms (640 byte) frames.

- client → server: binary = one frame; text `{"muted":bool}`.
- server → client: text `{"type":"hello","id"}` once, then `{"type":"roster","people":[{id,name,muted}]}` on every change; binary = `u32 BE sender id` + that sender's frame.

The server only fans frames out to the others in the room (a `tokio::sync::broadcast` per room); it never mixes or stores audio, and membership is in memory only. The page sends frames only while loud (plus ~200 ms) and never while muted; each remote speaker gets its own jitter-buffered player (60 ms prebuffer, 400 ms cap).

## Rules enforced server-side

- Signed-in, non-banned account (re-checked every 2 s while connected).
- Same-origin only (`hosts::same_origin_request`), so another site can't open the socket with a visitor's cookies.
- A valid ticket, which requires proof of membership in that Matrix room.
- One seat per account per room: joining again (e.g. a second tab) closes the first with code 4000.
- 16 people per call (`MAX_PEERS`), close code 4001 when full.
- Frames over 2048 bytes are dropped; senders are limited to 60 frames/s (50 is real time).

Bandwidth is about 32 KB/s per active speaker, sent to every other listener, so keep calls small.

## Not covered

Audio only: the client's camera and screen-share buttons do nothing. No end-to-end encryption of call audio (it passes through the server, like the rest of the site). The client's own code is a prebuilt bundle, so anything beyond the widget contract above (what it renders, when it rings) is the client's behavior.
