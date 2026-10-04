// Fallback voice calling over a plain WebSocket, for when LiveKit's WebRTC
// (direct UDP/TCP, or the TURN/TLS fallback) can't get through at all —
// restrictive wifi, a VPN, networks where even TURN:443 is blocked. This
// rides over the exact same WebSocket transport the site already proves
// reliable everywhere, at the cost of being more sensitive to packet loss
// than real WebRTC (TCP head-of-line blocking vs. RTP's graceful drop) and
// audio-only for now. It's independent of Cinny's own UI (which is a
// vendored, prebuilt bundle not meant to be hand-edited) — a small floating
// button injected on top.
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
    setStatus('connecting…');
    try {
      stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    } catch (err) {
      setStatus('mic denied');
      joined = false;
      return;
    }
    ws = new WebSocket(relayUrl(roomId));
    ws.binaryType = 'arraybuffer';
    ws.addEventListener('open', () => {
      setStatus('live (fallback)');
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

  // ── minimal floating UI ───────────────────────────────────────────────
  let statusEl = null;
  function setStatus(text) {
    if (!statusEl) return;
    if (text) {
      statusEl.textContent = text;
      statusEl.hidden = false;
    } else {
      statusEl.hidden = true;
    }
  }

  function mount() {
    const wrap = document.createElement('div');
    wrap.style.cssText =
      'position:fixed;left:12px;bottom:12px;z-index:9999;display:flex;' +
      'align-items:center;gap:8px;font:12px system-ui,sans-serif;';
    const btn = document.createElement('button');
    btn.textContent = '☎️ Fallback call';
    btn.title = 'Audio call over WebSocket — use if the normal call has no sound';
    btn.style.cssText =
      'padding:6px 10px;border-radius:6px;border:1px solid #444;' +
      'background:#1a1a1a;color:#eee;cursor:pointer;';
    statusEl = document.createElement('span');
    statusEl.hidden = true;
    statusEl.style.cssText = 'color:#9ca3af;';
    btn.addEventListener('click', () => {
      if (joined) {
        leave();
        btn.textContent = '☎️ Fallback call';
        return;
      }
      const roomId = typeof window.mitchGetActiveRoomIdentifier === 'function'
        ? window.mitchGetActiveRoomIdentifier()
        : '';
      if (!roomId) {
        setStatus('open a room first');
        setTimeout(() => setStatus(null), 2000);
        return;
      }
      join(roomId);
      btn.textContent = '☎️ End fallback call';
    });
    wrap.appendChild(btn);
    wrap.appendChild(statusEl);
    document.body.appendChild(wrap);
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', mount, { once: true });
  } else {
    mount();
  }
})();
