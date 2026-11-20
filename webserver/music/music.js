/* /music/ page and the pop-out window (docs/music.md). Both use the same
 * markup ids; the pop-out has no search or list. */
(function () {
  'use strict';
  var M = window.MitchMusic;
  var popout = document.body.dataset.mode === 'popout';
  var $ = function (id) { return document.getElementById(id); };

  var ICONS = {
    play: 'M8 5v14l11-7z',
    pause: 'M6 5h4v14H6zm8 0h4v14h-4z',
    prev: 'M6 6h2v12H6zm3.5 6L18 18V6z',
    next: 'M16 6h2v12h-2zM6 18l8.5-6L6 6z',
    pop: 'M14 3h7v7h-2V6.4l-8.3 8.3-1.4-1.4L17.6 5H14zM5 5h6v2H6v11h11v-5h2v7H4V5z',
    heart: 'M12 21s-7.5-4.6-9.6-9.2C.9 8.4 2.7 5 6 5c2 0 3.4 1.1 4 2.2h4c.6-1.1 2-2.2 4-2.2 3.3 0 5.1 3.4 3.6 6.8C19.5 16.4 12 21 12 21z',
    plus: 'M11 5h2v6h6v2h-6v6h-2v-6H5v-2h6z',
    queue: 'M4 6h16v2H4zm0 5h10v2H4zm0 5h10v2H4zm12-5v6.2a3 3 0 1 0 2 2.8V13h2v-2z'
  };
  function icon(name) {
    var svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
    svg.setAttribute('viewBox', '0 0 24 24');
    svg.setAttribute('aria-hidden', 'true');
    var p = document.createElementNS('http://www.w3.org/2000/svg', 'path');
    p.setAttribute('d', ICONS[name]);
    svg.appendChild(p);
    return svg;
  }
  function setIcon(el, name) { el.replaceChildren(icon(name)); }

  var el = {
    art: $('mp-art'), title: $('mp-title'), artist: $('mp-artist'), fav: $('mp-fav'), pop: $('mp-pop'),
    prev: $('mp-prev'), play: $('mp-play'), next: $('mp-next'), cur: $('mp-cur'), dur: $('mp-dur'),
    seek: $('mp-seek'), vol: $('mp-vol'), lyrics: $('mp-lyrics'), info: $('mp-lyricsinfo'),
    wrong: $('mp-wrong'), wrongForm: $('mp-wrongform'), wt: $('mp-wt'), wa: $('mp-wa'),
    status: $('mp-status'), list: $('mp-list'), q: $('mp-q')
  };
  setIcon(el.prev, 'prev'); setIcon(el.play, 'play'); setIcon(el.next, 'next'); setIcon(el.fav, 'heart');
  if (el.pop) setIcon(el.pop, 'pop');

  var lv = M.createLyricsView(el.lyrics, { onSeek: function (s) { M.seek(s); } });
  var results = [], library = [], favIds = {}, tab = 'search', shownId = '', seeking = false;

  /* ---------------------------------------------------------- list views */
  function say(text) { el.status.textContent = text || ''; }

  function row(track, i, list) {
    var li = document.createElement('li');
    li.className = 'mp-row';
    li.dataset.id = track.id;
    var img = document.createElement('img');
    img.className = 'mp-thumb'; img.alt = ''; img.loading = 'lazy'; img.width = 48; img.height = 48;
    img.src = M.thumb(track.id);
    var main = document.createElement('button');
    main.type = 'button'; main.className = 'mp-rowmain';
    var t = document.createElement('span'); t.className = 'mp-rowtitle'; t.textContent = track.title;
    var a = document.createElement('span'); a.className = 'mp-rowartist'; a.textContent = track.artist;
    main.append(t, a);
    main.addEventListener('click', function () { M.play(list, i); });
    var d = document.createElement('span');
    d.className = 'mp-rowdur'; d.textContent = M.fmt(track.duration);
    var add = document.createElement('button');
    add.type = 'button'; add.className = 'mp-icon mp-small'; add.title = 'Add to queue'; add.setAttribute('aria-label', 'Add to queue');
    setIcon(add, 'plus');
    add.addEventListener('click', function () { M.enqueue(track); say('Added to queue'); });
    var heart = document.createElement('button');
    heart.type = 'button'; heart.className = 'mp-icon mp-small mp-heart'; setIcon(heart, 'heart');
    syncHeart(heart, track.id);
    heart.addEventListener('click', function () { toggleFav(track).then(function () { syncHeart(heart, track.id); }); });
    li.append(img, main, d, add, heart);
    return li;
  }
  function syncHeart(btn, id) {
    var on = !!favIds[id];
    btn.classList.toggle('mp-on', on);
    btn.setAttribute('aria-pressed', on ? 'true' : 'false');
    var label = on ? 'Remove from library' : 'Save to library';
    btn.title = label;
    btn.setAttribute('aria-label', label);
  }

  function render() {
    if (!el.list) return;
    var list = tab === 'search' ? results : tab === 'library' ? library : M.state.queue;
    el.list.replaceChildren();
    if (!list.length) {
      var empty = document.createElement('li');
      empty.className = 'mp-empty';
      empty.textContent = tab === 'search' ? '' : tab === 'library' ? 'Nothing saved yet' : 'The queue is empty';
      if (empty.textContent) el.list.appendChild(empty);
      return;
    }
    list.forEach(function (t, i) {
      var r = row(t, i, list);
      if (tab === 'queue') {
        r.querySelector('.mp-rowmain').onclick = function () { M.jump(i); };
      }
      el.list.appendChild(r);
    });
    markPlaying();
  }
  function markPlaying() {
    if (!el.list) return;
    var cur = M.current();
    Array.prototype.forEach.call(el.list.children, function (li) {
      li.classList.toggle('mp-playing', !!cur && li.dataset.id === cur.id);
    });
  }

  function setTab(name) {
    tab = name;
    ['search', 'library', 'queue'].forEach(function (n) {
      $('tab-' + n).setAttribute('aria-selected', n === name ? 'true' : 'false');
    });
    say('');
    if (name === 'library') loadLibrary(); else render();
  }

  function loadLibrary() {
    return M.api('/api/music/library').then(function (d) {
      library = d.tracks || [];
      favIds = {};
      library.forEach(function (t) { favIds[t.id] = true; });
      if (tab === 'library') render();
      syncNowFav();
    }).catch(function (e) { say(e.message); });
  }

  function toggleFav(track) {
    var on = !!favIds[track.id];
    var req = on
      ? M.api('/api/music/library/remove', { body: { id: track.id } })
      : M.api('/api/music/library/add', { body: { id: track.id, title: track.title, artist: track.artist, duration: track.duration } });
    return req.then(function () {
      if (on) delete favIds[track.id]; else favIds[track.id] = true;
      library = library.filter(function (t) { return t.id !== track.id; });
      if (!on) library.unshift(track);
      syncNowFav();
      if (tab === 'library') render();
    }).catch(function (e) { say(e.message); });
  }
  function syncNowFav() {
    var t = M.current();
    el.fav.hidden = !t;
    if (t) syncHeart(el.fav, t.id);
  }

  /* -------------------------------------------------------------- search */
  var searchForm = $('mp-search');
  if (searchForm) {
    searchForm.addEventListener('submit', function (e) {
      e.preventDefault();
      var q = el.q.value.trim();
      if (!q) return;
      setTab('search');
      say('Searching…');
      M.api('/api/music/search?q=' + encodeURIComponent(q)).then(function (d) {
        results = d.tracks || [];
        say(results.length ? '' : 'No results');
        render();
      }).catch(function (err) { results = []; render(); say(err.message); });
    });
    Array.prototype.forEach.call(document.querySelectorAll('.mp-tabs [data-tab]'), function (b) {
      b.addEventListener('click', function () { setTab(b.dataset.tab); });
    });
  }

  /* ------------------------------------------------------- now playing */
  el.play.addEventListener('click', function () { M.toggle(); });
  el.prev.addEventListener('click', function () { M.prev(); });
  el.next.addEventListener('click', function () { M.next(); });
  if (el.pop) el.pop.addEventListener('click', function () { if (!M.popout()) say('Pop-up was blocked by the browser'); });
  el.fav.addEventListener('click', function () { var t = M.current(); if (t) toggleFav(t); });
  el.seek.addEventListener('input', function () {
    seeking = true;
    var d = M.state.dur || 0;
    el.cur.textContent = M.fmt((el.seek.value / 1000) * d);
  });
  el.seek.addEventListener('change', function () {
    var d = M.state.dur || 0;
    if (d > 0) M.seek((el.seek.value / 1000) * d);
    seeking = false;
  });
  el.vol.addEventListener('input', function () { M.volume(el.vol.value / 100); });

  function loadLyrics(track, custom) {
    lv.set('loading');
    el.info.textContent = '';
    el.wrong.hidden = true;
    M.getLyrics(track, custom).then(function (d) {
      if (!M.current() || M.current().id !== track.id) return;
      lv.set(d);
      el.wrong.hidden = false;
      if (d && d.found) {
        var how = d.sync === 'word' ? 'word by word' : d.sync === 'line' ? (d.estimated ? 'line timing, words estimated' : 'line by line') : 'text only';
        el.info.textContent = 'Lyrics from ' + d.source + ' · ' + how;
      } else {
        el.info.textContent = '';
      }
    });
  }
  el.wrong.addEventListener('click', function () {
    var t = M.current();
    if (!t) return;
    el.wt.value = t.title; el.wa.value = t.artist;
    el.wrongForm.hidden = !el.wrongForm.hidden;
    if (!el.wrongForm.hidden) el.wt.focus();
  });
  el.wrongForm.addEventListener('submit', function (e) {
    e.preventDefault();
    var t = M.current();
    if (!t) return;
    loadLyrics(t, { title: el.wt.value.trim() || t.title, artist: el.wa.value.trim() });
    el.wrongForm.hidden = true;
  });

  function onState(s) {
    var t = M.current();
    el.title.textContent = t ? t.title : 'Nothing playing';
    el.artist.textContent = t ? t.artist : '';
    el.art.hidden = !t;
    if (t && shownId !== t.id) {
      shownId = t.id;
      el.art.src = M.thumb(t.id);
      el.wrongForm.hidden = true;
      loadLyrics(t);
    }
    if (!t && shownId) { shownId = ''; lv.set(null); el.info.textContent = ''; el.wrong.hidden = true; }
    setIcon(el.play, s.playing ? 'pause' : 'play');
    el.play.setAttribute('aria-label', s.playing ? 'Pause' : 'Play');
    el.play.title = s.playing ? 'Pause' : 'Play';
    if (document.activeElement !== el.vol) el.vol.value = Math.round(s.vol * 100);
    var msg = s.error ? s.error : s.blocked ? 'Press play to continue'
      : (!popout && s.popoutOpen && s.role === 'remote') ? 'Playing in the pop-out window' : '';
    if (msg || (el.status.dataset.owner === 'state')) { say(msg); el.status.dataset.owner = msg ? 'state' : ''; }
    syncNowFav();
    markPlaying();
    if (tab === 'queue' && el.list) render();
  }

  function frame() {
    var s = M.state, time = M.getTime();
    if (!seeking) {
      el.cur.textContent = M.fmt(time);
      el.dur.textContent = M.fmt(s.dur);
      el.seek.value = s.dur > 0 ? Math.round((time / s.dur) * 1000) : 0;
    }
    lv.tick(time * 1000);
    requestAnimationFrame(frame);
  }

  document.addEventListener('keydown', function (e) {
    if (e.target && /^(INPUT|TEXTAREA|SELECT)$/.test(e.target.tagName)) return;
    if (e.key === ' ') { e.preventDefault(); M.toggle(); }
  });

  M.init({ popout: popout });
  M.onChange(onState);
  loadLibrary();
  requestAnimationFrame(frame);
})();
