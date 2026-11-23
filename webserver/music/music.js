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
    sync: 'M17.65 6.35A7.96 7.96 0 0 0 12 4a8 8 0 1 0 7.75 10h-2.08A6 6 0 1 1 12 6c1.66 0 3.14.69 4.22 1.78L13 11h7V4l-2.35 2.35z',
    more: 'M6 10a2 2 0 1 0 0 4 2 2 0 0 0 0-4zm6 0a2 2 0 1 0 0 4 2 2 0 0 0 0-4zm6 0a2 2 0 1 0 0 4 2 2 0 0 0 0-4z',
    queue: 'M4 6h16v2H4zm0 5h10v2H4zm0 5h10v2H4zm12-5v6.2a3 3 0 1 0 2 2.8V13h2v-2z',
    listadd: 'M3 6h12v2H3zm0 5h12v2H3zm0 5h8v2H3zm13-1v-3h2v3h3v2h-3v3h-2v-3h-3v-2z',
    shuffle: 'M16 4h5v5l-1.8-1.8L5.4 21 4 19.6 17.8 5.8 16 4zm0 16l1.8-1.8-3.2-3.2 1.4-1.4 3.2 3.2L21 15v5h-5zM4 4.4 9.6 10 8.2 11.4 2.6 5.8 4 4.4z',
    repeat: 'M7 7h10v3l4-4-4-4v3H5v6h2V7zm10 10H7v-3l-4 4 4 4v-3h12v-6h-2v4z',
    search: 'M15.5 14h-.79l-.28-.27A6.47 6.47 0 0 0 16 9.5 6.5 6.5 0 1 0 9.5 16c1.61 0 3.09-.59 4.23-1.57l.27.28v.79l5 4.99L20.49 19l-4.99-5zm-6 0C7.01 14 5 11.99 5 9.5S7.01 5 9.5 5 14 7.01 14 9.5 11.99 14 9.5 14z',
    note: 'M12 3v10.55A4 4 0 1 0 14 17V7h4V3z',
    playlist: 'M3 6h12v2H3zm0 5h12v2H3zm0 5h7v2H3zm13-8v7.2a3 3 0 1 0 2 2.8V10h3V8z',
    upnext: 'M4 6h12v2H4zm0 5h12v2H4zm0 5h7v2H4zm11-1.5 6 3.5-6 3.5z',
    discover: 'M12 2a10 10 0 1 0 0 20 10 10 0 0 0 0-20zm0 18a8 8 0 1 1 0-16 8 8 0 0 1 0 16zM7.5 16.5l3.2-5.8 5.8-3.2-3.2 5.8-5.8 3.2zm4.5-5a1.5 1.5 0 1 0 0 3 1.5 1.5 0 0 0 0-3z',
    up: 'M7 14l5-5 5 5z',
    down: 'M7 10l5 5 5-5z',
    remove: 'M6.4 5 5 6.4 10.6 12 5 17.6 6.4 19 12 13.4 17.6 19 19 17.6 13.4 12 19 6.4 17.6 5 12 10.6z'
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
  /** Sets a button's icon only if it changed. State updates arrive several
   *  times a second while playing; rebuilding the icon every time can swallow a
   *  click that lands on the node being replaced. */
  function setIcon(el, name) {
    if (el.dataset.icon === name) return;
    el.dataset.icon = name;
    el.replaceChildren(icon(name));
  }
  function setText(el, text) { if (el.textContent !== text) el.textContent = text; }
  function setAttr(el, name, value) { if (el.getAttribute(name) !== value) el.setAttribute(name, value); }

  var el = {
    art: $('mp-art'), title: $('mp-title'), artist: $('mp-artist'), fav: $('mp-fav'), addPl: $('mp-addpl'), pop: $('mp-pop'),
    prev: $('mp-prev'), play: $('mp-play'), next: $('mp-next'), cur: $('mp-cur'), dur: $('mp-dur'),
    seek: $('mp-seek'), vol: $('mp-vol'), lyrics: $('mp-lyrics'), info: $('mp-lyricsinfo'),
    wrong: $('mp-wrong'), wrongForm: $('mp-wrongform'), wt: $('mp-wt'), wa: $('mp-wa'),
    status: $('mp-status'), list: $('mp-list'), q: $('mp-q'),
    dvPanel: $('mp-discover'),
    plPanel: $('mp-playlists'), plHome: $('mp-plhome'), plList: $('mp-pllist'), plDetail: $('mp-pldetail'),
    plName: $('mp-pl-name'), plMeta: $('mp-pl-meta'), plActions: $('mp-pl-actions'), plShare: $('mp-pl-share'),
    plTracks: $('mp-pltracks'), plSearch: $('mp-plsearch'), plQ: $('mp-plq'),
    dialog: $('mp-dialog'), picker: $('mp-picker'),
    shuffle: $('mp-shuffle'), repeat: $('mp-repeat'), stage: $('mp-stage'), ambient: $('mp-ambient')
  };
  setIcon(el.prev, 'prev'); setIcon(el.play, 'play'); setIcon(el.next, 'next'); setIcon(el.fav, 'heart'); if (el.addPl) setIcon(el.addPl, 'listadd');
  if (el.shuffle) setIcon(el.shuffle, 'shuffle');
  if (el.repeat) setIcon(el.repeat, 'repeat');
  if ($('mp-searchicon')) setIcon($('mp-searchicon'), 'search');
  Array.prototype.forEach.call(document.querySelectorAll('.mp-navicon'), function (n) { setIcon(n, n.dataset.ico); });
  if ($('mp-sidepl-new')) setIcon($('mp-sidepl-new'), 'plus');
  if ($('mp-pl-add')) setIcon($('mp-pl-add'), 'plus');
  if ($('mp-coverempty')) setIcon($('mp-coverempty'), 'note');
  if (el.pop) setIcon(el.pop, 'pop');

  var lv = M.createLyricsView(el.lyrics, {
    onSeek: function (s) { M.seek(s); },
    // Every new line gives the backdrop a soft pulse.
    onLine: function () {
      var a = el.ambient;
      if (!a || !M.state.playing) return;
      a.classList.remove('mp-beat'); void a.offsetWidth; a.classList.add('mp-beat');
    }
  });
  var dv = { data: null, at: 0, genre: null, genreTracks: null, loading: false, req: 0 };
  var results = [], library = [], favIds = {}, tab = 'search', shownId = '', seeking = false, queueSig = '';

  /* ---------------------------------------------------------- list views */
  function say(text) { el.status.textContent = text || ''; }

  /** A song row. `onPlay` replaces what choosing the row does (the queue jumps
   *  instead of replacing itself). Choosing the song that is already playing
   *  pauses or resumes it. */
  function row(track, i, list, extra, onPlay) {
    var li = document.createElement('li');
    li.className = 'mp-row';
    li.dataset.id = track.id || '';
    var wrap = document.createElement('span');
    wrap.className = 'mp-thumbwrap';
    var img = document.createElement('img');
    img.className = 'mp-thumb'; img.alt = ''; img.loading = 'lazy'; img.width = 52; img.height = 52;
    if (track.id) img.src = M.thumb(track.id);
    var eq = document.createElement('span');
    eq.className = 'mp-eq';
    eq.setAttribute('aria-hidden', 'true');
    eq.append(document.createElement('i'), document.createElement('i'), document.createElement('i'));
    wrap.append(img, eq);
    var main = document.createElement('button');
    main.type = 'button'; main.className = 'mp-rowmain';
    var t = document.createElement('span'); t.className = 'mp-rowtitle'; t.textContent = track.title;
    var a = document.createElement('span'); a.className = 'mp-rowartist'; a.textContent = track.artist;
    if (track.suggested) {
      var tag = document.createElement('span'); tag.className = 'mp-tag'; tag.textContent = 'Suggested';
      a.prepend(tag);
    }
    main.append(t, a);
    function choose() {
      var cur = M.current();
      if (cur && cur.id && cur.id === track.id) M.toggle();
      else if (onPlay) onPlay();
      else M.play(list, i);
    }
    main.addEventListener('click', choose);
    // The cover shows a play hint on hover, so clicking it has to play too.
    wrap.addEventListener('click', choose);
    var d = document.createElement('span');
    d.className = 'mp-rowdur'; d.textContent = M.fmt(track.duration);
    var actions = document.createElement('span');
    actions.className = 'mp-rowactions';
    var add = document.createElement('button');
    add.type = 'button'; add.className = 'mp-icon mp-small'; add.title = 'Add to queue'; add.setAttribute('aria-label', 'Add to queue');
    setIcon(add, 'plus');
    add.addEventListener('click', function () { M.enqueue(track); say('Added to queue'); });
    actions.appendChild(add);
    li.append(wrap, main, d, actions);
    if (extra) { extra(li, actions); return li; }
    var toPl = document.createElement('button');
    toPl.type = 'button'; toPl.className = 'mp-icon mp-small'; toPl.title = 'Add to playlist'; toPl.setAttribute('aria-label', 'Add to playlist');
    setIcon(toPl, 'listadd');
    toPl.addEventListener('click', function () { pickPlaylist(track); });
    var heart = document.createElement('button');
    heart.type = 'button'; heart.className = 'mp-icon mp-small mp-heart'; setIcon(heart, 'heart');
    syncHeart(heart, track.id);
    heart.addEventListener('click', function () { toggleFav(track).then(function () { syncHeart(heart, track.id); }); });
    actions.append(toPl, heart);
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

  var searching = false;       // a search is in flight: show placeholder rows
  function skeleton() {
    for (var i = 0; i < 8; i++) {
      var li = document.createElement('li');
      li.className = 'mp-skel';
      li.setAttribute('aria-hidden', 'true');
      var art = document.createElement('i'), lines = document.createElement('span');
      var a = document.createElement('i'), b = document.createElement('i');
      a.style.width = (48 + (i * 17) % 36) + '%'; b.style.width = (26 + (i * 11) % 24) + '%';
      lines.append(a, b);
      li.append(art, lines);
      el.list.appendChild(li);
    }
  }

  function render() {
    if (!el.list) return;
    setTitle();
    el.list.hidden = tab === 'playlists' || tab === 'discover';
    el.plPanel.hidden = tab !== 'playlists';
    if (el.dvPanel) el.dvPanel.hidden = tab !== 'discover';
    if (tab === 'discover') { renderDiscover(); return; }
    if (tab === 'playlists') { renderPlaylists(); return; }
    var list = tab === 'search' ? results : tab === 'library' ? library : M.state.queue;
    el.list.replaceChildren();
    if (tab === 'search' && searching) { skeleton(); return; }
    if (!list.length) {
      var q = ($('mp-q') && $('mp-q').value.trim()) || '';
      var empty = document.createElement('li');
      empty.className = 'mp-empty';
      var glyph = document.createElement('span');
      glyph.className = 'mp-emptyicon';
      setIcon(glyph, tab === 'search' ? 'search' : tab === 'library' ? 'heart' : 'upnext');
      var words = document.createElement('span');
      words.textContent = tab === 'search' ? (q ? 'No results for \u201c' + q + '\u201d' : 'Search for songs or artists')
        : tab === 'library' ? 'Songs you save with the heart appear here' : 'The queue is empty';
      empty.append(glyph, words);
      el.list.appendChild(empty);
      return;
    }
    list.forEach(function (t, i) {
      el.list.appendChild(row(t, i, list, null, tab === 'queue' ? function () { M.jump(i); } : null));
    });
    markPlaying();
  }
  function markPlaying() {
    if (!el.list) return;
    var cur = M.current();
    var lists = [el.list, el.plTracks];
    if (el.dvPanel) Array.prototype.forEach.call(el.dvPanel.querySelectorAll('ul.mp-list'), function (ul) { lists.push(ul); });
    lists.forEach(function (ul) {
      Array.prototype.forEach.call(ul.children, function (li) {
        var mine = !!cur && !!cur.id && li.dataset.id === cur.id;
        li.classList.toggle('mp-playing', mine);
        li.classList.toggle('mp-fetching', mine && !!M.state.loading && !M.state.playing && !M.state.error);
        li.classList.toggle('mp-live', mine && !!M.state.playing);
      });
    });
  }

  /** The big heading above the list says what it is showing. */
  function setTitle() {
    var t = $('mp-viewtitle');
    if (!t) return;
    var q = ($('mp-q') && $('mp-q').value.trim()) || '';
    var text = tab === 'search' ? (q ? 'Results for \u201c' + q + '\u201d' : 'Search')
      : tab === 'discover' ? (dv.genre ? dv.genre.name : 'Discover')
      : tab === 'library' ? 'Library' : tab === 'queue' ? 'Queue'
      : plOpen ? plOpen.p.name : plMode === 'public' ? 'Public playlists' : 'Playlists';
    t.hidden = tab === 'playlists' && !!plOpen;       // an open playlist shows its own heading
    if (t.textContent !== text) t.textContent = text;
  }

  function setTab(name) {
    tab = name;
    queueSig = '';
    ['search', 'discover', 'library', 'playlists', 'queue'].forEach(function (n) {
      $('tab-' + n).setAttribute('aria-selected', n === name ? 'true' : 'false');
    });
    say('');
    setTitle();
    if (name === 'library') loadLibrary();
    else if (name === 'discover') { dv.genre = null; loadDiscover(); }
    else render();
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
    el.fav.hidden = !t || !t.id;
    if (el.addPl) el.addPl.hidden = !t;
    if (t && t.id) syncHeart(el.fav, t.id);
  }

  /* ------------------------------------------------------------ dialogs */
  /** Small form dialog. Resolves to {field: value} on submit, null on cancel. */
  function askForm(opts) {
    var d = el.dialog;
    $('mp-dialogtitle').textContent = opts.title;
    $('mp-dialognote').textContent = opts.note || '';
    $('mp-dialognote').classList.toggle('mp-problem', !!opts.problem);
    $('mp-dialogok').textContent = opts.submit;
    var box = $('mp-dialogfields');
    box.replaceChildren();
    var inputs = {};
    opts.fields.forEach(function (f) {
      var label = document.createElement('label');
      label.className = 'mp-field';
      var span = document.createElement('span');
      span.textContent = f.label;
      var input;
      if (f.options) {
        input = document.createElement('select');
        f.options.forEach(function (o) {
          var op = document.createElement('option');
          op.value = o[0]; op.textContent = o[1];
          input.appendChild(op);
        });
      } else {
        input = document.createElement('input');
        input.type = 'text'; input.maxLength = f.max || 200;
        if (f.placeholder) input.placeholder = f.placeholder;
      }
      if (f.value) input.value = f.value;      // a dropdown keeps its first option unless told otherwise
      inputs[f.name] = input;
      label.append(span, input);
      box.appendChild(label);
    });
    return new Promise(function (resolve) {
      function done(value) {
        d.removeEventListener('close', onClose);
        $('mp-dialogform').removeEventListener('submit', onSubmit);
        $('mp-dialogcancel').removeEventListener('click', onCancel);
        if (d.open) d.close();
        resolve(value);
      }
      function onSubmit(e) {
        e.preventDefault();
        var out = {};
        Object.keys(inputs).forEach(function (k) { out[k] = inputs[k].value.trim(); });
        done(out);
      }
      function onCancel() { done(null); }
      function onClose() { done(null); }
      $('mp-dialogform').addEventListener('submit', onSubmit);
      $('mp-dialogcancel').addEventListener('click', onCancel);
      d.addEventListener('close', onClose);
      d.showModal();
      var first = box.querySelector('input, select');
      if (first) first.focus();
    });
  }

  var VISIBILITY = [['private', 'Private'], ['public', 'Public']];
  function trackBody(t) { return { id: t.id || '', title: t.title, artist: t.artist, duration: t.duration || 0, spotifyId: t.spotifyId || '' }; }

  /* ----------------------------------------------------------- discover */
  // Top songs and genres. Loaded when the tab opens
  // and kept for ten minutes.

  function loadDiscover() {
    if (dv.data && Date.now() - dv.at < 600000) { render(); return Promise.resolve(); }
    dv.loading = true; render();
    return M.api('/api/music/discover').then(function (d) {
      dv.data = d; dv.at = Date.now(); dv.loading = false; render();
    }).catch(function (e) { dv.loading = false; say(e.message); render(); });
  }

  function openGenre(g) {
    var req = ++dv.req;
    dv.genre = g; dv.genreTracks = null; say(''); render();
    M.api('/api/music/discover?genre=' + encodeURIComponent(g.id)).then(function (d) {
      if (req !== dv.req || dv.genre !== g) return;
      dv.genreTracks = d.tracks || []; render();
    }).catch(function (e) { if (req === dv.req) { dv.genreTracks = []; say(e.message); render(); } });
  }

  function discoverSection(title, tracks) {
    var box = document.createElement('section');
    box.className = 'mp-dvsection';
    var h = document.createElement('h3');
    h.className = 'mp-dvtitle'; h.textContent = title;
    var ul = document.createElement('ul');
    ul.className = 'mp-list';
    tracks.forEach(function (t, i) { ul.appendChild(row(t, i, tracks)); });
    box.append(h, ul);
    return box;
  }

  function renderDiscover() {
    var box = el.dvPanel;
    box.replaceChildren();
    if (dv.genre) {
      var back = document.createElement('button');
      back.type = 'button'; back.className = 'mp-link mp-dvback'; back.textContent = '\u2190 Discover';
      back.addEventListener('click', function () { dv.genre = null; dv.req++; say(''); render(); });
      box.appendChild(back);
      if (dv.genreTracks === null) {
        var ul = document.createElement('ul'); ul.className = 'mp-list'; box.appendChild(ul);
        var saved = el.list; el.list = ul; skeleton(); el.list = saved;
      } else if (!dv.genreTracks.length) {
        var none = document.createElement('p'); none.className = 'mp-empty'; none.textContent = 'No songs found for this genre right now';
        box.appendChild(none);
      } else {
        var gl = document.createElement('ul'); gl.className = 'mp-list'; box.appendChild(gl);
        dv.genreTracks.forEach(function (t, i) { gl.appendChild(row(t, i, dv.genreTracks)); });
      }
      markPlaying();
      return;
    }
    if (!dv.data) {
      var wait = document.createElement('ul'); wait.className = 'mp-list'; box.appendChild(wait);
      var keep = el.list; el.list = wait; skeleton(); el.list = keep;
      return;
    }
    var d = dv.data;
    var genres = document.createElement('section');
    genres.className = 'mp-dvsection';
    var gh = document.createElement('h3'); gh.className = 'mp-dvtitle'; gh.textContent = 'Browse by genre';
    var grid = document.createElement('div'); grid.className = 'mp-genres';
    (d.genres || []).forEach(function (g) {
      var b = document.createElement('button');
      b.type = 'button'; b.className = 'mp-genre'; b.textContent = g.name;
      b.addEventListener('click', function () { openGenre(g); });
      grid.appendChild(b);
    });
    genres.append(gh, grid);
    box.appendChild(genres);
    if (d.top && d.top.length) box.appendChild(discoverSection('Top songs', d.top));
    if (!(d.top && d.top.length)) {
      var note = document.createElement('p'); note.className = 'mp-empty'; note.textContent = 'Top songs are unavailable right now. Genres still work.';
      box.appendChild(note);
    }
    markPlaying();
  }

  /* ---------------------------------------------------------- playlists */
  var plMode = 'mine', plQuery = '', plOpen = null, myPlaylists = null, plReq = 0;

  function on(node, ev, fn) { if (node) node.addEventListener(ev, fn); }
  function post(path, body) { return M.api(path, { body: body }); }

  /** A square of up to four cover thumbnails (or a note when there are none). */
  function buildMosaic(ids) {
    var covers = ids.slice(0, 4);
    var mosaic = document.createElement('span');
    mosaic.className = 'mp-plmosaic' + (covers.length < 4 ? ' one' : '');
    if (covers.length >= 4) {
      covers.forEach(function (id) {
        var tile = document.createElement('span'); tile.className = 'mp-mosaictile';
        var img = document.createElement('img'); img.alt = ''; img.loading = 'lazy'; img.src = M.thumb(id);
        tile.appendChild(img); mosaic.appendChild(tile);
      });
    } else if (covers.length) {
      var tile1 = document.createElement('span'); tile1.className = 'mp-mosaictile';
      var img1 = document.createElement('img'); img1.alt = ''; img1.loading = 'lazy'; img1.src = M.thumb(covers[0]);
      tile1.appendChild(img1); mosaic.appendChild(tile1);
    } else {
      var empty = document.createElement('span'); empty.className = 'mp-mosaicempty'; setIcon(empty, 'note');
      mosaic.appendChild(empty);
    }
    return mosaic;
  }

  /* A small pop-up list of actions, used instead of a row of buttons. */
  var openMenuEl = null;
  function closeMenu() {
    if (!openMenuEl) return;
    var a = openMenuEl._anchor;
    openMenuEl.remove(); openMenuEl = null;
    if (a) a.setAttribute('aria-expanded', 'false');
    document.removeEventListener('mousedown', onMenuOutside, true);
    document.removeEventListener('keydown', onMenuKey, true);
  }
  function onMenuOutside(e) {
    if (openMenuEl && !openMenuEl.contains(e.target) && !openMenuEl._anchor.contains(e.target)) closeMenu();
  }
  function onMenuKey(e) {
    if (e.key === 'Escape') { var a = openMenuEl && openMenuEl._anchor; closeMenu(); if (a) a.focus(); }
  }
  function openMenu(anchor, items) {
    if (openMenuEl && openMenuEl._anchor === anchor) { closeMenu(); return; }
    closeMenu();
    var ul = document.createElement('ul');
    ul.className = 'mp-menu'; ul.setAttribute('role', 'menu');
    items.forEach(function (it) {
      if (!it) return;
      var li = document.createElement('li'); li.setAttribute('role', 'none');
      var b = document.createElement('button');
      b.type = 'button'; b.className = 'mp-menuitem' + (it.danger ? ' mp-danger' : ''); b.setAttribute('role', 'menuitem');
      b.textContent = it.label;
      b.addEventListener('click', function () { closeMenu(); it.fn(); });
      li.appendChild(b); ul.appendChild(li);
    });
    document.body.appendChild(ul);
    var r = anchor.getBoundingClientRect();
    ul.style.left = Math.max(8, Math.min(r.left, innerWidth - ul.offsetWidth - 8)) + 'px';
    ul.style.top = (r.bottom + 6 + ul.offsetHeight > innerHeight - 8 ? Math.max(8, r.top - 6 - ul.offsetHeight) : r.bottom + 6) + 'px';
    ul._anchor = anchor; openMenuEl = ul;
    anchor.setAttribute('aria-expanded', 'true');
    document.addEventListener('mousedown', onMenuOutside, true);
    document.addEventListener('keydown', onMenuKey, true);
    var first = ul.querySelector('button'); if (first) first.focus();
  }

  /** A playlist as a card: a mosaic of its first covers, name and details. */
  function playlistRow(p) {
    var li = document.createElement('li');
    var card = document.createElement('button');
    card.type = 'button'; card.className = 'mp-plcard';
    var mosaic = buildMosaic(p.covers || []);
    var name = document.createElement('span'); name.className = 'mp-plcardname'; name.textContent = p.name;
    var meta = document.createElement('span'); meta.className = 'mp-plcardmeta'; meta.style.display = 'block';
    var bits = [p.trackCount + (p.trackCount === 1 ? ' song' : ' songs')];
    if (!p.mine) bits.push(p.owner.name); else bits.push(p.public ? 'Public' : 'Private');
    if (p.spotify) bits.push('Spotify');
    meta.textContent = bits.join(' · ');
    card.append(mosaic, name, meta);
    card.addEventListener('click', function () { openPlaylist(p.id); });
    li.appendChild(card);
    return li;
  }

  /* The sidebar lists the viewer's own playlists, like a music app's sidebar. */
  function renderSidePlaylists() {
    var box = $('mp-sidepl'), ul = $('mp-sideplist');
    if (!box || !ul) return;
    var list = myPlaylists || [];
    box.hidden = myPlaylists === null;
    ul.replaceChildren();
    if (!list.length) {
      var none = document.createElement('li');
      none.className = 'mp-sideplnone';
      none.textContent = 'No playlists yet';
      ul.appendChild(none);
    }
    list.forEach(function (p) {
      var li = document.createElement('li');
      var b = document.createElement('button');
      b.type = 'button'; b.className = 'mp-sidepl-item';
      if (plOpen && plOpen.p.id === p.id && tab === 'playlists') b.setAttribute('aria-current', 'true');
      var art = document.createElement('span'); art.className = 'mp-sideplart';
      if (p.covers && p.covers.length) { var im = document.createElement('img'); im.alt = ''; im.loading = 'lazy'; im.src = M.thumb(p.covers[0]); art.appendChild(im); }
      else setIcon(art, 'note');
      var name = document.createElement('span'); name.className = 'mp-sideplname'; name.textContent = p.name;
      b.append(art, name);
      b.addEventListener('click', function () { plMode = 'mine'; setTab('playlists'); openPlaylist(p.id); });
      li.appendChild(b); ul.appendChild(li);
    });
  }
  function refreshSidePlaylists() {
    return M.api('/api/music/playlists').then(function (d) { myPlaylists = d.playlists; renderSidePlaylists(); }).catch(function () { /* the sidebar just stays as it is */ });
  }
  on($('mp-sidepl-new'), 'click', function () { setTab('playlists'); newPlaylist(); });

  /** Songs in your playlists and library are saved on the server, up to a limit. */
  function fmtBytes(n) {
    if (n >= 1073741824) return (n / 1073741824).toFixed(1) + ' GB';
    return Math.round(n / 1048576) + ' MB';
  }
  function refreshStorage() {
    var line = $('mp-storage');
    if (!line) return;
    if (plMode !== 'mine') { line.hidden = true; return; }
    M.api('/api/music/storage').then(function (d) {
      if (!d.songs) { line.hidden = true; return; }
      line.textContent = 'Saved on the server: ' + fmtBytes(d.usedBytes) + ' of ' + fmtBytes(d.limitBytes) +
        ' (' + d.saved + ' of ' + d.songs + ' songs)';
      line.hidden = false;
    }).catch(function () { line.hidden = true; });
  }

  function renderPlaylists() {
    el.plHome.hidden = !!plOpen;
    el.plDetail.hidden = !plOpen;
    if (plOpen) { renderDetail(); return; }
    $('mp-seg-mine').setAttribute('aria-pressed', plMode === 'mine' ? 'true' : 'false');
    $('mp-seg-public').setAttribute('aria-pressed', plMode === 'public' ? 'true' : 'false');
    el.plSearch.hidden = plMode !== 'public';
    var url = plMode === 'mine' ? '/api/music/playlists' : '/api/music/playlists/public?q=' + encodeURIComponent(plQuery);
    refreshStorage();
    var req = ++plReq;
    M.api(url).then(function (d) {
      if (plOpen || req !== plReq) return;          // a newer request or a detail view took over
      if (plMode === 'mine') { myPlaylists = d.playlists; renderSidePlaylists(); }
      el.plList.replaceChildren();
      if (!d.playlists.length) {
        var empty = document.createElement('li');
        empty.className = 'mp-empty';
        empty.textContent = plMode === 'mine' ? 'No playlists yet' : 'No public playlists';
        el.plList.appendChild(empty);
        return;
      }
      d.playlists.forEach(function (p) { el.plList.appendChild(playlistRow(p)); });
    }).catch(function (e) { say(e.message); });
  }

  function shareLink(p, token) {
    return location.origin + '/music/?playlist=' + encodeURIComponent(p.id) + (token ? '&t=' + encodeURIComponent(token) : '');
  }

  function openPlaylist(id, token) {
    say('');
    return M.api('/api/music/playlist?id=' + encodeURIComponent(id) + (token ? '&t=' + encodeURIComponent(token) : '')).then(function (d) {
      plOpen = { p: d.playlist, tracks: d.tracks, token: token || '' };
      if (tab !== 'playlists') setTab('playlists'); else render();
      if (d.playlist.mine) refreshSidePlaylists();      // names, covers and the highlighted one
      if (d.playlist.mine && d.playlist.spotifyStale) syncSpotify(true);
    }).catch(function (e) {
      plOpen = null;
      if (tab !== 'playlists') setTab('playlists');
      say(e.status === 404 ? 'That playlist is private or no longer exists' : e.message);
    });
  }
  function reopen() { return plOpen ? openPlaylist(plOpen.p.id, plOpen.token) : Promise.resolve(); }

  function btn(label, onClick, cls) {
    var b = document.createElement('button');
    b.type = 'button'; b.className = 'mp-btn' + (cls ? ' ' + cls : '');
    b.textContent = label;
    b.addEventListener('click', onClick);
    return b;
  }

  /** A quiet text button, for secondary actions inside panels. */
  function linkBtn(label, onClick, cls) {
    var b = document.createElement('button');
    b.type = 'button'; b.className = 'mp-link' + (cls ? ' ' + cls : '');
    b.textContent = label;
    b.addEventListener('click', onClick);
    return b;
  }
  /** The big Play / Shuffle buttons of a playlist. */
  function bigBtn(label, iconName, onClick, primary) {
    var b = document.createElement('button');
    b.type = 'button'; b.className = 'mp-bigbtn' + (primary ? ' mp-primary' : '');
    var ic = document.createElement('span'); ic.className = 'mp-bigicon'; setIcon(ic, iconName);
    b.append(ic, document.createTextNode(label));
    b.addEventListener('click', onClick);
    return b;
  }

  function renderDetail() {
    var p = plOpen.p, tracks = plOpen.tracks;
    el.plName.textContent = p.name;
    el.plMeta.replaceChildren();
    var count = document.createElement('span');
    count.textContent = p.trackCount + (p.trackCount === 1 ? ' song' : ' songs') + ' · ' + (p.public ? 'Public' : 'Private') + (p.spotify ? ' · Synced with Spotify' : '');
    el.plMeta.appendChild(count);
    if (!p.mine) {
      var who = document.createElement('span');
      who.className = 'mp-owner';
      if (p.owner.pfp) {
        var img = document.createElement('img');
        img.src = p.owner.pfp; img.alt = ''; img.width = 20; img.height = 20; img.className = 'mp-ownerpic';
        who.appendChild(img);
      }
      var nm = document.createElement('span');
      nm.textContent = p.owner.name;
      if (p.owner.color) nm.style.color = p.owner.color;
      who.appendChild(nm);
      el.plMeta.appendChild(who);
    }

    var coverIds = [];
    tracks.forEach(function (t) { if (t.id && coverIds.indexOf(t.id) < 0 && coverIds.length < 4) coverIds.push(t.id); });
    if ($('mp-pl-cover')) $('mp-pl-cover').replaceChildren(buildMosaic(coverIds));

    el.plActions.replaceChildren();
    var playBtn = bigBtn('Play', 'play', function () { if (tracks.length) M.play(tracks, 0); }, true);
    var shuffleBtn = bigBtn('Shuffle', 'shuffle', function () { M.playShuffled(tracks); }, false);
    playBtn.disabled = shuffleBtn.disabled = !tracks.length;
    var more = document.createElement('button');
    more.type = 'button'; more.className = 'mp-icon mp-moremenu';
    more.title = 'More'; more.setAttribute('aria-label', 'More'); more.setAttribute('aria-haspopup', 'menu'); more.setAttribute('aria-expanded', 'false');
    setIcon(more, 'more');
    more.addEventListener('click', function () {
      openMenu(more, p.mine ? [
        { label: 'Share link\u2026', fn: toggleSharePanel },
        { label: p.public ? 'Make private' : 'Make public', fn: function () {
          post('/api/music/playlist/update', { id: p.id, public: !p.public }).then(reopen).catch(function (e) { say(e.message); });
        } },
        { label: 'Rename\u2026', fn: renamePlaylist },
        p.spotify ? { label: 'Resync from Spotify', fn: function () { syncSpotify(false); } } : null,
        { label: 'Delete playlist\u2026', fn: deletePlaylist, danger: true }
      ] : [
        { label: 'Save a copy', fn: function () {
          post('/api/music/playlist/copy', { id: p.id, t: plOpen.token }).then(function (d) {
            say('Saved to your playlists');
            plMode = 'mine';
            return openPlaylist(d.id);
          }).catch(function (e) { say(e.message); });
        } },
        { label: 'Message someone', fn: function () { sendInChat(p, plOpen.token); } }
      ]);
    });
    el.plActions.append(playBtn, shuffleBtn, more);
    renderSharePanel();

    el.plTracks.replaceChildren();
    if (!tracks.length) {
      var empty = document.createElement('li');
      empty.className = 'mp-empty';
      empty.textContent = p.mine ? 'Add songs with the playlist button on any search result' : 'This playlist is empty';
      el.plTracks.appendChild(empty);
    }
    tracks.forEach(function (t, i) {
      el.plTracks.appendChild(row(t, i, tracks, p.mine ? function (li, actions) {
        function small(name, label, fn, disabled) {
          var b = document.createElement('button');
          b.type = 'button'; b.className = 'mp-icon mp-small'; b.title = label; b.setAttribute('aria-label', label);
          b.disabled = !!disabled;
          setIcon(b, name);
          b.addEventListener('click', fn);
          return b;
        }
        actions.append(
          small('up', 'Move up', function () { moveTrack(i, i - 1); }, i === 0),
          small('down', 'Move down', function () { moveTrack(i, i + 1); }, i === tracks.length - 1),
          small('remove', 'Remove from playlist', function () {
            post('/api/music/playlist/tracks/remove', { id: p.id, position: i }).then(reopen).catch(function (e) { say(e.message); });
          })
        );
        li.classList.add('mp-editable');
      } : function (li) { li.classList.add('mp-readonly'); }));
    });
    markPlaying();
  }

  function moveTrack(from, to) {
    post('/api/music/playlist/tracks/move', { id: plOpen.p.id, from: from, to: to }).then(reopen).catch(function (e) { say(e.message); });
  }

  function renamePlaylist() {
    askForm({ title: 'Rename playlist', submit: 'Save', fields: [{ name: 'name', label: 'Name', value: plOpen.p.name, max: 80 }] }).then(function (v) {
      if (!v) return;
      post('/api/music/playlist/update', { id: plOpen.p.id, name: v.name }).then(reopen).catch(function (e) { say(e.message); });
    });
  }

  function deletePlaylist() {
    askForm({ title: 'Delete “' + plOpen.p.name + '”?', submit: 'Delete', fields: [], note: 'Links to it stop working.' }).then(function (v) {
      if (!v) return;
      post('/api/music/playlist/delete', { id: plOpen.p.id }).then(function () { plOpen = null; render(); refreshSidePlaylists(); }).catch(function (e) { say(e.message); });
    });
  }

  function syncSpotify(quiet) {
    if (!quiet) say('Syncing with Spotify\u2026');
    return post('/api/music/playlist/spotify/sync', { id: plOpen.p.id }).then(function (d) {
      var bits = [];
      if (d.added || d.removed) bits.push('Added ' + d.added + ', removed ' + d.removed);
      if (d.rechecked) bits.push('checking ' + d.rechecked + (d.rechecked === 1 ? ' song' : ' songs') + ' again');
      // Reopening clears the status line, so the message goes up after it.
      return reopen().then(function () { if (!quiet) say(bits.length ? bits.join(' \u00b7 ') : 'Already up to date'); });
    }).catch(function (e) { if (!quiet) { say(e.message); if (plOpen) renderDetail(); } });
  }

  /* -------------------------------------------------------------- sharing */
  var shareOpen = false;
  function toggleSharePanel() { shareOpen = !shareOpen; renderSharePanel(); }

  function renderSharePanel() {
    var box = el.plShare, p = plOpen.p;
    box.hidden = !(shareOpen && p.mine);
    if (box.hidden) return;
    box.replaceChildren();
    var token = p.public ? '' : (p.shareToken || '');
    if (!p.public && !token) {
      var note = document.createElement('p');
      note.textContent = 'Only you can see this playlist.';
      box.append(note, linkBtn('Create share link', function () {
        post('/api/music/playlist/share', { id: p.id, enable: true }).then(function (d) { p.shareToken = d.shareToken; plOpen.p.shareToken = d.shareToken; renderSharePanel(); }).catch(function (e) { say(e.message); });
      }));
      return;
    }
    var note2 = document.createElement('p');
    note2.textContent = p.public ? 'Anyone signed in can open this link.' : 'Anyone signed in with this link can open the playlist.';
    var input = document.createElement('input');
    input.type = 'text'; input.readOnly = true; input.value = shareLink(p, token); input.className = 'mp-sharelink';
    input.setAttribute('aria-label', 'Share link');
    var row2 = document.createElement('div');
    row2.className = 'mp-sharebtns';
    row2.append(
      linkBtn('Copy link', function () {
        function ok() { say('Link copied'); }
        if (navigator.clipboard && navigator.clipboard.writeText) navigator.clipboard.writeText(input.value).then(ok, function () { input.select(); });
        else { input.select(); document.execCommand('copy'); ok(); }
      }),
      linkBtn('Message someone', function () { sendInChat(p, token); })
    );
    if (!p.public) {
      row2.appendChild(linkBtn('Stop sharing', function () {
        post('/api/music/playlist/share', { id: p.id, enable: false }).then(function () { p.shareToken = ''; renderSharePanel(); say('Old links no longer work'); }).catch(function (e) { say(e.message); });
      }));
    }
    box.append(note2, input, row2);
  }

  /** Opens the chat with a "pick a person" prompt that drafts a DM with this playlist link. */
  function sendInChat(p, token) {
    location.href = '/matrix/?mshare=' + encodeURIComponent(shareLink(p, token)) + '&mname=' + encodeURIComponent(p.name);
  }

  /* ------------------------------------------------------ create / import */
  function newPlaylist() {
    askForm({ title: 'New playlist', submit: 'Create', fields: [
      { name: 'name', label: 'Name', max: 80 }, { name: 'visibility', label: 'Who can see it', options: VISIBILITY }
    ] }).then(function (v) {
      if (!v) return;
      post('/api/music/playlists/create', { name: v.name, public: v.visibility === 'public' }).then(function (d) {
        plMode = 'mine'; return openPlaylist(d.id);
      }).catch(function (e) { say(e.message); });
    });
  }

  /** The Spotify import dialog. A failure reopens it with the link still filled
   *  in and the reason shown in it, so it is obvious what to fix. */
  function importFromSpotify(prefillUrl, problem, prefillVisibility) {
    askForm({ title: 'Import from Spotify', submit: 'Import',
      note: problem || 'Works with public playlists. Spotify shares the first 100 songs.',
      problem: !!problem,
      fields: [{ name: 'url', label: 'Playlist link', placeholder: 'https://open.spotify.com/playlist/…', max: 300, value: prefillUrl || '' }, { name: 'visibility', label: 'Who can see it', options: VISIBILITY, value: prefillVisibility || '' }]
    }).then(function (v) {
      if (!v || !v.url) return;
      say('Importing from Spotify…');
      post('/api/music/playlist/spotify/import', { url: v.url, public: v.visibility === 'public' }).then(function (d) {
        say('Imported ' + d.tracks + ' songs');
        plMode = 'mine'; return openPlaylist(d.id);
      }).catch(function (e) {
        say('');
        if (e.status === 400) importFromSpotify(v.url, e.message, v.visibility);   // a problem with the link: fix it and retry
        else say(e.message);
      });
    });
  }
  on($('mp-pl-add'), 'click', function () {
    openMenu($('mp-pl-add'), [
      { label: 'New playlist\u2026', fn: newPlaylist },
      { label: 'Import from Spotify\u2026', fn: function () { importFromSpotify('', ''); } }
    ]);
  });
  on($('mp-seg-mine'), 'click', function () { plMode = 'mine'; renderPlaylists(); });
  on($('mp-seg-public'), 'click', function () { plMode = 'public'; renderPlaylists(); });
  on($('mp-pl-back'), 'click', function () { plOpen = null; shareOpen = false; render(); });
  on(el.plSearch, 'submit', function (e) { e.preventDefault(); plQuery = el.plQ.value.trim(); renderPlaylists(); });

  /** "Add to playlist" picker for one track. */
  function pickPlaylist(track) {
    var list = $('mp-pickerlist');
    list.replaceChildren();
    function added(n, name) { el.picker.close(); say(n ? 'Added to ' + name : 'Already in ' + name); if (plOpen && tab === 'playlists') reopen(); }
    function add(id, name) {
      post('/api/music/playlist/tracks/add', { id: id, tracks: [trackBody(track)] }).then(function (d) { added(d.added, name); }).catch(function (e) { el.picker.close(); say(e.message); });
    }
    M.api('/api/music/playlists').then(function (d) {
      d.playlists.forEach(function (p) {
        var li = document.createElement('li');
        li.className = 'mp-row mp-plrow';
        var b = document.createElement('button');
        b.type = 'button'; b.className = 'mp-rowmain';
        var t = document.createElement('span'); t.className = 'mp-rowtitle'; t.textContent = p.name;
        var a = document.createElement('span'); a.className = 'mp-rowartist'; a.textContent = p.trackCount + ' songs · ' + (p.public ? 'Public' : 'Private');
        b.append(t, a);
        b.addEventListener('click', function () { add(p.id, p.name); });
        li.appendChild(b);
        list.appendChild(li);
      });
    }).catch(function (e) { say(e.message); });
    $('mp-pickernew').onclick = function () {
      el.picker.close();
      askForm({ title: 'New playlist', submit: 'Create and add', fields: [{ name: 'name', label: 'Name', max: 80 }, { name: 'visibility', label: 'Who can see it', options: VISIBILITY }] }).then(function (v) {
        if (!v) return;
        post('/api/music/playlists/create', { name: v.name, public: v.visibility === 'public', tracks: [trackBody(track)] }).then(function () { say('Added to ' + v.name); }).catch(function (e) { say(e.message); });
      });
    };
    $('mp-pickerclose').onclick = function () { el.picker.close(); };
    el.picker.showModal();
  }


  /* -------------------------------------------------------------- search */
  var searchForm = $('mp-search');
  if (searchForm) {
    // Searches as you type: after a short pause, and only for a query not
    // already shown. Answers that arrive out of order are dropped.
    var searchSeq = 0, lastQuery = '', typingTimer = 0;
    var runSearch = function (force) {
      var q = el.q.value.trim();
      if (!q) { searchSeq++; lastQuery = ''; searching = false; results = []; say(''); render(); return; }
      if (q.length < 2 || (q === lastQuery && !force)) return;
      lastQuery = q;
      var mine = ++searchSeq;
      setTab('search');
      searching = true;
      render();
      M.api('/api/music/search?q=' + encodeURIComponent(q)).then(function (d) {
        if (mine !== searchSeq) return;
        searching = false;
        results = d.tracks || [];
        say('');
        render();
      }).catch(function (err) {
        if (mine !== searchSeq) return;
        lastQuery = '';
        searching = false;
        results = []; render(); say(err.message);
      });
    };
    searchForm.addEventListener('submit', function (e) { e.preventDefault(); clearTimeout(typingTimer); runSearch(true); });
    el.q.addEventListener('input', function () { clearTimeout(typingTimer); typingTimer = setTimeout(runSearch, 650); });
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
  if (el.addPl) el.addPl.addEventListener('click', function () { var t = M.current(); if (t) pickPlaylist(t); });
  el.seek.addEventListener('input', function () {
    seeking = true;
    var d = M.state.dur || 0;
    el.cur.textContent = M.fmt((el.seek.value / 1000) * d);
    el.seek.style.setProperty('--p', (el.seek.value / 10) + '%');
  });
  el.seek.addEventListener('change', function () {
    var d = M.state.dur || 0;
    if (d > 0) M.seek((el.seek.value / 1000) * d);
    seeking = false;
  });
  el.vol.addEventListener('input', function () { M.volume(el.vol.value / 100); el.vol.style.setProperty('--p', el.vol.value + '%'); });
  // Shuffle cycles: off, random (any song, any time), smart (every song once,
  // artists mixed, a suggestion now and then).
  var SHUFFLE_NEXT = { off: 'random', random: 'smart', smart: 'off' };
  var SHUFFLE_SAY = { off: 'Shuffle off', random: 'Shuffle: random', smart: 'Smart shuffle: plays every song once, mixes artists, adds suggestions' };
  if (el.shuffle) el.shuffle.addEventListener('click', function () {
    var next = SHUFFLE_NEXT[M.state.shuffle] || 'random';
    M.shuffle(next);
    say(SHUFFLE_SAY[next]);
  });
  var XF_STEPS = [0, 3, 6, 9];
  if ($('mp-xfade')) $('mp-xfade').addEventListener('click', function () {
    var at = XF_STEPS.indexOf(M.state.crossfade);
    M.crossfade(XF_STEPS[(at + 1) % XF_STEPS.length]);
  });
  if (el.repeat) el.repeat.addEventListener('click', function () { M.repeat(M.state.repeat === 'off' ? 'all' : M.state.repeat === 'all' ? 'one' : 'off'); });

  function loadLyrics(track, custom) {
    lv.set('loading');
    el.info.textContent = '';
    el.wrong.hidden = true;
    M.getLyrics(track, custom).then(function (d) {
      if (!M.current() || M.current().id !== track.id) return;
      lv.set(d);
      el.wrong.hidden = false;
      showNudge(d);
      if (d && d.found) {
        var how = d.sync === 'word' ? 'word by word' : d.sync === 'line' ? (d.estimated ? 'line timing, words estimated' : 'line by line') : 'text only';
        el.info.textContent = 'Lyrics from ' + d.source + ' · ' + how + (d.approximate ? ' · timing is for the full song' : '');
      } else {
        el.info.textContent = '';
      }
    });
  }
  /* A way to fix lyrics that are early or late for this song: shown only for
   * synced lyrics, remembered in this browser. */
  function showNudge(d) {
    var nudge = $('mp-nudge'), t = M.current();
    if (!nudge) return;
    nudge.hidden = !(d && d.found && d.sync !== 'none');
    var off = t ? M.lyricOffset(t.id) : 0;
    $('mp-lyroff').textContent = off ? 'Timing ' + (off > 0 ? '+' : '') + (off / 1000).toFixed(1) + 's' : 'Timing';
  }
  function nudge(by) {
    var t = M.current();
    if (!t) return;
    M.setLyricOffset(t.id, M.lyricOffset(t.id) + by);
    loadLyrics(t);
  }
  if ($('mp-lyrbk')) $('mp-lyrbk').addEventListener('click', function () { nudge(-500); });
  if ($('mp-lyrfw')) $('mp-lyrfw').addEventListener('click', function () { nudge(500); });
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
    var wantTitle = el.wt.value.trim() || t.title, wantArtist = el.wa.value.trim();
    // Sent again as it was: a retry of the normal lookup, which may improve the
    // lyrics everyone gets (a slow source can have missed the first try).
    loadLyrics(t, { title: wantTitle, artist: wantArtist, retry: wantTitle === t.title && wantArtist === t.artist });
    el.wrongForm.hidden = true;
  });

  function onState(s) {
    var t = M.current();
    setText(el.title, t ? t.title : 'Nothing playing');
    setText(el.artist, t ? t.artist : '');
    el.art.hidden = !t;
    if (t && t.id && shownId !== t.id) {
      shownId = t.id;
      el.art.src = M.thumb(t.id);
      if (el.ambient) el.ambient.style.setProperty('--cover', 'url("' + M.thumb(t.id) + '")');
      el.wrongForm.hidden = true;
      loadLyrics(t);
    }
    if (t && !t.id) el.art.hidden = true;
    if (!t && shownId) { shownId = ''; lv.set('idle'); el.info.textContent = ''; el.wrong.hidden = true; }
    if (!t && !shownId) lv.set('idle');
    if (el.stage) {
      el.stage.classList.toggle('is-empty', !t);
      el.stage.classList.toggle('is-playing', !!s.playing);
    }
    if (el.shuffle) {
      setAttr(el.shuffle, 'aria-pressed', s.shuffle !== 'off' ? 'true' : 'false');
      el.shuffle.dataset.mode = s.shuffle;
      var sl = s.shuffle === 'smart' ? 'Smart shuffle on' : s.shuffle === 'random' ? 'Shuffle on (random)' : 'Shuffle off';
      if (el.shuffle.title !== sl) el.shuffle.title = sl;
      setAttr(el.shuffle, 'aria-label', sl);
    }
    var xf = $('mp-xfade');
    if (xf) {
      var xl = s.crossfade ? 'Fade ' + s.crossfade + 's' : 'Fade off';
      if (xf.hidden === !!s.xfOk) xf.hidden = !s.xfOk;      // not offered where the device can't do it
      if (xf.textContent !== xl) xf.textContent = xl;
      xf.dataset.on = s.crossfade ? 'true' : 'false';
    }
    if (el.repeat) {
      el.repeat.dataset.mode = s.repeat;
      var rl = s.repeat === 'one' ? 'Repeat this song' : s.repeat === 'all' ? 'Repeat all' : 'Repeat off';
      if (el.repeat.title !== rl) el.repeat.title = rl;
      setAttr(el.repeat, 'aria-label', rl);
      setAttr(el.repeat, 'aria-pressed', s.repeat === 'off' ? 'false' : 'true');
    }
    var loading = !!s.loading && !s.playing && !s.error;
    setIcon(el.play, s.playing ? 'pause' : 'play');
    el.play.classList.toggle('mp-loading', loading);
    setAttr(el.play, 'aria-busy', loading ? 'true' : 'false');
    var label = loading ? 'Loading' : s.playing ? 'Pause' : 'Play';
    setAttr(el.play, 'aria-label', label);
    if (el.play.title !== label) el.play.title = label;
    if (document.activeElement !== el.vol) { el.vol.value = Math.round(s.vol * 100); el.vol.style.setProperty('--p', Math.round(s.vol * 100) + '%'); }
    var msg = s.error ? s.error : loading ? 'Loading…' : s.blocked ? 'Press play to continue'
      : (!popout && s.popoutOpen && s.role === 'remote') ? 'Playing in the pop-out window' : '';
    if (msg || (el.status.dataset.owner === 'state')) { say(msg); el.status.dataset.owner = msg ? 'state' : ''; }
    syncNowFav();
    markPlaying();
    if (tab === 'queue' && el.list) {
      var sig = s.queue.length + ':' + s.index + ':' + (s.queue[0] && s.queue[0].id) + ':' + (s.queue[s.queue.length - 1] && s.queue[s.queue.length - 1].id);
      if (sig !== queueSig) { queueSig = sig; render(); }
    }
  }

  function frame() {
    var s = M.state, time = M.getTime();
    if (!seeking) {
      el.cur.textContent = M.fmt(time);
      el.dur.textContent = M.fmt(s.dur);
      el.seek.value = s.dur > 0 ? Math.round((time / s.dur) * 1000) : 0;
      el.seek.style.setProperty('--p', (el.seek.value / 10) + '%');
    }
    lv.tick(time * 1000);
    requestAnimationFrame(frame);
  }

  document.addEventListener('keydown', function (e) {
    if (e.target && /^(INPUT|TEXTAREA|SELECT)$/.test(e.target.tagName)) return;
    if (e.key === ' ') { e.preventDefault(); M.toggle(); }
    else if (e.key === 'ArrowRight') M.seek(M.getTime() + 5);
    else if (e.key === 'ArrowLeft') M.seek(Math.max(0, M.getTime() - 5));
  });

  // The follow-along card is for other pages. This page is the player, so make
  // sure no card is ever shown here, whatever loaded it.
  document.querySelectorAll('.mm-card').forEach(function (c) { c.remove(); });
  M.init({ popout: popout });
  M.onChange(onState);
  render();                 // the empty state of the first tab
  loadLibrary();
  refreshSidePlaylists();
  var params = new URLSearchParams(location.search);
  if (!popout && params.get('playlist')) openPlaylist(params.get('playlist'), params.get('t') || '');
  requestAnimationFrame(frame);
})();
