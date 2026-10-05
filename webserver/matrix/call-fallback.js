// Voice calling over a plain WebSocket, replacing Cinny's native LiveKit
// call button rather than adding a second one next to it. LiveKit's WebRTC
// path (direct UDP/TCP, or a TURN/TLS fallback) needs some UDP- or
// TURN-reachable path to work at all, and that's been failing on the
// networks actually being tested from — so this rides over the exact same
// plain WebSocket/TCP connection the site already reaches everywhere, at
// the cost of being more sensitive to packet loss than real WebRTC (TCP
// head-of-line blocking vs. RTP's graceful frame drop), and audio-only.
//
// Cinny's own call button lives inside its vendored, prebuilt JS bundle —
// not something safe to hand-edit or reliably call into. Instead this
// intercepts clicks on it at the document level, in the capture phase
// (runs before React's own bubble-phase listener ever sees the event, so
// `stopImmediatePropagation` here reliably stops Cinny's own handler —
// and therefore the broken Element Call iframe — from firing at all), and
// substitutes this module's join/leave instead. The button itself has no
// stable aria-label/class (CSS-in-JS hashes change per build), so it's
// identified by its SVG icon path data, which only changes if Cinny's own
// call icon artwork does.
//
// Protocol: client -> server frames are raw MediaRecorder output chunks
// (audio/webm;codecs=opus), sent as-is. Server -> client frames are the same
// chunks with an 8-byte little-endian connection-id prefix added by the
// relay, so a receiver with more than one remote participant can demux back
// into one MediaSource per sender instead of interleaving two encoders'
// output into one stream.
//
// Known limitation: a participant who joins after someone else's
// MediaRecorder has already started misses that sender's WebM init segment
// and can't decode their audio until that sender's recorder restarts (which
// happens automatically every RESTART_INTERVAL_MS specifically so a late
// joiner self-heals within that window, at the cost of a short gap).
(function () {
  const MIME_TYPE = 'audio/webm;codecs=opus';
  const CHUNK_MS = 200;
  const RESTART_INTERVAL_MS = 15000;

  if (!window.MediaRecorder || !window.MediaSource || !MediaRecorder.isTypeSupported(MIME_TYPE)) {
    return; // Silently absent on browsers that can't support this path.
  }

  let ws = null;
  let stream = null;
  let recorder = null;
  let restartTimer = null;
  let joined = false;
  let epoch = 0; // bumped on every join/leave so a stale async join() from
  // before a fast leave() (e.g. clicked again while the mic-permission
  // prompt was still up) can tell it's been superseded and bail out.
  const peers = new Map(); // senderId (string) -> { mediaSource, sourceBuffer, audioEl, queue }

  function relayUrl(roomId) {
    const scheme = location.protocol === 'https:' ? 'wss:' : 'ws:';
    return `${scheme}//${location.host}/calls/ws/${encodeURIComponent(roomId)}`;
  }

  function teardownPeer(senderId) {
    const peer = peers.get(senderId);
    if (!peer) return;
    peers.delete(senderId);
    try { peer.audioEl.pause(); } catch (_) {}
    peer.audioEl.remove();
    try {
      if (peer.mediaSource.readyState === 'open') peer.mediaSource.endOfStream();
    } catch (_) {}
  }

  function getOrCreatePeer(senderId) {
    let peer = peers.get(senderId);
    if (peer) return peer;
    const mediaSource = new MediaSource();
    const audioEl = document.createElement('audio');
    audioEl.autoplay = true;
    audioEl.style.display = 'none';
    audioEl.dataset.mitchCallPeer = senderId;
    document.body.appendChild(audioEl);
    audioEl.src = URL.createObjectURL(mediaSource);
    peer = { mediaSource, sourceBuffer: null, audioEl, queue: [], sawInit: false };
    peers.set(senderId, peer);
    mediaSource.addEventListener('sourceopen', () => {
      if (peer.sourceBuffer) return;
      try {
        peer.sourceBuffer = mediaSource.addSourceBuffer(MIME_TYPE);
      } catch (_) {
        return;
      }
      peer.sourceBuffer.addEventListener('updateend', () => drainPeerQueue(peer));
      drainPeerQueue(peer);
    });
    return peer;
  }

  function drainPeerQueue(peer) {
    if (!peer.sourceBuffer || peer.sourceBuffer.updating) return;
    const next = peer.queue.shift();
    if (!next) return;
    try {
      peer.sourceBuffer.appendBuffer(next);
    } catch (_) {
      // A full/erroring buffer here just drops this chunk — the stream
      // self-heals on the sender's next init-segment restart.
    }
  }

  function handleRemoteFrame(buf) {
    if (buf.byteLength < 9) return;
    const view = new DataView(buf);
    const senderId = view.getBigUint64(0, true).toString();
    const chunk = buf.slice(8);
    const peer = getOrCreatePeer(senderId);
    peer.queue.push(chunk);
    drainPeerQueue(peer);
  }

  async function join(roomId) {
    if (joined) return;
    joined = true;
    const myEpoch = ++epoch;
    setStatus('connecting…');
    let mic;
    try {
      mic = await navigator.mediaDevices.getUserMedia({ audio: true });
    } catch (err) {
      if (myEpoch === epoch) {
        setStatus('mic denied');
        joined = false;
      }
      return;
    }
    if (myEpoch !== epoch) {
      // Superseded by a leave() (or another join()) while the permission
      // prompt was up — this join lost the race, so just release the mic.
      mic.getTracks().forEach((t) => t.stop());
      return;
    }
    stream = mic;
    ws = new WebSocket(relayUrl(roomId));
    ws.binaryType = 'arraybuffer';
    ws.addEventListener('open', () => {
      setStatus('live');
      startRecorder();
    });
    ws.addEventListener('message', (ev) => {
      if (ev.data instanceof ArrayBuffer) handleRemoteFrame(ev.data);
    });
    ws.addEventListener('close', () => leave());
    ws.addEventListener('error', () => leave());
  }

  function startRecorder() {
    if (!stream) return;
    recorder = new MediaRecorder(stream, { mimeType: MIME_TYPE });
    recorder.addEventListener('dataavailable', (ev) => {
      if (ev.data.size > 0 && ws && ws.readyState === WebSocket.OPEN) {
        ev.data.arrayBuffer().then((buf) => {
          if (ws && ws.readyState === WebSocket.OPEN) ws.send(buf);
        });
      }
    });
    recorder.start(CHUNK_MS);
    // Periodic restart re-emits a fresh WebM init segment, so a late
    // joiner (or anyone whose buffer got dropped) resyncs within this
    // window instead of staying silent for the rest of the call.
    clearInterval(restartTimer);
    restartTimer = setInterval(() => {
      if (recorder && recorder.state === 'recording') {
        recorder.stop();
        recorder.start(CHUNK_MS);
      }
    }, RESTART_INTERVAL_MS);
  }

  function leave() {
    if (!joined) return;
    joined = false;
    epoch++; // invalidate any in-flight join() past its getUserMedia await
    clearInterval(restartTimer);
    restartTimer = null;
    try { recorder && recorder.stop(); } catch (_) {}
    recorder = null;
    if (stream) {
      stream.getTracks().forEach((t) => t.stop());
      stream = null;
    }
    if (ws) {
      try { ws.close(); } catch (_) {}
      ws = null;
    }
    for (const senderId of Array.from(peers.keys())) teardownPeer(senderId);
    setStatus(null);
  }

  // ── wiring into Cinny's own call button ─────────────────────────────────
  // Fingerprint: the first path segment of Cinny's call-icon SVG. Confirmed
  // live against the deployed build (room header, tooltip reads "Call").
  const CALL_ICON_FINGERPRINT = 'M3.5 17L2 17L2 7L3.5 7L3.5 17';
  const LIVE_BG = '#dc2626';

  function findCallButton() {
    return Array.from(document.querySelectorAll('button')).find((b) => {
      const svg = b.querySelector('svg');
      return svg && svg.innerHTML.includes(CALL_ICON_FINGERPRINT);
    }) || null;
  }

  function setStatus(text) {
    const btn = findCallButton();
    if (!btn) return;
    if (text) {
      btn.title = text;
      btn.style.setProperty('background-color', LIVE_BG, 'important');
    } else {
      btn.title = '';
      btn.style.removeProperty('background-color');
    }
  }

  document.addEventListener(
    'click',
    (event) => {
      const btn = event.target.closest('button');
      if (!btn) return;
      const svg = btn.querySelector('svg');
      if (!svg || !svg.innerHTML.includes(CALL_ICON_FINGERPRINT)) return;
      // Capture phase, ahead of React's own bubble-phase listener —
      // this reliably keeps Cinny's native handler (and the LiveKit/
      // Element Call iframe it would open) from ever running.
      event.preventDefault();
      event.stopImmediatePropagation();
      event.stopPropagation();

      if (joined) {
        leave();
        return;
      }
      const roomId = typeof window.mitchGetActiveRoomIdentifier === 'function'
        ? window.mitchGetActiveRoomIdentifier()
        : '';
      if (!roomId) {
        setStatus('Open a room first');
        setTimeout(() => setStatus(null), 2000);
        return;
      }
      join(roomId);
    },
    true,
  );
})();
