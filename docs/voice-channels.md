# Voice channels

`/voice/` hosts audio-only voice channels (Lounge, Gaming, Study). Audio is relayed by the site's own server over a WebSocket, so it works on every mirror domain and on any network that can load the page. There is no WebRTC, SFU or TURN involved. (The earlier LiveKit version failed with "could not establish pc connection" because only 443/80 are reachable, and TURN would have needed a subdomain on every mirror domain.)

## How it works

- Backend: `rust/crates/mitch-server/src/routes/voice.rs`.
  - `GET /api/voice/channels` lists channels and who is in each.
  - `GET /api/voice/ws?channel=voice-lounge` (WebSocket) joins a channel. Dispatched from `handler.rs` next to the other post-gate upgrades.
- Wire format: 16 kHz mono signed 16-bit little-endian PCM, 20 ms (640 byte) frames.
  - client → server: binary = one frame; text `{"muted":bool}`.
  - server → client: text `{"type":"hello","id"}` once, then `{"type":"roster","people":[{id,name,muted}]}` on every change; binary = `u32 BE sender id` + that sender's frame.
- The server only fans frames out to the others in the room (a `tokio::sync::broadcast` per channel); it never mixes or stores audio. A slow listener just misses frames.
- Membership is in memory only. Nothing about who is in a channel is persisted.
- Client (`webserver/voice/index.html`): an inline AudioWorklet downsamples the mic to 16 kHz and cuts frames; frames are sent only while loud (plus ~200 ms) and never while muted. Each remote speaker gets its own jitter-buffered player worklet, upsampled to the device rate (60 ms prebuffer, 400 ms cap).

## Rules enforced server-side

- Signed-in, non-banned account (also re-checked every 2 s while connected).
- Same-origin only (`hosts::same_origin_request`), so another site can't open the socket with a visitor's cookies.
- One seat per account per channel: joining again (e.g. a second tab) closes the first with code 4000.
- 12 people per channel (`MAX_PEERS`), close code 4001 when full.
- Frames over 2048 bytes are dropped; senders are limited to 60 frames/s (50 is real time).

Bandwidth is about 32 KB/s per active speaker, sent to every other listener, so keep channels small. To add a channel, add a row to `CHANNELS` in `voice.rs`.

## In the chat page (`/matrix/`)

The chat client is a prebuilt bundle, so voice is added as a dock on top of it: `webserver/matrix/voice-dock.js` + `voice-dock.css`, loaded from `matrix/index.html`. It shares the audio engine (`webserver/voice/voice-client.js`) with `/voice/`.

- **Voice channels:** the dock lists Lounge/Gaming/Study with who is in each and a Join button.
- **Calling a chat:** when a room is open (the room id is read from the URL), "Call this chat" starts a call in it. A call is a channel named `dm:<room id>` that exists only while someone is in it.
  1. `POST /api/voice/call {room_id, mx_token}`. The server uses the caller's own Matrix token (`cinny_access_token`) against Conduit: `whoami` must map (via `email_for_user_id`) to the signed-in site account, and `joined_members` must include them. Rooms with more than 8 members are refused, so the public rooms can't be called.
  2. It records the call (members' site accounts, caller) and returns a one-time `ticket` (60 s, bound to the account and channel) that the socket requires for `dm:` channels.
- **Ringing:** the dock polls `GET /api/voice/incoming` every 5 s while the tab is visible. A call rings for the room's other members for 45 s while someone is on the line; Answer calls `/api/voice/call` for that room, Decline calls `/api/voice/decline`. The ring tone is the chat client's own call sound.
- A call and its ring state disappear when the last person leaves. All of this is in memory only.
