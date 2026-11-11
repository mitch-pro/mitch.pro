# Voice and video calls in the chat client

Calls in `/matrix/` (the chat client, Cinny) run on our own backend. Voice rooms and call buttons in DMs use the client's normal UI; there are no separate "voice channels" to join. Audio and camera video go over a WebSocket on the site's own origin, so it works on every mirror domain and any network that can load the page. No WebRTC, SFU or TURN. (Element Call + LiveKit was tried first and failed with "could not establish pc connection" because only 443/80 are reachable and TURN would need a subdomain on every mirror domain.)

## How a call starts

1. The client opens a call for a Matrix room (a voice room, or the call button in a DM or small room) by loading `/matrix/public/element-call/index.html` in an iframe, exactly as it did for Element Call. That page is ours (`call.js`, `call.css`, `voice-client.js` next to it); the client's `config.json` still points at that URL.
2. The client drives the page with the Matrix **widget API** (postMessage). `call.js` answers the handshake and asks only for: send its own `org.matrix.msc3401.call.member` state event, receive that type, and send/receive `org.matrix.msc4075.rtc.notification`. The client's allowlist approves these.
3. The page reads the user's Matrix token from `localStorage.cinny_access_token` (same origin) and calls `POST /api/voice/call {room_id, mx_token}`.
4. The server (`routes/voice.rs`) checks that token with Conduit: `whoami` must map (via `email_for_user_id`) to the signed-in site account, and `joined_members` must include them. It returns a one-time `ticket` (60 s, bound to account and channel).
5. The page opens `GET /api/voice/ws?channel=room:<room id>&ticket=…`. **The backend room is created here**, the first time anyone joins, and removed when the last person leaves. So any voice room or DM the client calls gets its own backend room without any setup.
6. The page publishes the membership state event (`_<user>_<device>_m.call`, re-sent every 20 s with a 90 s expiry so a closed tab drops out) so the client lists who is in the call, and sends `io.element.join`. When it is the one starting a call (`sendNotificationType` param) it also sends an `rtc.notification` referencing that membership event, which makes the client show its built-in incoming-call banner (with ring tone, Answer/Reject) to the other members.
7. Mute, camera and hang-up come from the client's own controls over the widget API (`io.element.device_mute` carries `audio_enabled` and `video_enabled`, `im.vector.hangup` ends the call); the page replies with the real state, and on hang-up sends an empty membership event and `io.element.close`.

Remote audio plays through an `<audio>` element in the page because the client's deafen button works by muting those.

The `#voice-general` room (see `OFFICIAL_ROOMS` / `official_room_create_payload` in `routes/matrix.rs`) is created as a Matrix call room (type `org.matrix.msc3417.call`, no guests, call events open to every member), so it shows up as a voice room in the client's space. Anyone can also create their own voice room from the client.

## Wire format

Binary messages start with a type byte. Audio is 16 kHz mono signed 16-bit little-endian PCM, 20 ms (640 byte) frames.

- client → server: `[1][pcm]` audio; `[2][codec][flags][data]` video (flags bit 0 = keyframe); text `{"muted":bool}`, `{"video":bool}` (ask to turn the camera on/off), `{"keyframe":true}` (a decoder needs a keyframe).
- server → client: `[1][u32 BE sender id][pcm]`; `[2][u32 BE sender id][codec][flags][data]`; text `{"type":"hello","id"}` once, `{"type":"roster","people":[{id,name,muted,video}]}` on every change, `{"type":"video","ok","on"}` answering a camera request, `{"type":"keyframe"}` asking a camera to send a keyframe.

The server only fans frames out to the others in the room; it never mixes, decodes or stores media, and membership is in memory only. The page sends audio only while loud (plus ~200 ms) and never while muted; each remote speaker gets its own jitter-buffered player (60 ms prebuffer, 400 ms cap).

## Video

- **Capture:** 320x240 at about 15 fps (cover-cropped from the camera), encoded with WebCodecs `VideoEncoder` as VP8, else H.264 (Annex B), else JPEG frames at about 8 fps (works in any browser). A keyframe goes out every 2 s and on request. Codec ids: 0 JPEG, 1 VP8, 2 H.264.
- **Display:** each remote camera is decoded (`VideoDecoder`, or `createImageBitmap` for JPEG) onto its own canvas shown in that person's tile. Your own tile shows the camera preview. A browser that can't decode the sender's codec shows the avatar and "can't show video here".
- **Keyframes:** a decoder can start only on a keyframe, and a dropped delta breaks the picture until the next one. The server asks all cameras for a keyframe (at most every 500 ms) when someone joins, when a listener's video queue overflows, or when a decoder reports an error.
- **Audio first:** video has its own broadcast channel (16 frames) that the connection task polls after audio. A slow listener drops video frames and requests a keyframe; audio is never delayed or dropped because of video. The sender also skips frames while its socket buffer is backed up.
- **Limits:** 6 cameras on per call (`MAX_VIDEO_SENDERS`; the server refuses the 7th and the page says so), frames up to 128 KB, about 250 KB/s per sender. A 320x240 VP8 stream is roughly 40-60 KB/s, sent to every other listener, so keep camera calls small.
- Not done: screen share (the client's button does nothing), adaptive quality, simulcast, higher resolutions.

## Rules enforced server-side

- Signed-in, non-banned account (re-checked every 2 s while connected).
- Same-origin only (`hosts::same_origin_request`), so another site can't open the socket with a visitor's cookies.
- A valid ticket, which requires proof of membership in that Matrix room.
- One seat per account per room: joining again (e.g. a second tab) closes the first with code 4000.
- 16 people per call (`MAX_PEERS`), close code 4001 when full.
- Audio frames over 2048 bytes are dropped; senders are limited to 60 audio frames/s (50 is real time). Video frames are accepted only while the server has allowed that person's camera, and are limited by size and a per-sender byte budget.

Bandwidth is about 32 KB/s per active speaker, sent to every other listener, so keep calls small.

## Not covered

Screen share: the client's button does nothing. No end-to-end encryption of call audio or video (it passes through the server, like the rest of the site). The client's own code is a prebuilt bundle, so anything beyond the widget contract above (what it renders, when it rings) is the client's behavior.
