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
  var wantPlayUntil = 0;             // until then a paused-with-data element is started again (see canplay)

  var st = {
    queue: [], index: -1, playing: false, vol: 0.9, pos: 0, dur: 0,
    role: 'idle',            // 'host' | 'remote' | 'idle'
    buffering: false, blocked: false, error: '',
    loading: false,                    // a start was asked for and sound hasn't begun yet
    isPopout: false, popoutOpen: false, remoteTs: 0,
    crossfade: 0,                     // seconds of overlap between songs; 0 = off
    xfOk: false,                       // whether this device lets us set the volume (iOS doesn't)
    shuffle: 'off', repeat: 'off'      // shuffle: 'off' | 'random' | 'smart'; repeat: 'off' | 'all' | 'one'
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
        vol: st.vol, shuffle: st.shuffle, repeat: st.repeat, crossfade: st.crossfade, owner: ownerId(), ts: Date.now()
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
    killFade();
    if (st.role === 'host') { try { audio.pause(); } catch (e) { /* ignore */ } }
    st.queue = []; st.index = -1; st.playing = false; st.role = 'idle';
    clearStore(); emit();
  }

  /* --------------------------------------------------------------- audio */
  function ensureAudio() {
    if (audio) return audio;
    var el = audio = new Audio();
    el.preload = 'auto';
    el.volume = st.vol;
    // Only the current element may change state: during a crossfade the old one
    // is still alive and its events (pause, ended) must not touch the player.
    function on(ev, fn) { el.addEventListener(ev, function () { if (el === audio) fn(); }); }
    on('playing', function () { st.playing = true; st.loading = false; st.buffering = false; st.blocked = false; st.error = ''; changed(); maybeSuggest(); });
    on('pause', function () { if (!el.ended) { st.playing = false; changed(); } });
    on('waiting', function () { st.buffering = true; emit(); });
    on('canplay', function () {
      st.buffering = false;
      // If a start was asked for but the browser dropped it (a play() cut short
      // by the source loading), try again now that there is data, so a song
      // never sits loaded and waiting for a second click.
      // A stale 'playing' from the previous source can clear `loading`, so a
      // start asked for in the last few seconds is retried as well.
      if ((st.loading || Date.now() < wantPlayUntil) && el.paused) playNow();
      emit();
    });
    on('loadedmetadata', function () { if (isFinite(el.duration) && el.duration > 0) st.dur = el.duration; });
    on('timeupdate', onTime);
    on('ended', function () { advance(true); changed(); });
    on('error', onAudioError);
    return el;
  }

  /* ----------------------------------------------------------- crossfade */
  // The next song starts while the last seconds of this one fade out. It needs
  // two <audio> elements and control of their volume; iOS fixes the volume of
  // <audio> at 100%, so there `xfOk` is false and songs simply follow each other.
  var fade = null;                  // { old, start, ms, timer } while two songs overlap
  function volumeSettable() {
    try { var a = new Audio(); a.volume = 0.5; return a.volume === 0.5; } catch (e) { return false; }
  }
  st.xfOk = volumeSettable();
  function normFade(v) { v = Math.round(Number(v)); return st.xfOk && v > 0 ? clamp(v, 1, 12) : 0; }

  function killFade() {
    if (!fade) return;
    clearInterval(fade.timer);
    var old = fade.old;
    fade = null;
    try { old.pause(); old.removeAttribute('src'); old.load(); } catch (e) { /* ignore */ }
    if (audio) audio.volume = st.vol;
  }

  /** Near the end of a song: start the next one and fade between them. */
  function maybeCrossfade() {
    if (fade || !st.crossfade || st.role !== 'host' || !st.playing || st.repeat === 'one') return;
    var xf = st.crossfade, left = st.dur - st.pos;
    if (!(st.dur > xf * 3) || left > xf || left < 0.4) return;
    var hasNext = st.shuffle !== 'off' ? st.queue.length > 1 : st.index + 1 < st.queue.length || st.repeat === 'all';
    if (!hasNext) return;
    var old = audio;
    audio = null;
    ensureAudio();                                  // a fresh element for the next song
    audio.volume = 0;
    var me = { old: old, start: Date.now(), ms: xf * 1000, timer: 0 };
    fade = me;
    advance(true);                                  // loads and starts it
    me.timer = setInterval(function () {
      if (fade !== me) return;
      var p = clamp((Date.now() - me.start) / me.ms, 0, 1);
      try { me.old.volume = st.vol * Math.cos(p * Math.PI / 2); } catch (e) { /* ignore */ }
      if (audio) audio.volume = st.vol * Math.sin(p * Math.PI / 2);
      if (p >= 1) killFade();
    }, 80);
  }

  // Gets the next few songs ready while the current one plays: matched to a
  // video if they came from Spotify, their stream link resolved, and (for songs
  // in the listener's library) the server starts saving them. Each queue entry is
  // warmed once.
  var UPCOMING = 3;
  var warmed = typeof WeakSet === 'function' ? new WeakSet() : null;
  var warming = false;
  function warmUpcoming() {
    // One song at a time, so looking ahead never floods the server or the
    // connection the song being played needs. An entry is tried once: whether it
    // worked or not it stays marked, so a song that can't be matched (or a
    // rate limit) can't turn this into a loop of requests.
    if (warming) return;
    var next = null;
    for (var k = 1; k <= UPCOMING && !next; k++) {
      var n = st.queue[st.index + k];
      if (n && !(warmed && warmed.has(n))) next = n;
    }
    if (!next) return;
    if (warmed) warmed.add(next);
    warming = true;
    var ready = next.id ? Promise.resolve(next.id) : matchTrack(next, true).then(function (id) { next.id = id; queueDirty = true; return id; });
    ready.then(function (id) {
      return fetch('/api/music/prefetch?id=' + encodeURIComponent(id), { credentials: 'include' });
    }).catch(function () { /* best effort: not retried */ }).then(function () {
      warming = false;
      warmUpcoming();
    });
  }

  function onTime() {
    st.pos = audio.currentTime || 0;
    if (isFinite(audio.duration) && audio.duration > 0) st.dur = audio.duration;
    var t = current();
    if (t && st.pos > 2 && prefetchedFor !== t.id) { prefetchedFor = t.id; warmUpcoming(); }
    if (Math.abs(st.pos - lastSessionPos) > 1) { lastSessionPos = st.pos; updateSessionPosition(); }
    maybeCrossfade();
    emit();
  }

  function onAudioError() {
    var t = current();
    if (!t) return;
    // The <audio> element can't say why; ask the server.
    fetch('/api/music/prefetch?now=1&id=' + encodeURIComponent(t.id), { credentials: 'include' })
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
  // `ahead`: matching an upcoming song, not the one about to play.
  function matchTrack(t, ahead) {
    return api('/api/music/match?title=' + encodeURIComponent(t.title) + '&artist=' + encodeURIComponent(t.artist) +
      '&duration=' + encodeURIComponent(t.duration || 0) + (ahead ? '&bg=1' : '')).then(function (d) { return d.id; });
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
    setTimeout(warmUpcoming, 1500);      // after this song has had the first requests
    wantPlayUntil = autoplay ? Date.now() + 8000 : 0;
    if (pos > 0) {
      audio.addEventListener('loadedmetadata', function seekOnce() {
        audio.removeEventListener('loadedmetadata', seekOnce);
        try { audio.currentTime = pos; } catch (e) { /* ignore */ }
      });
    }
    setSession(t);
    if (autoplay) playNow();
  }

  /* ------------------------------------------------------------- shuffle */
  // Two kinds. "random" is fully random: each next song is any other song in the
  // queue, so a song can come back soon. "smart" plays every song once before any
  // repeats, steers away from the same artist back to back and from jumps in
  // length, and every few songs slips in one suggested by YouTube's own Mix for
  // the song that is playing (marked "Suggested" in the queue).
  var history = [];                 // songs played in shuffle order, for Previous
  var smartPlayed = {};             // songs already played this round (smart)
  var smartCount = 0;               // smart picks since the last suggestion
  var pendingSuggestion = null;     // a suggested song waiting to be queued next
  var suggestFor = '';              // the song the pending/last lookup was for
  var suggested = {};               // suggestions already used this session

  function normShuffle(v) { return v === true || v === 'random' ? 'random' : v === 'smart' ? 'smart' : 'off'; }
  function resetShuffleMemory() { history = []; smartPlayed = {}; smartCount = 0; pendingSuggestion = null; suggestFor = ''; }
  function songKey(t) { return t.id || ('s:' + (t.spotifyId || t.title)); }
  function artistKey(t) { return String(t.artist || '').toLowerCase().replace(/[^a-z0-9]+/g, ' ').trim(); }

  function remember() {
    var t = st.queue[st.index];
    if (t) { history.push(t); if (history.length > 100) history.shift(); }
  }

  /** Picks the next index for smart shuffle, or -1 when there is nothing left. */
  function pickSmart() {
    var cur = st.queue[st.index];
    if (!cur) return -1;
    // A suggestion that has arrived goes in right after the current song.
    if (pendingSuggestion) {
      var have = false;
      st.queue.forEach(function (q) { if (q.id === pendingSuggestion.id) have = true; });
      var sug = pendingSuggestion;
      pendingSuggestion = null;
      if (!have) {
        suggested[sug.id] = true; smartCount = 0; queueDirty = true;
        st.queue.splice(st.index + 1, 0, sug);
        smartPlayed[songKey(cur)] = true;
        return st.index + 1;
      }
    }
    smartPlayed[songKey(cur)] = true;
    var pool = function () {
      var out = [];
      for (var i = 0; i < st.queue.length; i++) if (i !== st.index && !smartPlayed[songKey(st.queue[i])]) out.push(i);
      return out;
    };
    var cand = pool();
    if (!cand.length) { smartPlayed = {}; smartPlayed[songKey(cur)] = true; cand = pool(); }   // a new round
    if (!cand.length) return -1;
    var recent = [artistKey(cur)];
    for (var h = history.length - 1; h >= 0 && recent.length < 3; h--) recent.push(artistKey(history[h]));
    var weights = cand.map(function (i) {
      var t = st.queue[i], w = 1, a = artistKey(t);
      if (a && a === recent[0]) w *= 0.12;                       // not the same artist twice in a row
      else if (a && recent.indexOf(a) > 0) w *= 0.45;            // or again very soon
      if (cur.duration && t.duration) {                           // keep the length from jumping around
        var r = Math.min(cur.duration, t.duration) / Math.max(cur.duration, t.duration);
        w *= 0.7 + 0.6 * r;
      }
      return w;
    });
    var total = weights.reduce(function (x, y) { return x + y; }, 0), roll = Math.random() * total, pick = cand[cand.length - 1];
    for (var k = 0; k < cand.length; k++) { roll -= weights[k]; if (roll <= 0) { pick = cand[k]; break; } }
    smartCount++;
    return pick;
  }

  /** Looks up a suggestion in the background so it is ready by the next song. */
  function maybeSuggest() {
    var t = current();
    if (st.shuffle !== 'smart' || !t || !t.id || pendingSuggestion || suggestFor === t.id || smartCount < 2) return;
    suggestFor = t.id;
    api('/api/music/related?id=' + encodeURIComponent(t.id)).then(function (d) {
      if (st.shuffle !== 'smart') return;
      var have = {};
      st.queue.forEach(function (q) { have[q.id] = true; });
      var found = (d.tracks || []).filter(function (x) { return x && x.id && !have[x.id] && !suggested[x.id]; })[0];
      if (found) pendingSuggestion = { id: found.id, title: found.title, artist: found.artist, duration: found.duration, suggested: true };
    }).catch(function () { /* no suggestion this time */ });
  }

  /** The song after the current one: shuffled, or in order, wrapping when repeating all. */
  function advance(auto) {
    if (!st.queue.length) return;
    if (auto && st.repeat === 'one') { go(st.index, true); return; }
    if (st.shuffle === 'random' && st.queue.length > 1) {
      var n;
      do { n = Math.floor(Math.random() * st.queue.length); } while (n === st.index);
      remember();
      go(n, true);
      return;
    }
    if (st.shuffle === 'smart') {
      var next = pickSmart();
      if (next >= 0) { remember(); go(next, true); return; }
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
    killFade();
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
      shuffle: st.shuffle, repeat: st.repeat, crossfade: st.crossfade, xfOk: st.xfOk
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
    st.shuffle = normShuffle(s.shuffle); st.repeat = s.repeat || 'off';
    st.crossfade = s.crossfade || 0; st.xfOk = !!s.xfOk;
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

  /* The player's settings (shuffle, repeat, crossfade, volume). They are kept
   * apart from the queue, so they can be set before anything is playing and are
   * still there after the player is closed. */
  var PREFS_KEY = 'mitch.music.prefs';
  function savePrefs() {
    try {
      localStorage.setItem(PREFS_KEY, JSON.stringify({ shuffle: st.shuffle, repeat: st.repeat, crossfade: st.crossfade, vol: st.vol }));
    } catch (e) { /* storage unavailable */ }
  }
  function loadPrefs() {
    var p = null;
    try { p = JSON.parse(localStorage.getItem(PREFS_KEY) || 'null'); } catch (e) { /* ignore */ }
    if (!p) return;
    st.shuffle = normShuffle(p.shuffle);
    st.repeat = p.repeat === 'all' || p.repeat === 'one' ? p.repeat : 'off';
    st.crossfade = normFade(p.crossfade);
    if (typeof p.vol === 'number') st.vol = clamp(p.vol, 0, 1);
  }
  function setting(cmd, arg) {
    if (cmd === 'shuffle') { st.shuffle = normShuffle(arg); resetShuffleMemory(); }
    else if (cmd === 'repeat') st.repeat = arg === 'all' || arg === 'one' ? arg : 'off';
    else if (cmd === 'crossfade') st.crossfade = normFade(arg);
    else if (cmd === 'vol') { st.vol = clamp(arg, 0, 1); if (audio && !fade) audio.volume = st.vol; }
    savePrefs();
  }

  /* -------------------------------------------------------------- commands */
  function runCmd(cmd, arg) {
    // Anything that changes what is playing ends an overlap in progress.
    if (fade && cmd !== 'vol' && cmd !== 'seek' && cmd !== 'shuffle' && cmd !== 'repeat' && cmd !== 'crossfade' && cmd !== 'enqueue') killFade();
    switch (cmd) {
      case 'toggle':
        if (!st.queue.length) return;
        // A song that is still being fetched ignores the button, so repeated
        // presses don't start and stop it.
        if (st.loading && !st.playing && !st.error) return;
        if (audio.paused) playNow(); else { wantPlayUntil = 0; audio.pause(); }
        break;
      case 'play': playNow(); break;
      case 'pause': wantPlayUntil = 0; audio.pause(); break;
      case 'next': advance(false); break;
      case 'shuffle':
      case 'crossfade':
      case 'repeat':
        setting(cmd, arg);
        break;
      case 'prev':
        if (audio.currentTime > 3) { audio.currentTime = 0; break; }
        // Shuffled: go back to the song that actually played before this one.
        if (st.shuffle !== 'off' && history.length) {
          var back = history.pop(), at = st.queue.indexOf(back);
          if (at >= 0) { go(at, true); break; }
        }
        go(st.index - 1, true);
        break;
      case 'seek': if (isFinite(arg)) { audio.currentTime = clamp(arg, 0, st.dur || 1e9); st.pos = audio.currentTime; } break;
      case 'vol': setting('vol', arg); break;
      case 'jump': go(arg, true); break;
      case 'playQueue':
        resetShuffleMemory();
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
    killFade();
    wantPlayUntil = 0;
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
      // Settings don't need a song: apply them where they are.
      if (cmd === 'shuffle' || cmd === 'repeat' || cmd === 'crossfade' || cmd === 'vol') {
        setting(cmd, arg);
        if (st.queue.length) save();
        emit();
        return;
      }
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
        artwork: [{ src: absolute('/api/music/thumb?id=' + encodeURIComponent(t.id)), sizes: '600x600' }]
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
      st.shuffle = normShuffle(s.shuffle); st.repeat = s.repeat === 'all' || s.repeat === 'one' ? s.repeat : 'off';
      st.crossfade = normFade(s.crossfade);
    }
    loadPrefs();               // the settings win over what an old saved state held
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
  function getLyricsRaw(track, custom) {
    if (!track) return Promise.resolve({ found: false });
    if (!custom) {
      if (lyricCache[track.id]) return lyricCache[track.id];
      var kept = sessionLyrics(track.id);
      if (kept) return (lyricCache[track.id] = Promise.resolve(kept));
    }
    var q = '/api/music/lyrics?id=' + encodeURIComponent(track.id) +
      '&title=' + encodeURIComponent(custom ? custom.title : track.title) +
      '&artist=' + encodeURIComponent(custom ? custom.artist : track.artist) +
      '&duration=' + encodeURIComponent(track.duration || 0) + (custom ? '&custom=1' + (custom.retry ? '&retry=1' : '') : '');
    var p = api(q).then(function (d) {
      if (!custom && d && d.found !== undefined) sessionLyrics(track.id, d);
      return d;
    }).catch(function () { delete lyricCache[track.id]; return { found: false, error: true }; });
    if (!custom) lyricCache[track.id] = p;
    return p;
  }

  /* The viewer's own timing correction for a song (milliseconds, kept in this
   * browser), for lyrics the automatic check couldn't fix. */
  var OFFSET_KEY = 'mitch.music.lyroff';
  function lyricOffset(id) {
    try { return Number((JSON.parse(localStorage.getItem(OFFSET_KEY) || '{}') || {})[id]) || 0; } catch (e) { return 0; }
  }
  function setLyricOffset(id, ms) {
    try {
      var all = JSON.parse(localStorage.getItem(OFFSET_KEY) || '{}') || {};
      ms = clamp(Math.round(ms), -30000, 30000);
      if (ms) all[id] = ms; else delete all[id];
      var keys = Object.keys(all);
      if (keys.length > 300) delete all[keys[0]];
      localStorage.setItem(OFFSET_KEY, JSON.stringify(all));
    } catch (e) { /* storage unavailable */ }
  }
  function withOffset(d, ms) {
    if (!ms || !d || !d.found || !d.lines || d.sync === 'none') return d;
    var mv = function (t) { return Math.max(0, t + ms); };
    var out = {};
    Object.keys(d).forEach(function (k) { out[k] = d[k]; });
    out.lines = d.lines.map(function (l) {
      return Object.assign({}, l, {
        t: mv(l.t), e: mv(l.e),
        words: (l.words || []).map(function (w) { return Object.assign({}, w, { t: mv(w.t), e: mv(w.e) }); })
      });
    });
    return out;
  }
  function getLyrics(track, custom) {
    return getLyricsRaw(track, custom).then(function (d) { return track ? withOffset(d, lyricOffset(track.id)) : d; });
  }

  /** Plays a list in shuffled order: turns shuffle on if it is off (random; an
   *  already chosen mode is kept) and starts at a random song. */
  function playShuffled(queue) {
    var list = (queue || []).filter(function (t) { return t && (t.id || t.spotifyId); });
    if (!list.length) return;
    if (st.shuffle === 'off') control('shuffle', 'random');
    control('playQueue', { queue: list, index: Math.floor(Math.random() * list.length) });
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
      data = null; lineEls = []; wordEls = []; active = -2; alive = 0;
    }

    function set(d) {
      active = -2; lineEls = []; wordEls = []; wordsOf = []; alive = 0;
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
          for (var g = 0; g < 3; g++) el.appendChild(document.createElement('i')).className = 'ml-dot';
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

    /* Sparkles: while a word is sung, small stars appear at the edge of the fill
     * and float away. A few per second, never more than MAX alive, none with
     * reduced motion or in a hidden tab. */
    var lastSpark = 0, alive = 0, MAX = 12;
    var coarse = window.matchMedia && matchMedia('(pointer: coarse)').matches;
    function sparkle(i, ms) {
      if (reduce || opts.sparkles === false || document.hidden || alive >= MAX) return;
      var now = Date.now();
      if (now - lastSpark < (coarse ? 260 : 150)) return;
      var line = data.lines[i], ws = wordEls[i];
      if (!ws.length) return;
      for (var k = 0; k < line.words.length; k++) {
        var w = line.words[k];
        if (ms >= w.t && ms < w.e) {
          lastSpark = now;
          var el = ws[k], p = (ms - w.t) / Math.max(1, w.e - w.t);
          var star = document.createElement('i');
          star.className = 'ml-spark';
          star.style.left = Math.round(el.offsetLeft + el.offsetWidth * p) + 'px';
          star.style.top = Math.round(el.offsetTop + el.offsetHeight * (0.1 + Math.random() * 0.8)) + 'px';
          star.style.setProperty('--dx', (Math.random() * 26 - 13).toFixed(1) + 'px');
          star.style.setProperty('--dy', (-10 - Math.random() * 22).toFixed(1) + 'px');
          star.style.setProperty('--s', (0.45 + Math.random() * 0.6).toFixed(2));
          star.style.animationDuration = (700 + Math.random() * 500).toFixed(0) + 'ms';
          alive++;
          var gone = false;
          var done = function () { if (gone) return; gone = true; alive = Math.max(0, alive - 1); star.remove(); };
          star.addEventListener('animationend', done);
          setTimeout(done, 1800);                 // in case the animation never ends (hidden tab)
          lineEls[i].appendChild(star);
          return;
        }
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
        // How far each line is from the one being sung (CSS fades and blurs by it).
        if (i >= 0) {
          for (var d = 0; d < lineEls.length; d++) lineEls[d].style.setProperty('--d', Math.min(5, Math.abs(d - i)));
          if (opts.onLine) opts.onLine(i);
        }
        if (i >= 0 && Date.now() > userScrollUntil) {
          var el = lineEls[i];
          var top = el.offsetTop - root.clientHeight * 0.32 + el.offsetHeight / 2;
          root.scrollTo({ top: Math.max(0, top), behavior: reduce ? 'auto' : 'smooth' });
        }
      }
      if (i >= 0) { paintWords(i, ms); sparkle(i, ms); }
      if (i >= 0 && lineEls[i].classList.contains('ml-gap')) {      // an instrumental break: the dots fill up
        var nextT = data.lines[i + 1] ? data.lines[i + 1].t : data.lines[i].e;
        var span = Math.max(1, nextT - data.lines[i].t);
        lineEls[i].style.setProperty('--gp', Math.min(1, Math.max(0, (ms - data.lines[i].t) / span)).toFixed(3));
      }
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
    shuffle: function (mode) { control('shuffle', mode); },
    repeat: function (mode) { control('repeat', mode); },
    crossfade: function (secs) { control('crossfade', secs); },
    playShuffled: playShuffled,
    stop: function () { if (st.role === 'remote') post({ k: 'cmd', cmd: 'stop' }); else stopHost(); },
    popout: popout,
    getLyrics: getLyrics, lyricOffset: lyricOffset, setLyricOffset: setLyricOffset, lineAt: lineAt, createLyricsView: createLyricsView
  };
})();
