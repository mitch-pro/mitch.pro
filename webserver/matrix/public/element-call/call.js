/* Voice call page for the chat client.
 *
 * The chat client (Cinny) opens this page in an iframe the same way it used to
 * open Element Call, and drives it with the Matrix widget API over
 * postMessage. This page:
 *   - answers the widget handshake and asks for the few permissions it needs,
 *   - joins the call for the room it was opened in through our own backend
 *     (POST /api/voice/call, then the audio WebSocket; see voice-client.js),
 *   - publishes the room's call-membership state event so the client shows who
 *     is in the call, and sends the standard call notification so other
 *     members get the client's built-in ringing banner,
 *   - obeys the client's mute / hang-up requests and reports back.
 * See docs/voice-channels.md.
 */
(function () {
  'use strict';
  var q = new URLSearchParams(location.search);
  var P = {
    widgetId: q.get('widgetId') || '',
    parentUrl: q.get('parentUrl') || location.origin,
    roomId: q.get('roomId') || '',
    userId: q.get('userId') || '',
    deviceId: q.get('deviceId') || '',
    notify: q.get('sendNotificationType') || '',   // "ring" | "notification" when this opens a new call
    intent: q.get('intent') || '',                  // e.g. start_call_dm_voice; no "_voice" suffix = video call
    theme: q.get('theme') || 'dark'
  };
  var MEMBER = 'org.matrix.msc3401.call.member';
  var NOTIFY = 'org.matrix.msc4075.rtc.notification';
  var MEMBER_KEY = '_' + P.userId + '_' + P.deviceId + '_m.call';
  var HEARTBEAT_MS = 20000;
  var MEMBER_EXPIRES_MS = 90000;     // re-sent every HEARTBEAT_MS; a crashed tab drops out after this

  var CALL_INTENT = /_voice$/.test(P.intent) ? 'audio' : 'video';
  var statusEl = document.getElementById('status');
  var noteEl = document.getElementById('note');
  var tilesEl = document.getElementById('tiles');
  var closeBtn = document.getElementById('close');

  var session = null;        // voice-client session
  var started = false, ended = false, muted = false;
  var heartbeat = null;
  var seq = 0;
  var pending = {};

  function setTheme(t) { document.body.className = t === 'light' ? 'theme-light' : 'theme-dark'; }
  setTheme(P.theme);

  function setStatus(msg, isError) {
    statusEl.textContent = msg || '';
    statusEl.className = 'status' + (isError ? ' error' : '');
    statusEl.hidden = !msg;
  }

  // ── widget transport ──────────────────────────────────────────────────────
  function post(msg) { window.parent.postMessage(msg, P.parentUrl); }

  function request(action, data) {
    var id = 'mitch-' + (++seq) + '-' + Math.random().toString(36).slice(2, 8);
    return new Promise(function (resolve, reject) {
      pending[id] = { resolve: resolve, reject: reject };
      post({ api: 'fromWidget', widgetId: P.widgetId, requestId: id, action: action, data: data || {} });
      setTimeout(function () {
        if (pending[id]) { delete pending[id]; reject(new Error('The chat client did not respond (' + action + ')')); }
      }, 15000);
    });
  }
  function reply(msg, response) { post(Object.assign({}, msg, { response: response || {} })); }

  window.addEventListener('message', function (ev) {
    var m = ev.data;
    if (ev.source !== window.parent || !m || m.widgetId !== P.widgetId) return;
    if (m.api === 'fromWidget' && m.response !== undefined) {
      var p = pending[m.requestId];
      if (!p) return;
      delete pending[m.requestId];
      if (m.response && m.response.error) p.reject(new Error(m.response.error.message || 'Request refused'));
      else p.resolve(m.response);
    } else if (m.api === 'toWidget' && m.response === undefined) {
      onHostRequest(m);
    }
  });

  function capabilities() {
    return [
      'org.matrix.msc2762.timeline:' + P.roomId,
      'org.matrix.msc2762.send.state_event:' + MEMBER + '#' + MEMBER_KEY,
      'org.matrix.msc2762.receive.state_event:' + MEMBER,
      'org.matrix.msc2762.send.event:' + NOTIFY,
      'org.matrix.msc2762.receive.event:' + NOTIFY
    ];
  }

  function onHostRequest(m) {
    switch (m.action) {
      case 'capabilities':
        reply(m, { capabilities: capabilities() });
        break;
      case 'notify_capabilities':
        reply(m, {});
        begin();
        break;
      case 'io.element.device_mute': {
        var d = m.data || {};
        if (typeof d.audio_enabled === 'boolean') setMuted(!d.audio_enabled, true);
        var wantVideo = typeof d.video_enabled === 'boolean' ? d.video_enabled : cameraOn();
        setVideo(wantVideo).then(function (actual) {
          reply(m, { audio_enabled: !muted, video_enabled: actual });
          reportMute();
        });
        break;
      }
      case 'im.vector.hangup':
        reply(m, {});
        hangup();
        break;
      case 'theme_change':
        setTheme(m.data && m.data.name);
        reply(m, {});
        break;
      default:
        reply(m, {});
    }
  }

  function cameraOn() { return !!(session && session.videoOn); }

  // Turns the camera on or off; resolves to whether it is on afterwards.
  function setVideo(on) {
    if (!session) return Promise.resolve(false);
    return session.setVideo(on).then(function (actual) { render(); return actual; });
  }

  function reportMute() {
    request('io.element.device_mute', { audio_enabled: !muted, video_enabled: cameraOn() }).catch(function () {});
  }

  // ── call lifecycle ────────────────────────────────────────────────────────
  function memberContent() {
    return {
      application: 'm.call',
      call_id: '',
      scope: 'm.room',
      device_id: P.deviceId,
      expires: MEMBER_EXPIRES_MS,
      created_ts: Date.now(),
      focus_active: { type: 'livekit', focus_selection: 'oldest_membership' },
      foci_preferred: [],
      'm.call.intent': CALL_INTENT
    };
  }
  function sendMember(content) {
    return request('send_event', { type: MEMBER, state_key: MEMBER_KEY, content: content });
  }

  function mxToken() {
    try { return localStorage.getItem('cinny_access_token') || ''; } catch (_) { return ''; }
  }

  async function begin() {
    if (started) return;
    started = true;
    try {
      if (!MitchVoice.supported()) throw new Error('This browser does not support voice calls.');
      var token = mxToken();
      if (!token) throw new Error('Sign in to chat to start a call.');
      var r = await fetch('/api/voice/call', {
        method: 'POST',
        credentials: 'include',
        headers: { 'Content-Type': 'application/json', 'X-Mitch-Requested-With': '1' },
        body: JSON.stringify({ room_id: P.roomId, mx_token: token })
      });
      var data = await r.json().catch(function () { return {}; });
      if (!r.ok) throw new Error(data.error || 'Could not join the call (' + r.status + ')');
      session = await MitchVoice.connect({
        channel: data.channel,
        ticket: data.ticket,
        onChange: render,
        onClose: function (reason) { endedByServer(reason); }
      });
      setMuted(muted);
      var res = await sendMember(memberContent());
      if (P.notify && res && res.event_id) {
        request('send_event', {
          type: NOTIFY,
          content: {
            'm.mentions': { user_ids: [], room: true },
            notification_type: P.notify,
            'm.relates_to': { event_id: res.event_id, rel_type: 'm.reference' },
            sender_ts: Date.now(),
            lifetime: 30000,
            'm.call.intent': CALL_INTENT
          }
        }).catch(function () {});
      }
      heartbeat = setInterval(function () { sendMember(memberContent()).catch(function () {}); }, HEARTBEAT_MS);
      setStatus('');
      request('io.element.join', {}).catch(function () {});
      reportMute();
      render();
    } catch (e) {
      failed((e && e.message) || 'Could not join the call.');
    }
  }

  function failed(msg) {
    if (session) { session.leave(); session = null; }
    setStatus(msg, true);
    closeBtn.hidden = false;
    closeBtn.onclick = function () { request('io.element.close', {}).catch(function () {}); };
  }

  function stopCall() {
    if (ended) return false;
    ended = true;
    if (heartbeat) clearInterval(heartbeat);
    if (session) { session.leave(); session = null; }
    // An empty membership event is how a call member leaves.
    if (started) sendMember({}).catch(function () {});
    return true;
  }

  function hangup() {
    if (!stopCall()) return;
    render();
    setTimeout(function () { request('io.element.close', {}).catch(function () {}); }, 150);
  }

  function endedByServer(reason) {
    if (!stopCall()) return;
    session = null;
    setStatus(reason ? reason + '.' : 'The call ended.', true);
    render();
    setTimeout(function () { request('io.element.close', {}).catch(function () {}); }, 2500);
  }

  function setMuted(m, fromHost) {
    muted = !!m;
    if (session) session.setMuted(muted);
    if (!fromHost) reportMute();
  }

  // ── UI ────────────────────────────────────────────────────────────────────
  function render() {
    tilesEl.textContent = '';
    noteEl.textContent = session && session.videoError ? session.videoError : '';
    noteEl.hidden = !noteEl.textContent;
    if (!session) return;
    session.roster.forEach(function (p) {
      var me = p.id === session.myId;
      var talking = me ? session.talking : session.isSpeaking(p.id);
      var li = document.createElement('li');
      li.className = 'tile' + (talking && !p.muted ? ' speaking' : '');
      // Our own picture is the camera preview; others are decoded canvases.
      var pic = me ? (session.videoOn ? session.previewEl : null) : session.videoCanvas(p.id);
      var sub = document.createElement('div');
      sub.className = 'sub';
      if (pic) {
        li.classList.add('has-video');
        pic.classList.add('video');
        pic.classList.toggle('mirror', me);
        li.appendChild(pic);
        if (me && pic.play) pic.play().catch(function () {});
      } else {
        var av = document.createElement('div');
        av.className = 'avatar';
        av.textContent = (p.name || '?').trim().charAt(0).toUpperCase() || '?';
        li.appendChild(av);
        if (p.video && !me && session.videoUnsupported(p.id)) sub.textContent = "can't show video here";
      }
      var nm = document.createElement('div');
      nm.className = 'name';
      nm.textContent = p.name + (me ? ' (you)' : '');
      if (!sub.textContent && p.muted) sub.textContent = 'muted';
      li.appendChild(nm); li.appendChild(sub);
      tilesEl.appendChild(li);
    });
  }

  document.querySelector('[data-testid="incall_leave"]').addEventListener('click', hangup);
  window.addEventListener('beforeunload', function () { if (!ended) stopCall(); });

  // Tell the client we're ready; it answers by asking for capabilities.
  request('content_loaded', {}).catch(function () {});
  // If the client never confirms the capabilities, start anyway after a moment.
  setTimeout(function () { if (!started) begin(); }, 8000);
})();
