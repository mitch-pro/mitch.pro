/* Voice engine for the chat client's call page (/matrix/public/element-call/).
 *
 * Audio is 16 kHz mono signed 16-bit PCM in 20 ms frames, relayed by the
 * server over a WebSocket (see docs/voice-channels.md).
 *
 *   MitchVoice.supported()
 *   MitchVoice.connect({ channel, ticket, onChange, onClose }) -> Promise<session>
 *
 * Remote audio is played through an <audio> element in this document, because
 * the chat client mutes/unmutes "sound" by toggling every <audio> in the frame.
 *
 * session: { myId, roster, muted, talking, videoOn, videoError, previewEl,
 *            isSpeaking(id), setMuted(bool), setVideo(bool) -> Promise<bool>,
 *            videoCanvas(id), videoUnsupported(id), leave() }
 * onChange() fires when the roster, mute state or who is talking changes.
 * onClose(reason) fires if the connection ends without leave() being called.
 */
(function (global) {
  'use strict';
  var RATE = 16000, FRAME = 320;
  var SEND_RMS = 0.012, HANGOVER_FRAMES = 10;   // keep sending ~200 ms past the last loud frame
  var SPEAK_MS = 350;

  // Video: 320x240, ~15 fps, a keyframe every 2 s. Encoded with WebCodecs
  // (VP8, else H.264) when available, else as JPEG frames. Frames go to the
  // server as [2][codec][flags][data]; flags bit 0 = keyframe.
  var VW = 320, VH = 240, VFPS = 15, VBITRATE = 350000, KEY_EVERY = 30;
  var JPEG_FPS = 8, JPEG_QUALITY = 0.5;
  var CODEC_JPEG = 0, CODEC_VP8 = 1, CODEC_H264 = 2;
  var MAX_SEND_BUFFER = 200000, MAX_VIDEO_BYTES = 128 * 1024;

  // Mic: downsample to 16 kHz, cut into frames, report each frame's loudness.
  // Speaker: one jitter-buffered player per remote speaker.
  var WORKLET = [
    'class Capture extends AudioWorkletProcessor {',
    '  constructor(){ super(); this.step = sampleRate / ' + RATE + '; this.pos = 0; this.acc = 0; this.n = 0;',
    '    this.buf = new Int16Array(' + FRAME + '); this.i = 0; this.sq = 0; }',
    '  process(inputs){ var ch = inputs[0] && inputs[0][0]; if(!ch) return true;',
    '    for (var k = 0; k < ch.length; k++){ this.acc += ch[k]; this.n++; this.pos++;',
    '      if (this.pos >= this.step){ this.pos -= this.step; var v = this.acc / this.n; this.acc = 0; this.n = 0;',
    '        v = Math.max(-1, Math.min(1, v)); this.sq += v*v; this.buf[this.i++] = v < 0 ? v*32768 : v*32767;',
    '        if (this.i === ' + FRAME + '){ this.port.postMessage({ pcm: this.buf.buffer, rms: Math.sqrt(this.sq/' + FRAME + ') }, [this.buf.buffer]);',
    '          this.buf = new Int16Array(' + FRAME + '); this.i = 0; this.sq = 0; } } }',
    '    return true; }',
    '}',
    'registerProcessor("capture", Capture);',
    'class Player extends AudioWorkletProcessor {',
    '  constructor(){ super(); this.reset(); this.step = ' + RATE + ' / sampleRate;',
    '    this.port.onmessage = (e) => {',
    '      if (e.data === "reset") { this.reset(); return; }',
    '      this.q.push(e.data); this.queued += e.data.length;',
    '      while (this.queued > ' + (RATE * 0.4) + ' && this.q.length){ this.queued -= this.q.shift().length; } }; }',
    '  reset(){ this.q = []; this.cur = null; this.ci = 0; this.queued = 0; this.playing = false; this.frac = 0; this.prev = 0; this.last = 0; }',
    '  next(){ if (!this.cur || this.ci >= this.cur.length){ this.cur = this.q.shift() || null; this.ci = 0;',
    '      if (this.cur) this.queued -= this.cur.length; } return this.cur ? this.cur[this.ci++] : null; }',
    '  process(inputs, outputs){ var out = outputs[0] && outputs[0][0]; if (!out) return true;',
    '    // Always write every sample: some browsers hand back the previous block',
    '    // instead of silence, which would replay the last bit of audio forever.',
    '    out.fill(0);',
    '    if (!this.playing){ if (this.queued < ' + (RATE * 0.06) + ') return true; this.playing = true; }',
    '    for (var k = 0; k < out.length; k++){ this.frac += this.step;',
    '      while (this.frac >= 1){ this.frac -= 1; var s = this.next();',
    '        if (s === null){ this.playing = false; this.frac = 0; this.prev = 0; this.last = 0; return true; }',
    '        this.prev = this.last; this.last = s; }',
    '      out[k] = this.prev + (this.last - this.prev) * this.frac; }',
    '    return true; }',
    '}',
    'registerProcessor("player", Player);'
  ].join('\n');

  function supported() {
    return !!(global.AudioWorkletNode && navigator.mediaDevices && navigator.mediaDevices.getUserMedia && global.WebSocket);
  }

  function teardownRaw(s) {
    stopCamera(s);
    Object.keys(s.remote || {}).forEach(function (id) { closeRemote(s, id); });
    try { if (s.ws) { s.ws.onclose = null; s.ws.close(); } } catch (_) {}
    try { if (s.stream) s.stream.getTracks().forEach(function (t) { t.stop(); }); } catch (_) {}
    try { if (s.audioEl) { s.audioEl.srcObject = null; s.audioEl.remove(); } } catch (_) {}
    try { if (s.ctx) s.ctx.close(); } catch (_) {}
  }

  async function connect(opts) {
    var s = {
      myId: 0, roster: [], inCall: null, muted: false, talking: false, closed: false,
      players: {}, speakingAt: {}, hang: 0,
      videoOn: false, videoError: '', previewEl: null, remote: {}
    };
    var changed = function () { if (opts.onChange && !s.closed) opts.onChange(); };
    s.isSpeaking = function (id) { return Date.now() - (s.speakingAt[id] || 0) < SPEAK_MS; };
    s.leave = function () { if (s.closed) return; s.closed = true; teardownRaw(s); };
    s.setMuted = function (m) {
      s.muted = !!m;
      if (s.muted) { s.talking = false; s.hang = 0; }
      if (s.ws && s.ws.readyState === 1) s.ws.send(JSON.stringify({ muted: s.muted }));
      changed();
    };
    s.videoCanvas = function (id) { var r = s.remote[id]; return r && r.live ? r.canvas : null; };
    s.videoUnsupported = function (id) { var r = s.remote[id]; return !!(r && r.bad); };
    s.setVideo = async function (on) {
      if (s.closed || !s.ws || s.ws.readyState !== 1) return false;
      if (!on) {
        var was = s.videoOn;
        stopCamera(s);
        if (was) { s.ws.send(JSON.stringify({ video: false })); changed(); }
        return false;
      }
      if (s.videoOn) return true;
      s.videoError = '';
      var cam;
      try {
        cam = await navigator.mediaDevices.getUserMedia({
          video: { width: { ideal: 640 }, height: { ideal: 480 }, frameRate: { ideal: 15 }, facingMode: 'user' }
        });
      } catch (e) {
        s.videoError = e && e.name === 'NotAllowedError' ? 'Camera access was blocked.' : 'No camera found.';
        changed();
        return false;
      }
      // The server decides whether another camera fits in this call.
      var ok = await new Promise(function (resolve) {
        s.videoAck = resolve;
        s.ws.send(JSON.stringify({ video: true }));
        setTimeout(function () { if (s.videoAck === resolve) { s.videoAck = null; resolve(false); } }, 4000);
      });
      if (!ok || s.closed) {
        cam.getTracks().forEach(function (t) { t.stop(); });
        if (!s.closed) { s.videoError = 'Too many cameras are on in this call.'; changed(); }
        return false;
      }
      s.videoOn = true;
      startCamera(s, cam, changed);
      changed();
      return true;
    };
    try {
      s.stream = await navigator.mediaDevices.getUserMedia({
        audio: { channelCount: 1, echoCancellation: true, noiseSuppression: true, autoGainControl: true }
      });
      s.ctx = new (global.AudioContext || global.webkitAudioContext)({ latencyHint: 'interactive' });
      s.dest = s.ctx.createMediaStreamDestination();
      s.audioEl = document.createElement('audio');
      s.audioEl.autoplay = true;
      s.audioEl.srcObject = s.dest.stream;
      document.body.appendChild(s.audioEl);
      var url = URL.createObjectURL(new Blob([WORKLET], { type: 'text/javascript' }));
      await s.ctx.audioWorklet.addModule(url);
      URL.revokeObjectURL(url);
      await s.ctx.resume();
      try { await s.audioEl.play(); } catch (_) {}
      var scheme = location.protocol === 'https:' ? 'wss://' : 'ws://';
      var q = '?channel=' + encodeURIComponent(opts.channel) + (opts.ticket ? '&ticket=' + encodeURIComponent(opts.ticket) : '');
      s.ws = new WebSocket(scheme + location.host + '/api/voice/ws' + q);
      s.ws.binaryType = 'arraybuffer';
      await new Promise(function (resolve, reject) {
        var settled = false;
        s.ws.onopen = function () { settled = true; resolve(); };
        s.ws.onerror = function () { if (!settled) reject(new Error('Could not reach the voice server.')); };
        s.ws.onclose = function (ev) { if (!settled) reject(new Error(ev.reason || 'Could not join the call.')); };
      });
      s.ws.onmessage = function (ev) { onMessage(s, ev, changed); };
      s.ws.onclose = function (ev) {
        if (s.closed) return;
        s.closed = true;
        teardownRaw(s);
        if (opts.onClose) opts.onClose(ev.reason || '');
      };
      s.capture = new AudioWorkletNode(s.ctx, 'capture', { numberOfOutputs: 0 });
      s.ctx.createMediaStreamSource(s.stream).connect(s.capture);
      s.capture.port.onmessage = function (e) {
        var loud = e.data.rms > SEND_RMS;
        s.hang = loud ? HANGOVER_FRAMES : Math.max(0, s.hang - 1);
        var talking = !s.muted && s.hang > 0;
        if (talking !== s.talking) { s.talking = talking; changed(); }
        if (!talking || s.ws.readyState !== 1 || s.ws.bufferedAmount > 65536) return;
        var out = new Uint8Array(1 + e.data.pcm.byteLength);
        out[0] = 1;
        out.set(new Uint8Array(e.data.pcm), 1);
        s.ws.send(out);
      };
      // Redraw when who is speaking changes.
      s.timer = setInterval(function () {
        if (s.closed) return clearInterval(s.timer);
        var key = Object.keys(s.speakingAt).filter(function (k) { return s.isSpeaking(k); }).join(',');
        if (key !== s.lastKey) { s.lastKey = key; changed(); }
      }, 150);
      return s;
    } catch (e) {
      s.closed = true;
      teardownRaw(s);
      if (e && e.name === 'NotAllowedError') throw new Error('Microphone access was blocked. Allow it in your browser and try again.');
      if (e && e.name === 'NotFoundError') throw new Error('No microphone found.');
      throw e;
    }
  }

  function playerFor(s, id) {
    var p = s.players[id];
    if (!p) {
      p = s.players[id] = new AudioWorkletNode(s.ctx, 'player', { numberOfInputs: 0, outputChannelCount: [1] });
      p.connect(s.dest);
    }
    return p;
  }

  // ── video send ─────────────────────────────────────────────────────────────
  async function pickVideoCodec() {
    if (global.VideoEncoder && global.VideoFrame) {
      var base = { width: VW, height: VH, bitrate: VBITRATE, framerate: VFPS, latencyMode: 'realtime' };
      var tries = [
        { id: CODEC_VP8, config: Object.assign({ codec: 'vp8' }, base) },
        { id: CODEC_H264, config: Object.assign({ codec: 'avc1.42001e', avc: { format: 'annexb' } }, base) }
      ];
      for (var i = 0; i < tries.length; i++) {
        try {
          var r = await global.VideoEncoder.isConfigSupported(tries[i].config);
          if (r && r.supported) return tries[i];
        } catch (_) {}
      }
    }
    return { id: CODEC_JPEG };
  }

  function sendVideo(s, codec, key, bytes) {
    var len = bytes.byteLength;
    if (!s.ws || s.ws.readyState !== 1 || len + 3 > MAX_VIDEO_BYTES) return;
    var out = new Uint8Array(3 + len);
    out[0] = 2; out[1] = codec; out[2] = key ? 1 : 0;
    out.set(bytes, 3);
    s.ws.send(out);
  }

  function startJpegTimer(s, v, cv, cx) {
    if (s.videoTimer) clearInterval(s.videoTimer);
    s.vcodec = CODEC_JPEG;
    s.videoTimer = setInterval(function () { captureFrame(s, v, cv, cx); }, 1000 / JPEG_FPS);
  }

  async function startCamera(s, cam, changed) {
    s.camStream = cam;
    var v = document.createElement('video');
    v.muted = true; v.playsInline = true; v.autoplay = true; v.srcObject = cam;
    s.previewEl = v;
    try { await v.play(); } catch (_) {}
    var cv = document.createElement('canvas');
    cv.width = VW; cv.height = VH;
    var cx = cv.getContext('2d');
    var codec = await pickVideoCodec();
    if (s.closed || s.camStream !== cam) return;
    s.forceKey = true; s.vframe = 0;
    if (codec.id === CODEC_JPEG) {
      startJpegTimer(s, v, cv, cx);
    } else {
      try {
        s.encoder = new global.VideoEncoder({
          output: function (chunk) {
            var b = new Uint8Array(chunk.byteLength);
            chunk.copyTo(b);
            sendVideo(s, codec.id, chunk.type === 'key', b);
          },
          error: function () {
            // The encoder gave up; JPEG always works.
            try { s.encoder.close(); } catch (_) {}
            s.encoder = null;
            startJpegTimer(s, v, cv, cx);
          }
        });
        s.encoder.configure(codec.config);
        s.vcodec = codec.id;
        s.videoTimer = setInterval(function () { captureFrame(s, v, cv, cx); }, 1000 / VFPS);
      } catch (_) {
        s.encoder = null;
        startJpegTimer(s, v, cv, cx);
      }
    }
    changed();
  }

  function captureFrame(s, v, cv, cx) {
    var vw = v.videoWidth, vh = v.videoHeight;
    if (!vw || !s.ws || s.ws.readyState !== 1) return;
    // A backed-up connection: skip the frame and resync with a keyframe later.
    if (s.ws.bufferedAmount > MAX_SEND_BUFFER) { s.forceKey = true; return; }
    var scale = Math.max(VW / vw, VH / vh), dw = vw * scale, dh = vh * scale;
    cx.drawImage(v, (VW - dw) / 2, (VH - dh) / 2, dw, dh);
    if (s.vcodec === CODEC_JPEG) {
      if (s.jpegBusy) return;
      s.jpegBusy = true;
      cv.toBlob(function (blob) {
        s.jpegBusy = false;
        if (blob) blob.arrayBuffer().then(function (ab) { sendVideo(s, CODEC_JPEG, true, new Uint8Array(ab)); });
      }, 'image/jpeg', JPEG_QUALITY);
      return;
    }
    if (!s.encoder || s.encoder.encodeQueueSize > 4) return;
    var key = s.forceKey || (s.vframe++ % KEY_EVERY === 0);
    s.forceKey = false;
    var frame = new global.VideoFrame(cv, { timestamp: Math.round(performance.now() * 1000) });
    try { s.encoder.encode(frame, { keyFrame: key }); } catch (_) {} finally { frame.close(); }
  }

  function stopCamera(s) {
    s.videoOn = false;
    if (s.videoTimer) { clearInterval(s.videoTimer); s.videoTimer = null; }
    try { if (s.encoder) s.encoder.close(); } catch (_) {}
    s.encoder = null;
    try { if (s.camStream) s.camStream.getTracks().forEach(function (t) { t.stop(); }); } catch (_) {}
    s.camStream = null;
    if (s.previewEl) { try { s.previewEl.srcObject = null; } catch (_) {} s.previewEl = null; }
  }

  // ── video receive ──────────────────────────────────────────────────────────
  function remoteFor(s, id) {
    var r = s.remote[id];
    if (!r) {
      var cv = document.createElement('canvas');
      cv.width = VW; cv.height = VH; cv.className = 'video';
      r = s.remote[id] = { canvas: cv, ctx: cv.getContext('2d'), decoder: null, codec: -1, ts: 0, live: false, bad: false, askedAt: 0 };
    }
    return r;
  }

  function closeRemote(s, id) {
    var r = s.remote[id];
    if (!r) return;
    try { if (r.decoder) r.decoder.close(); } catch (_) {}
    delete s.remote[id];
  }

  function askKeyframe(s, r) {
    var now = Date.now();
    if (now - r.askedAt < 1000 || !s.ws || s.ws.readyState !== 1) return;
    r.askedAt = now;
    s.ws.send(JSON.stringify({ keyframe: true }));
  }

  function drawn(r, changed) {
    if (!r.live) { r.live = true; changed(); }
  }

  function onVideo(s, id, codec, key, data, changed) {
    var r = remoteFor(s, id);
    if (codec === CODEC_JPEG) {
      if (!global.createImageBitmap) { r.bad = true; return; }
      createImageBitmap(new Blob([data], { type: 'image/jpeg' })).then(function (bmp) {
        r.ctx.drawImage(bmp, 0, 0, VW, VH);
        if (bmp.close) bmp.close();
        drawn(r, changed);
      }).catch(function () {});
      return;
    }
    if (!global.VideoDecoder || !global.EncodedVideoChunk) {
      if (!r.bad) { r.bad = true; changed(); }
      return;
    }
    if (!r.decoder || r.codec !== codec) {
      if (!key) { askKeyframe(s, r); return; }
      try { if (r.decoder) r.decoder.close(); } catch (_) {}
      r.codec = codec;
      r.decoder = new global.VideoDecoder({
        output: function (frame) {
          r.ctx.drawImage(frame, 0, 0, VW, VH);
          frame.close();
          drawn(r, changed);
        },
        error: function () { r.decoder = null; askKeyframe(s, r); }
      });
      try {
        r.decoder.configure({ codec: codec === CODEC_VP8 ? 'vp8' : 'avc1.42001e' });
      } catch (_) {
        r.decoder = null;
        if (!r.bad) { r.bad = true; changed(); }
        return;
      }
    }
    if (r.decoder.decodeQueueSize > 10) { askKeyframe(s, r); return; }
    try {
      r.decoder.decode(new global.EncodedVideoChunk({ type: key ? 'key' : 'delta', timestamp: (r.ts += 66666), data: data }));
    } catch (_) { r.decoder = null; askKeyframe(s, r); }
  }

  // ── messages ───────────────────────────────────────────────────────────────
  function onMessage(s, ev, changed) {
    if (typeof ev.data === 'string') {
      var m = JSON.parse(ev.data);
      if (m.type === 'hello') { s.myId = m.id; }
      else if (m.type === 'roster') {
        s.roster = m.people;
        var live = {}, cams = {};
        m.people.forEach(function (p) { live[p.id] = 1; if (p.video) cams[p.id] = 1; });
        s.inCall = live;
        Object.keys(s.players).forEach(function (id) {
          if (!live[id]) {
            // Empty the player before dropping it so nothing of theirs lingers.
            try { s.players[id].port.postMessage('reset'); } catch (_) {}
            s.players[id].disconnect();
            delete s.players[id];
            delete s.speakingAt[id];
          }
        });
        Object.keys(s.remote).forEach(function (id) {
          if (!live[id]) closeRemote(s, id);
          else if (!cams[id]) {
            // Camera turned off: forget the picture so a restart begins on a keyframe.
            var r = s.remote[id];
            r.live = false;
            try { if (r.decoder) r.decoder.close(); } catch (_) {}
            r.decoder = null;
          }
        });
        changed();
      } else if (m.type === 'video') {
        if (s.videoAck) { var ack = s.videoAck; s.videoAck = null; ack(!!m.ok && !!m.on); }
      } else if (m.type === 'keyframe') {
        s.forceKey = true;
      }
      return;
    }
    var dv = new DataView(ev.data);
    var type = dv.getUint8(0);
    if (type === 1) {
      // [1][u32 sender][pcm]
      if (dv.byteLength <= 5) return;
      var id = dv.getUint32(1);
      // A late frame from someone who has already left must not bring their
      // player back.
      if (!s.inCall || !s.inCall[id]) return;
      var n = (dv.byteLength - 5) >> 1;
      var f = new Float32Array(n);
      for (var i = 0; i < n; i++) f[i] = dv.getInt16(5 + i * 2, true) / 32768;
      s.speakingAt[id] = Date.now();
      playerFor(s, id).port.postMessage(f);
    } else if (type === 2) {
      // [2][u32 sender][codec][flags][data]
      if (dv.byteLength <= 8) return;
      onVideo(s, dv.getUint32(1), dv.getUint8(5), (dv.getUint8(6) & 1) === 1, new Uint8Array(ev.data, 7), changed);
    }
  }

  global.MitchVoice = { supported: supported, connect: connect };
})(window);
