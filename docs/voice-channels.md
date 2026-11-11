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

Signaling goes through the site on 443. Media (WebRTC) is tried directly against the SFU on UDP 7882 / TCP 7881 first, then falls back to LiveKit's built-in TURN over TLS on 443, which is what works on school wifi. Without a working TURN path the browser reports "could not establish pc connection".

TURN is configured in `docker-compose.yml` (`turn:` block, `external_tls: true`): LiveKit listens for plain TCP on container port 443 (published as host `5349`), and expects something in front of it to terminate TLS for `TURN_DOMAIN` (default `turn.mitch.pro`) and advertise it. LiveKit hands clients `turns:<TURN_DOMAIN>:443` itself.

Host nginx setup (not in this repo): in the `stream` block that does SNI routing on 443, send that hostname to a TLS-terminating server that proxies to LiveKit, and get a certificate that covers it (`certbot certonly -d turn.mitch.pro`, or the existing cert if it lists the name):

```nginx
# inside the existing `map $ssl_preread_server_name $upstream { ... }`
turn.mitch.pro  127.0.0.1:8444;

# in the stream block
server {
    listen 127.0.0.1:8444 ssl;
    ssl_certificate     /etc/letsencrypt/live/turn.mitch.pro/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/turn.mitch.pro/privkey.pem;
    proxy_pass 192.168.100.15:5349;   # the docker host
}
```

No `proxy_protocol` on this route; LiveKit's TURN doesn't expect it.

## Updating the library

`npm pack livekit-client@<version>` and copy `dist/livekit-client.umd.js` and `LICENSE` into `webserver/voice/`.
