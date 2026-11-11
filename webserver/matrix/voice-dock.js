/* Voice for the chat page: join voice channels, call the room you have open,
 * and answer incoming calls. The chat client itself is a prebuilt bundle, so
 * this adds a small dock on top of it. Audio engine: /voice/voice-client.js,
 * server: docs/voice-channels.md. */
(function () {
  'use strict';
  if (!window.MitchVoice || !MitchVoice.supported()) return;

  var POLL_MS = 5000;
  var dock, panel, toggleBtn, ringEl, ringAudio;
  var channels = [];
  var session = null;       // active MitchVoice session
  var activeLabel = '';     // what we're connected to ("Lounge", "this chat")
  var busy = '';            // key of the join in progress
  var roomId = '';          // Matrix room currently open in the client
  var rings = [];           // incoming calls
  var message = '';
  var open = false;
  var signedIn = true;

  function el(tag, cls, text) {
    var n = document.createElement(tag);
    if (cls) n.className = cls;
    if (text != null) n.textContent = text;
    return n;
  }
  function btn(label, cls, fn) {
    var b = el('button', 'vd-btn' + (cls ? ' ' + cls : ''), label);
    b.type = 'button';
    b.addEventListener('click', fn);
    return b;
  }

  function mxToken() {
    try { return localStorage.getItem('cinny_access_token') || ''; } catch (_) { return ''; }
  }
  function currentRoomId() {
    var m;
    try { m = decodeURIComponent(location.pathname).match(/(![^/?#]+:[^/?#]+)/); } catch (_) { m = null; }
    return m ? m[1] : '';
  }
  function api(path, body) {
    var opts = { credentials: 'include' };
    if (body) {
      opts.method = 'POST';
      opts.headers = { 'Content-Type': 'application/json', 'X-Mitch-Requested-With': '1' };
      opts.body = JSON.stringify(body);
    }
    return fetch(path, opts).then(function (r) {
      return r.json().catch(function () { return {}; }).then(function (data) {
        if (!r.ok) { var e = new Error(data.error || 'Request failed'); e.status = r.status; throw e; }
        return data;
      });
    });
  }

  function build() {
    dock = el('div', 'vd');
    dock.id = 'mitch-voice-dock';
    toggleBtn = el('button', 'vd-toggle');
    toggleBtn.type = 'button';
    toggleBtn.setAttribute('aria-label', 'Voice');
    toggleBtn.addEventListener('click', function () { open = !open; render(); });
    panel = el('div', 'vd-panel');
    panel.hidden = true;
    ringEl = el('div', 'vd-rings');
    ringEl.setAttribute('role', 'alert');
    dock.appendChild(ringEl);
    dock.appendChild(panel);
    dock.appendChild(toggleBtn);
    document.body.appendChild(dock);
  }

  function renderRoster(box) {
    var ul = el('ul', 'vd-people');
    session.roster.forEach(function (p) {
      var me = p.id === session.myId;
      var talking = me ? session.talking : session.isSpeaking(p.id);
      var li = el('li', talking && !p.muted ? 'speaking' : '');
      li.appendChild(el('span', 'vd-dot'));
      li.appendChild(el('span', '', p.name + (me ? ' (you)' : '')));
      if (p.muted) li.appendChild(el('span', 'vd-muted', 'muted'));
      ul.appendChild(li);
    });
    box.appendChild(ul);
  }

  function render() {
    if (!dock) return;
    dock.classList.toggle('in-call', !!session);
    toggleBtn.textContent = session ? '● In call' : 'Voice';
    panel.hidden = !open;
    ringEl.textContent = '';
    rings.forEach(function (r) {
      var card = el('div', 'vd-ring');
      card.appendChild(el('span', 'vd-ring-text', r.from + ' is calling'));
      card.appendChild(btn('Answer', 'primary', function () { answer(r); }));
      card.appendChild(btn('Decline', '', function () { decline(r); }));
      ringEl.appendChild(card);
    });
    if (!open) return;
    panel.textContent = '';
    if (message) panel.appendChild(el('p', 'vd-msg', message));

    if (session) {
      var head = el('div', 'vd-head');
      head.appendChild(el('b', '', activeLabel));
      panel.appendChild(head);
      renderRoster(panel);
      var bar = el('div', 'vd-controls');
      bar.appendChild(btn(session.muted ? 'Unmute' : 'Mute', '', function () { session.setMuted(!session.muted); }));
      bar.appendChild(btn('Leave', 'danger', leave));
      panel.appendChild(bar);
      return;
    }

    if (roomId && mxToken()) {
      var call = btn(busy === 'room' ? 'Calling…' : 'Call this chat', 'primary wide', callRoom);
      call.disabled = !!busy;
      panel.appendChild(call);
    }
    panel.appendChild(el('div', 'vd-section', 'Voice channels'));
    channels.forEach(function (ch) {
      var row = el('div', 'vd-row');
      var label = el('span', 'vd-row-name', ch.name);
      row.appendChild(label);
      row.appendChild(el('span', 'vd-row-count', ch.participants.length ? ch.participants.join(', ') : ''));
      var j = btn(busy === ch.id ? 'Joining…' : 'Join', '', function () { joinChannel(ch); });
      j.disabled = !!busy;
      row.appendChild(j);
      panel.appendChild(row);
    });
  }

  function setMessage(m) { message = m || ''; render(); }

  function connect(opts, label) {
    return MitchVoice.connect({
      channel: opts.channel,
      ticket: opts.ticket,
      onChange: render,
      onClose: function (reason) {
        session = null;
        setMessage(reason ? reason + '.' : 'Disconnected from the call.');
        open = true;
        render();
      }
    }).then(function (s) {
      session = s;
      activeLabel = label;
      open = true;
      message = '';
    });
  }

  function joinChannel(ch) {
    if (busy) return;
    busy = ch.id; setMessage('');
    connect({ channel: ch.id }, ch.name)
      .catch(function (e) { message = (e && e.message) || 'Could not connect'; })
      .then(function () { busy = ''; render(); });
  }

  function startRoomCall(id, label) {
    busy = 'room'; message = ''; render();
    return api('/api/voice/call', { room_id: id, mx_token: mxToken() })
      .then(function (r) { return connect({ channel: r.channel, ticket: r.ticket }, label); })
      .catch(function (e) { message = (e && e.message) || 'Could not start the call'; open = true; })
      .then(function () { busy = ''; render(); });
  }
  function callRoom() { if (!busy && roomId) startRoomCall(roomId, 'This chat'); }

  function answer(r) {
    rings = rings.filter(function (x) { return x !== r; });
    stopRing();
    if (session) leave();
    if (busy) return;
    startRoomCall(r.room_id, r.from);
  }
  function decline(r) {
    rings = rings.filter(function (x) { return x !== r; });
    stopRing();
    render();
    api('/api/voice/decline', { room_id: r.room_id }).catch(function () {});
  }
  function leave() {
    if (session) session.leave();
    session = null;
    setMessage('');
  }

  // Ring tone: the chat client's own call sound, looped while a call rings.
  function startRing() {
    if (ringAudio) return;
    try {
      ringAudio = new Audio('/matrix/assets/call-H6UnaODb.ogg');
      ringAudio.loop = true;
      var p = ringAudio.play();
      if (p && p.catch) p.catch(function () {});
    } catch (_) {}
  }
  function stopRing() {
    if (!ringAudio || rings.length) return;
    try { ringAudio.pause(); } catch (_) {}
    ringAudio = null;
  }

  function poll() {
    if (document.hidden || !signedIn) return;
    api('/api/voice/incoming').then(function (d) {
      var next = (d.calls || []).filter(function () { return !session; });
      var key = function (l) { return l.map(function (c) { return c.room_id; }).join('|'); };
      if (key(next) !== key(rings)) {
        rings = next;
        rings.length ? startRing() : stopRing();
        render();
      }
    }).catch(function (e) { if (e.status === 401) signedIn = false; });
    if (!session && !busy) {
      api('/api/voice/channels').then(function (d) { channels = d.channels || []; if (open) render(); })
        .catch(function () {});
    }
  }

  function watchRoute() {
    var id = currentRoomId();
    if (id !== roomId) { roomId = id; if (open) render(); }
  }

  build();
  render();
  poll();
  setInterval(poll, POLL_MS);
  setInterval(watchRoute, 700);
  watchRoute();
  document.addEventListener('visibilitychange', function () { if (!document.hidden) poll(); });
  window.addEventListener('beforeunload', function () { if (session) session.leave(); });
})();
