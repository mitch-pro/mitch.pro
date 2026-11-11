# Voice channels

`/voice/` hosts audio-only voice channels (Lounge, Gaming, Study). It replaces the MatrixRTC / Element Call path in `/matrix`, which never connected and whose call UI is not wired up in the bundled client.

## How it works

- The page loads the vendored `livekit-client` (`webserver/voice/livekit-client.umd.js`, Apache-2.0, license alongside) and joins a LiveKit room straight from the browser. No Matrix involvement.
- Backend: `rust/crates/mitch-server/src/routes/voice.rs`.
  - `GET /api/voice/channels` lists the channels and who is in each (LiveKit `ListParticipants`, best effort; an unreachable SFU shows empty channels).
  - `POST /api/voice/token` `{channel}` returns `{url, token}`.
- Both require a signed-in, non-banned account. Tokens are scoped to one fixed room id (`voice-*`), microphone audio only, no data channel, 6 h TTL. Unlike the legacy anonymous `/livekit/get_token`, a caller can't pick an arbitrary room.
- The client connects to `wss://<host>/livekit/rtc`, which `routes/livekit.rs` proxies to the SFU (ban checks apply while connected).
- Channels are the `CHANNELS` constant in `voice.rs`; add a row there to add a channel.

## Network requirements

Signaling goes through the site on 443. Media (WebRTC) goes to the SFU directly over UDP 7882 / TCP 7881 (`docker-compose.yml`). Only 443/80 are forwarded to the host, so clients that can't reach those ports need a working TURN relay. LiveKit's `rtc.turn_servers` entries take `host`, `port`, `protocol`, `username` and `credential`; the `secret` field currently in `docker-compose.yml` is not a LiveKit option, so that relay is not in effect. If calls connect but have no audio, or fail with a peer connection error, this is the first place to look.

## Updating the library

`npm pack livekit-client@<version>` and copy `dist/livekit-client.umd.js` and `LICENSE` into `webserver/voice/`.
