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
 * session: { myId, roster, muted, talking, isSpeaking(id), setMuted(bool), leave() }
 * onChange() fires when the roster, mute state or who is talking changes.
 * onClose(reason) fires if the connection ends without leave() being called.
 */
(function (global) {
  'use strict';
  var RATE = 16000, FRAME = 320;
  var SEND_RMS = 0.012, HANGOVER_FRAMES = 10;   // keep sending ~200 ms past the last loud frame
  var SPEAK_MS = 350;

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
    '  constructor(){ super(); this.q = []; this.cur = null; this.ci = 0; this.queued = 0; this.playing = false;',
    '    this.step = ' + RATE + ' / sampleRate; this.frac = 0; this.last = 0;',
    '    this.port.onmessage = (e) => { this.q.push(e.data); this.queued += e.data.length;',
    '      while (this.queued > ' + (RATE * 0.4) + '){ this.queued -= this.q.shift().length; } }; }',
    '  next(){ if (!this.cur || this.ci >= this.cur.length){ this.cur = this.q.shift() || null; this.ci = 0;',
    '      if (this.cur) this.queued -= this.cur.length; } return this.cur ? this.cur[this.ci++] : null; }',
    '  process(inputs, outputs){ var out = outputs[0][0];',
    '    if (!this.playing){ if (this.queued < ' + (RATE * 0.06) + ') return true; this.playing = true; }',
    '    for (var k = 0; k < out.length; k++){ this.frac += this.step;',
    '      while (this.frac >= 1){ this.frac -= 1; var s = this.next(); if (s === null){ this.playing = false; this.frac = 0; out.fill(0, k); return true; } this.prev = this.last; this.last = s; }',
    '      var p = this.prev === undefined ? this.last : this.prev; out[k] = p + (this.last - p) * this.frac; }',
    '    return true; }',
    '}',
    'registerProcessor("player", Player);'
  ].join('\n');

  function supported() {
    return !!(global.AudioWorkletNode && navigator.mediaDevices && navigator.mediaDevices.getUserMedia && global.WebSocket);
  }

  function teardownRaw(s) {
    try { if (s.ws) { s.ws.onclose = null; s.ws.close(); } } catch (_) {}
    try { if (s.stream) s.stream.getTracks().forEach(function (t) { t.stop(); }); } catch (_) {}
    try { if (s.audioEl) { s.audioEl.srcObject = null; s.audioEl.remove(); } } catch (_) {}
    try { if (s.ctx) s.ctx.close(); } catch (_) {}
  }

  async function connect(opts) {
    var s = {
      myId: 0, roster: [], muted: false, talking: false, closed: false,
      players: {}, speakingAt: {}, hang: 0
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
        s.ws.send(e.data.pcm);
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

  function onMessage(s, ev, changed) {
    if (typeof ev.data === 'string') {
      var m = JSON.parse(ev.data);
      if (m.type === 'hello') { s.myId = m.id; }
      else if (m.type === 'roster') {
        s.roster = m.people;
        var live = {};
        m.people.forEach(function (p) { live[p.id] = 1; });
        Object.keys(s.players).forEach(function (id) {
          if (!live[id]) { s.players[id].disconnect(); delete s.players[id]; delete s.speakingAt[id]; }
        });
        changed();
      }
      return;
    }
    var dv = new DataView(ev.data);
    if (dv.byteLength <= 4) return;
    var id = dv.getUint32(0);
    var n = (dv.byteLength - 4) >> 1;
    var f = new Float32Array(n);
    for (var i = 0; i < n; i++) f[i] = dv.getInt16(4 + i * 2, true) / 32768;
    s.speakingAt[id] = Date.now();
    playerFor(s, id).port.postMessage(f);
  }

  global.MitchVoice = { supported: supported, connect: connect };
})(window);
