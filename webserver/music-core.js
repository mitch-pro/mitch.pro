/* Music player engine, shared by the follow-along card (music-widget.js),
 * the /music/ page and the pop-out window. See docs/music.md.
 *
 * One page at a time owns the <audio> element (the "host"). Everything else is
 * a remote: it shows the host's state and sends it commands over a
 * BroadcastChannel. The host is:
 *   - the pop-out window, when it is open, or
 *   - the last tab where the visitor pressed play.
 * Pages here are separate documents, so on navigation the old page's audio
 * stops and the next page resumes from the saved position (localStorage).
 */
(function () {
  'use strict';
  if (window.MitchMusic) return;

  var KEY = 'mitch.music.v1';
  var CHANNEL = 'mitch-music';
  var POPOUT_NAME = 'mitch-music-popout';
  var MAX_QUEUE = 300;
  var HOST_TIMEOUT_MS = 2500;
  var LOADING_LIMIT_MS = 40000;
  var loadingSince = 0;

  var st = {
    queue: [], index: -1, playing: false, vol: 0.9, pos: 0, dur: 0,
    role: 'idle',            // 'host' | 'remote' | 'idle'
    buffering: false, blocked: false, error: '',
    loading: false,                    // a start was asked for and sound hasn't begun yet
    isPopout: false, popoutOpen: false, remoteTs: 0,
    shuffle: false, repeat: 'off'      // repeat: 'off' | 'all' | 'one'
  };
  var listeners = [];
  var audio = null, bc = null, popWin = null;
  var lastBeat = 0, beatTimer = 0, watchTimer = 0, saveTimer = 0;
  var prefetchedFor = '', lastSessionPos = 0, queueDirty = true, inited = false;

  var tabId = '';
  try {
    tabId = sessionStorage.getItem('mitch.music.tab') || '';
    if (!tabId) { tabId = Math.random().toString(36).slice(2); sessionStorage.setItem('mitch.music.tab', tabId); }
  } catch (e) { tabId = Math.random().toString(36).slice(2); }

  function ownerId() { return st.isPopout ? 'popout' : tabId; }
  function clamp(n, a, b) { return Math.max(a, Math.min(b, n)); }
  function current() { return st.index >= 0 ? st.queue[st.index] || null : null; }
  function emit() { listeners.slice().forEach(function (fn) { try { fn(st); } catch (e) { console.error(e); } }); }
  function post(m) { try { if (bc) bc.postMessage(m); } catch (e) { /* closed */ } }

  /* ------------------------------------------------------------ storage */
  function readStore() { try { return JSON.parse(localStorage.getItem(KEY) || 'null'); } catch (e) { return null; } }
  function currentPos() { return st.role === 'host' && audio ? (audio.currentTime || 0) : st.pos; }
  function save(extra) {
    try {
      localStorage.setItem(KEY, JSON.stringify(Object.assign({
        queue: st.queue, index: st.index, pos: currentPos(), playing: st.playing,
        vol: st.vol, shuffle: st.shuffle, repeat: st.repeat, owner: ownerId(), ts: Date.now()
      }, extra || {})));
    } catch (e) { /* storage unavailable */ }
  }
  function clearStore() { try { localStorage.removeItem(KEY); } catch (e) { /* ignore */ } }

  /* ----------------------------------------------------------------- api */
  function api(path, opts) {
    opts = opts || {};
    var init = { credentials: 'include', method: opts.method || 'GET', headers: {} };
    if (opts.body !== undefined) {
      init.method = 'POST';
      init.headers['Content-Type'] = 'application/json';
      init.headers['X-Mitch-Requested-With'] = '1';
      init.body = JSON.stringify(opts.body);
    }
    return fetch(path, init).then(function (r) {
      return r.json().catch(function () { return {}; }).then(function (data) {
        if (r.status === 401) loggedOut();
        if (!r.ok) { var err = new Error(data.error || 'request failed'); err.status = r.status; throw err; }
        return data;
      });
    });
  }
  function loggedOut() {
    if (st.role === 'host') { try { audio.pause(); } catch (e) { /* ignore */ } }
    st.queue = []; st.index = -1; st.playing = false; st.role = 'idle';
    clearStore(); emit();
  }

  /* --------------------------------------------------------------- audio */
  function ensureAudio() {
    if (audio) return audio;
    audio = new Audio();
    audio.preload = 'auto';
    audio.volume = st.vol;
    audio.addEventListener('playing', function () { st.playing = true; st.loading = false; st.buffering = false; st.blocked = false; st.error = ''; changed(); });
    audio.addEventListener('pause', function () { if (!audio.ended) { st.playing = false; changed(); } });
    audio.addEventListener('waiting', function () { st.buffering = true; emit(); });
    audio.addEventListener('canplay', function () {
      st.buffering = false;
      // If a start was asked for but the browser dropped it (a play() cut short
      // by the source loading), try again now that there is data, so a song
      // never sits loaded and waiting for a second click.
      if (st.loading && audio.paused) playNow();
      emit();
    });
    audio.addEventListener('loadedmetadata', function () { if (isFinite(audio.duration) && audio.duration > 0) st.dur = audio.duration; });
    audio.addEventListener('timeupdate', onTime);
    audio.addEventListener('ended', function () { advance(true); changed(); });
    audio.addEventListener('error', onAudioError);
    return audio;
  }

  function onTime() {
    st.pos = audio.currentTime || 0;
    if (isFinite(audio.duration) && audio.duration > 0) st.dur = audio.duration;
    var t = current();
    if (t && st.pos > 6 && prefetchedFor !== t.id) {
      prefetchedFor = t.id;
      var next = st.queue[st.index + 1];
      if (next && next.id) fetch('/api/music/prefetch?id=' + encodeURIComponent(next.id), { credentials: 'include' }).catch(function () {});
      else if (next) matchTrack(next).then(function (id) { next.id = id; queueDirty = true; return fetch('/api/music/prefetch?id=' + encodeURIComponent(id), { credentials: 'include' }); }).catch(function () {});
    }
    if (Math.abs(st.pos - lastSessionPos) > 1) { lastSessionPos = st.pos; updateSessionPosition(); }
    emit();
  }

  function onAudioError() {
    var t = current();
    if (!t) return;
    // The <audio> element can't say why; ask the server.
    fetch('/api/music/prefetch?id=' + encodeURIComponent(t.id), { credentials: 'include' })
      .then(function (r) {
        if (r.status === 401) { loggedOut(); return; }
        return r.json().catch(function () { return {}; }).then(function (d) {
          st.error = (d && d.error) || 'Could not play this track';
          st.playing = false; st.buffering = false; st.loading = false; changed();
          if (st.index + 1 < st.queue.length) setTimeout(function () { if (st.error) go(st.index + 1, true); }, 1800);
        });
      }).catch(function () { st.error = 'Could not play this track'; st.playing = false; st.loading = false; changed(); });
  }

  function playNow() {
    var p = audio.play();
    if (p && p.catch) {
      p.catch(function (err) {
        if (err && err.name === 'NotAllowedError') { st.blocked = true; st.loading = false; st.playing = false; changed(); }
      });
    }
  }

  /** Finds the YouTube video for a track that came from a Spotify playlist. */
  function matchTrack(t) {
    return api('/api/music/match?title=' + encodeURIComponent(t.title) + '&artist=' + encodeURIComponent(t.artist) +
      '&duration=' + encodeURIComponent(t.duration || 0)).then(function (d) { return d.id; });
  }

  function loadTrack(i, pos, autoplay) {
    var t = st.queue[i];
    if (!t) return;
    ensureAudio();
    if (!t.id) {                       // not matched to a video yet
      st.buffering = true; st.error = ''; st.loading = !!autoplay; st.dur = t.duration || 0; audio.removeAttribute('src');
      emit();
      matchTrack(t).then(function (id) {
        if (st.queue[i] !== t) return;           // the queue moved on
        t.id = id; queueDirty = true;
        loadTrack(i, pos, autoplay);
        changed();
      }).catch(function (e) {
        if (st.queue[i] !== t) return;
        st.error = e.message || "Couldn't find that song"; st.buffering = false; st.playing = false; st.loading = false; changed();
        if (st.index + 1 < st.queue.length) setTimeout(function () { if (st.error && st.queue[st.index] === t) go(st.index + 1, true); }, 1800);
      });
      return;
    }
    prefetchedFor = '';
    st.dur = t.duration || 0;
    st.buffering = true; st.error = ''; st.blocked = false; st.loading = !!autoplay;
    audio.src = '/api/music/stream?id=' + encodeURIComponent(t.id);
    if (pos > 0) {
      audio.addEventListener('loadedmetadata', function seekOnce() {
        audio.removeEventListener('loadedmetadata', seekOnce);
        try { audio.currentTime = pos; } catch (e) { /* ignore */ }
      });
    }
    setSession(t);
    if (autoplay) playNow();
  }

  /** The song after the current one: random when shuffling, wrapping when repeating all. */
  function advance(auto) {
    if (!st.queue.length) return;
    if (auto && st.repeat === 'one') { go(st.index, true); return; }
    if (st.shuffle && st.queue.length > 1) {
      var n;
      do { n = Math.floor(Math.random() * st.queue.length); } while (n === st.index);
      go(n, true);
      return;
    }
    if (st.index + 1 >= st.queue.length && st.repeat === 'all') { go(0, true); return; }
    go(st.index + 1, true);
  }

  function go(i, autoplay) {
    if (!st.queue.length) return;
    if (i >= st.queue.length) {            // end of the queue
      audio.pause(); st.playing = false; st.pos = 0; try { audio.currentTime = 0; } catch (e) { /* ignore */ }
      return;
    }
    if (i < 0) i = 0;
    st.index = i; st.pos = 0; queueDirty = true;
    st.buffering = true; st.error = ''; st.playing = false; st.loading = !!autoplay;   // shows the spinner at once
    loadTrack(i, 0, autoplay);
  }

  /* --------------------------------------------------------- role changes */
  function becomeHost(autoplay) {
    if (st.role === 'host') return;
    ensureAudio();
    post({ k: 'claim', from: ownerId() });
    st.role = 'host';
    startTimers();
    if (st.index >= 0) loadTrack(st.index, st.pos, !!autoplay);
  }

  function releaseHost() {
    if (audio) { try { audio.pause(); } catch (e) { /* ignore */ } }
    save({ playing: false });
    st.playing = false; st.role = 'remote'; lastBeat = Date.now();
    emit();
  }

  function startTimers() {
    if (!beatTimer) beatTimer = setInterval(function () { if (st.role === 'host') beat(false); }, 500);
    if (!saveTimer) saveTimer = setInterval(function () { if (st.role === 'host' && st.playing) save(); }, 2000);
  }

  function changed() {
    // A page that has just handed playback away must not overwrite the saved
    // state (late <audio> events still arrive after pause()).
    if (st.role !== 'host') { emit(); return; }
    save(); beat(true); emit(); updateSession();
  }

  function snapshot(withQueue) {
    var s = {
      index: st.index, playing: st.playing, pos: currentPos(), dur: st.dur, vol: st.vol,
      buffering: st.buffering, loading: st.loading, blocked: st.blocked, error: st.error, ts: Date.now(),
      shuffle: st.shuffle, repeat: st.repeat
    };
    if (withQueue) s.queue = st.queue;
    return s;
  }
  function beat(force) {
    if (st.role !== 'host') return;
    post({ k: 'state', popout: st.isPopout, s: snapshot(queueDirty) });
    queueDirty = false;
  }

  function applyRemote(m) {
    var s = m.s || {};
    st.role = 'remote';
    if (s.queue) st.queue = s.queue;
    st.index = s.index; st.playing = !!s.playing; st.pos = s.pos || 0; st.dur = s.dur || 0;
    if (typeof s.vol === 'number') st.vol = s.vol;
    st.buffering = !!s.buffering; st.loading = !!s.loading; st.blocked = !!s.blocked; st.error = s.error || '';
    st.shuffle = !!s.shuffle; st.repeat = s.repeat || 'off';
    st.popoutOpen = !!m.popout; st.remoteTs = s.ts || Date.now();
    lastBeat = Date.now();
    emit();
  }

  function onMessage(e) {
    var m = e.data || {};
    if (m.k === 'state') { if (st.role !== 'host') applyRemote(m); }
    else if (m.k === 'claim') { if (st.role === 'host' && m.from !== ownerId()) releaseHost(); }
    else if (m.k === 'cmd') { if (st.role === 'host') runCmd(m.cmd, m.arg); }
    else if (m.k === 'ping') { if (st.role === 'host') { queueDirty = true; beat(true); } }
    else if (m.k === 'stop') { if (st.role !== 'host') { st.queue = []; st.index = -1; st.playing = false; st.role = 'idle'; emit(); } }
  }

  /* -------------------------------------------------------------- commands */
  function runCmd(cmd, arg) {
    switch (cmd) {
      case 'toggle':
        if (!st.queue.length) return;
        // A song that is still being fetched ignores the button, so repeated
        // presses don't start and stop it.
        if (st.loading && !st.playing && !st.error) return;
        if (audio.paused) playNow(); else audio.pause();
        break;
      case 'play': playNow(); break;
      case 'pause': audio.pause(); break;
      case 'next': advance(false); break;
      case 'shuffle': st.shuffle = !!arg; break;
      case 'repeat': st.repeat = arg === 'all' || arg === 'one' ? arg : 'off'; break;
      case 'prev': if (audio.currentTime > 3) audio.currentTime = 0; else go(st.index - 1, true); break;
      case 'seek': if (isFinite(arg)) { audio.currentTime = clamp(arg, 0, st.dur || 1e9); st.pos = audio.currentTime; } break;
      case 'vol': st.vol = clamp(arg, 0, 1); audio.volume = st.vol; break;
      case 'jump': go(arg, true); break;
      case 'playQueue':
        st.queue = trimQueue(arg.queue, arg.index);
        go(trimmedIndex(arg), true);
        break;
      case 'enqueue':
        st.queue.push(arg); queueDirty = true;
        if (st.index < 0) go(0, true);
        break;
      case 'stop': stopHost(); return;
      default: return;
    }
    changed();
  }

  function trimQueue(queue, index) {
    queue = (queue || []).filter(function (t) { return t && (t.id || t.spotifyId); });
    if (queue.length <= MAX_QUEUE) return queue;
    var start = clamp(index - 20, 0, queue.length - MAX_QUEUE);
    return queue.slice(start, start + MAX_QUEUE);
  }
  function trimmedIndex(arg) {
    var q = (arg.queue || []).filter(function (t) { return t && (t.id || t.spotifyId); });
    var start = q.length <= MAX_QUEUE ? 0 : clamp(arg.index - 20, 0, q.length - MAX_QUEUE);
    return arg.index - start;
  }

  function stopHost() {
    if (audio) { try { audio.pause(); audio.removeAttribute('src'); audio.load(); } catch (e) { /* ignore */ } }
    st.queue = []; st.index = -1; st.playing = false; st.pos = 0; st.dur = 0; st.role = 'idle';
    clearStore(); post({ k: 'stop' }); emit();
    if ('mediaSession' in navigator) { try { navigator.mediaSession.metadata = null; } catch (e) { /* ignore */ } }
  }

  /** Sends a command to the host, becoming the host first if there is none. */
  function control(cmd, arg) {
    if (st.role === 'remote') {
      if (cmd === 'playQueue' || cmd === 'jump' || cmd === 'next' || cmd === 'prev') { st.buffering = true; st.loading = true; st.error = ''; emit(); }
      post({ k: 'cmd', cmd: cmd, arg: arg });
      return;
    }
    if (st.role === 'idle') {
      if (!st.queue.length && cmd !== 'playQueue' && cmd !== 'enqueue') return;
      if (cmd === 'playQueue') st.index = -1;      // don't load the stale track first
      // The first play after a page load has to load the song and start it in
      // one go. (Loading it first and then running the toggle would see
      // "loading" and do nothing, so it took a second click.)
      if (cmd === 'toggle' || cmd === 'play') {
        becomeHost(true);
        changed();
        return;
      }
      becomeHost();
    }
    runCmd(cmd, arg);
  }

  /* --------------------------------------------------------- media session */
  function absolute(path) { return location.origin + path; }
  function setSession(t) {
    if (!('mediaSession' in navigator) || !window.MediaMetadata) return;
    try {
      navigator.mediaSession.metadata = new MediaMetadata({
        title: t.title, artist: t.artist,
        artwork: [{ src: absolute('/api/music/thumb?id=' + encodeURIComponent(t.id)), sizes: '480x360', type: 'image/jpeg' }]
      });
      var h = navigator.mediaSession.setActionHandler.bind(navigator.mediaSession);
      h('play', function () { control('play'); });
      h('pause', function () { control('pause'); });
      h('previoustrack', function () { control('prev'); });
      h('nexttrack', function () { control('next'); });
      h('seekto', function (d) { if (d && isFinite(d.seekTime)) control('seek', d.seekTime); });
    } catch (e) { /* unsupported action */ }
  }
  function updateSession() {
    if (!('mediaSession' in navigator)) return;
    try { navigator.mediaSession.playbackState = st.playing ? 'playing' : 'paused'; } catch (e) { /* ignore */ }
  }
  function updateSessionPosition() {
    if (!('mediaSession' in navigator) || !navigator.mediaSession.setPositionState) return;
    try {
      if (st.dur > 0) navigator.mediaSession.setPositionState({ duration: st.dur, position: clamp(st.pos, 0, st.dur), playbackRate: 1 });
    } catch (e) { /* ignore */ }
  }

  /* --------------------------------------------------------------- pop-out */
  function popout() {
    if (st.isPopout) return true;
    // Already open (its heartbeats are arriving): focus it instead of reloading.
    if (st.popoutOpen) {
      var existing = window.open('', POPOUT_NAME);
      if (existing) { try { existing.focus(); } catch (e) { /* ignore */ } return true; }
    }
    var pos = currentPos(), wasPlaying = st.playing;
    popWin = window.open('/music/popout/', POPOUT_NAME, 'popup=yes,width=400,height=700');
    if (!popWin) return false;                       // blocked: keep playing here
    if (st.role === 'host') {
      try { audio.pause(); } catch (e) { /* ignore */ }
      st.role = 'remote'; st.playing = false; lastBeat = Date.now();
    }
    // The pop-out reads this on load and carries on from the same spot.
    save({ owner: 'popout', playing: wasPlaying, pos: pos });
    emit();
    return true;
  }

  /* ---------------------------------------------------------------- init */
  function init(opts) {
    if (inited) return;
    inited = true;
    opts = opts || {};
    st.isPopout = !!opts.popout;
    var s = readStore();
    if (s && Array.isArray(s.queue)) {
      st.queue = s.queue; st.index = typeof s.index === 'number' ? s.index : -1;
      st.pos = s.pos || 0; if (typeof s.vol === 'number') st.vol = s.vol;
      st.shuffle = !!s.shuffle; st.repeat = s.repeat === 'all' || s.repeat === 'one' ? s.repeat : 'off';
    }
    emit();
    try {
      bc = new BroadcastChannel(CHANNEL);
      bc.onmessage = onMessage;
    } catch (e) { bc = null; }
    watchTimer = setInterval(function () {
      if (st.role === 'remote' && Date.now() - lastBeat > HOST_TIMEOUT_MS) {
        st.role = 'idle'; st.playing = false; st.loading = false; st.popoutOpen = false; emit();
      }
      // A start that never produces sound ends with a message, not a spinner forever.
      if (st.role === 'host' && st.loading) {
        if (!loadingSince) loadingSince = Date.now();
        if (Date.now() - loadingSince > LOADING_LIMIT_MS) {
          st.loading = false; st.buffering = false; st.error = 'Took too long to load. Try again.';
          loadingSince = 0; changed();
        }
      } else loadingSince = 0;
    }, 1000);
    window.addEventListener('pagehide', function () {
      if (st.role === 'host') {
        if (st.isPopout) { post({ k: 'popout-closed' }); save({ playing: false, owner: '' }); }
        else save();
      }
    });
    post({ k: 'ping' });
    // Wait briefly for an existing host to answer before deciding.
    setTimeout(function () {
      if (st.role !== 'idle') return;
      var resume = s && s.playing && s.owner === ownerId() && Date.now() - (s.ts || 0) < 60000 && st.queue.length && st.index >= 0;
      if (resume) { becomeHost(true); }
      else if (st.isPopout && st.queue.length) becomeHost();
    }, 300);
  }

  /* --------------------------------------------------------------- lyrics */
  var lyricCache = {};
  function sessionLyrics(id, value) {
    var k = 'mitch.music.lyr.' + id;
    try {
      if (value === undefined) { var raw = sessionStorage.getItem(k); return raw ? JSON.parse(raw) : null; }
      var order = JSON.parse(sessionStorage.getItem('mitch.music.lyr.order') || '[]').filter(function (x) { return x !== id; });
      order.push(id);
      while (order.length > 4) sessionStorage.removeItem('mitch.music.lyr.' + order.shift());
      sessionStorage.setItem('mitch.music.lyr.order', JSON.stringify(order));
      sessionStorage.setItem(k, JSON.stringify(value));
    } catch (e) { /* quota or unavailable */ }
    return null;
  }
  /** Lyrics for a track. `custom` ({title, artist}) searches again by hand. */
  function getLyrics(track, custom) {
    if (!track) return Promise.resolve({ found: false });
    if (!custom) {
      if (lyricCache[track.id]) return lyricCache[track.id];
      var kept = sessionLyrics(track.id);
      if (kept) return (lyricCache[track.id] = Promise.resolve(kept));
    }
    var q = '/api/music/lyrics?id=' + encodeURIComponent(track.id) +
      '&title=' + encodeURIComponent(custom ? custom.title : track.title) +
      '&artist=' + encodeURIComponent(custom ? custom.artist : track.artist) +
      '&duration=' + encodeURIComponent(track.duration || 0) + (custom ? '&custom=1' : '');
    var p = api(q).then(function (d) {
      if (!custom && d && d.found !== undefined) sessionLyrics(track.id, d);
      return d;
    }).catch(function () { delete lyricCache[track.id]; return { found: false, error: true }; });
    if (!custom) lyricCache[track.id] = p;
    return p;
  }

  /** Index of the line playing at `ms`, or -1 before the first line. */
  function lineAt(lines, ms) {
    var lo = 0, hi = lines.length - 1, ans = -1;
    while (lo <= hi) {
      var mid = (lo + hi) >> 1;
      if (lines[mid].t <= ms) { ans = mid; lo = mid + 1; } else hi = mid - 1;
    }
    return ans;
  }

  /* ----------------------------------------------------------- lyrics view */
  /** Apple Music style lyrics: the current line is large and bright, and its
   *  words fill in as they are sung. `opts.onSeek(seconds)` makes lines
   *  clickable. Call `set()` with an /api/music/lyrics response, then `tick()`
   *  with the playback time in milliseconds. */
  function createLyricsView(root, opts) {
    opts = opts || {};
    var data = null, lineEls = [], wordEls = [], active = -2, userScrollUntil = 0, wordsOf = [];
    var reduce = window.matchMedia && matchMedia('(prefers-reduced-motion: reduce)').matches;
    root.classList.add('ml-root');
    root.addEventListener('wheel', function () { userScrollUntil = Date.now() + 3500; }, { passive: true });
    root.addEventListener('touchmove', function () { userScrollUntil = Date.now() + 3500; }, { passive: true });

    function message(text) {
      root.replaceChildren();
      var p = document.createElement('p');
      p.className = 'ml-message';
      p.textContent = text;
      root.appendChild(p);
      data = null; lineEls = []; wordEls = []; active = -2;
    }

    function set(d) {
      active = -2; lineEls = []; wordEls = []; wordsOf = [];
      if (d === 'idle') { root.replaceChildren(); data = null; return; }
      if (d === 'loading') return message('Finding lyrics…');
      if (!d || !d.found || !d.lines || !d.lines.length) return message(d && d.error ? 'Could not load lyrics' : 'No lyrics found');
      data = d;
      root.replaceChildren();
      var box = document.createElement('div');
      box.className = 'ml-lines' + (d.sync === 'none' ? ' ml-plain' : '');
      d.lines.forEach(function (line, i) {
        var el = document.createElement('div');
        el.className = 'ml-line';
        var ws = [];
        if (!line.text) {
          el.classList.add('ml-gap');
          el.textContent = '• • •';
        } else if (line.words && line.words.length) {
          line.words.forEach(function (w) {
            var s = document.createElement('span');
            s.className = 'ml-w';
            s.textContent = w.text;
            el.appendChild(s);
            ws.push(s);
          });
        } else {
          el.textContent = line.text;
        }
        if (d.sync !== 'none' && opts.onSeek) {
          el.classList.add('ml-seekable');
          el.addEventListener('click', function () { opts.onSeek(line.t / 1000); });
        }
        box.appendChild(el);
        lineEls.push(el); wordEls.push(ws);
      });
      root.appendChild(box);
      root.scrollTop = 0;
      if (d.sync !== 'none') box.classList.add('ml-timed');
    }

    function paintWords(i, ms) {
      var line = data.lines[i], ws = wordEls[i];
      if (!ws.length) return;
      for (var k = 0; k < ws.length; k++) {
        var w = line.words[k];
        var p = ms >= w.e ? 1 : ms <= w.t ? 0 : (ms - w.t) / Math.max(1, w.e - w.t);
        ws[k].style.setProperty('--p', p.toFixed(3));
      }
    }

    function settle(from, to) {            // lines between a jump: fully sung or not yet
      var lo = Math.max(0, Math.min(from, to) - 1), hi = Math.min(lineEls.length - 1, Math.max(from, to) + 1);
      for (var i = lo; i <= hi; i++) {
        if (i === to) continue;
        var v = i < to ? '1' : '0';
        for (var k = 0; k < wordEls[i].length; k++) wordEls[i][k].style.setProperty('--p', v);
      }
    }

    function tick(ms) {
      if (!data || data.sync === 'none') return;
      var i = lineAt(data.lines, ms);
      if (i !== active) {
        var prev = active;
        if (prev >= 0 && lineEls[prev]) lineEls[prev].classList.remove('ml-active');
        if (i >= 0) lineEls[i].classList.add('ml-active');
        if (Math.abs(i - prev) > 1 || prev < 0) {      // seeked: repaint everything
          for (var j = 0; j < lineEls.length; j++) {
            var v = j < i ? '1' : '0';
            for (var k = 0; k < wordEls[j].length; k++) wordEls[j][k].style.setProperty('--p', v);
            lineEls[j].classList.toggle('ml-past', j < i);
          }
        } else {
          settle(prev, i);
          if (prev >= 0) lineEls[prev].classList.add('ml-past');
          if (i >= 0) lineEls[i].classList.remove('ml-past');
        }
        active = i;
        if (i >= 0 && Date.now() > userScrollUntil) {
          var el = lineEls[i];
          var top = el.offsetTop - root.clientHeight * 0.32 + el.offsetHeight / 2;
          root.scrollTo({ top: Math.max(0, top), behavior: reduce ? 'auto' : 'smooth' });
        }
      }
      if (i >= 0) paintWords(i, ms);
    }

    set(null);
    return { set: set, tick: tick, get data() { return data; } };
  }

  /* ------------------------------------------------------------------ misc */
  function getTime() {
    if (st.role === 'host' && audio) return audio.currentTime || 0;
    if (st.role === 'remote' && st.playing) return st.pos + Math.max(0, (Date.now() - st.remoteTs) / 1000);
    return st.pos;
  }
  function fmt(sec) {
    sec = Math.max(0, Math.floor(sec || 0));
    return Math.floor(sec / 60) + ':' + ('0' + (sec % 60)).slice(-2);
  }

  window.MitchMusic = {
    state: st, init: init, onChange: function (fn) { listeners.push(fn); fn(st); },
    current: current, getTime: getTime, fmt: fmt, api: api,
    thumb: function (id) { return '/api/music/thumb?id=' + encodeURIComponent(id); },
    play: function (queue, index) { control('playQueue', { queue: queue, index: index }); },
    enqueue: function (track) { control('enqueue', track); },
    toggle: function () { control('toggle'); },
    next: function () { control('next'); },
    prev: function () { control('prev'); },
    seek: function (sec) { control('seek', sec); },
    volume: function (v) { control('vol', v); },
    jump: function (i) { control('jump', i); },
    shuffle: function (on) { control('shuffle', on); },
    repeat: function (mode) { control('repeat', mode); },
    stop: function () { if (st.role === 'remote') post({ k: 'cmd', cmd: 'stop' }); else stopHost(); },
    popout: popout,
    getLyrics: getLyrics, lineAt: lineAt, createLyricsView: createLyricsView
  };
})();
