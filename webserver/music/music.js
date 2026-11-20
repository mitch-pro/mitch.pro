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
    queue: 'M4 6h16v2H4zm0 5h10v2H4zm0 5h10v2H4zm12-5v6.2a3 3 0 1 0 2 2.8V13h2v-2z',
    listadd: 'M3 6h12v2H3zm0 5h12v2H3zm0 5h8v2H3zm13-1v-3h2v3h3v2h-3v3h-2v-3h-3v-2z',
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
  function setIcon(el, name) { el.replaceChildren(icon(name)); }

  var el = {
    art: $('mp-art'), title: $('mp-title'), artist: $('mp-artist'), fav: $('mp-fav'), pop: $('mp-pop'),
    prev: $('mp-prev'), play: $('mp-play'), next: $('mp-next'), cur: $('mp-cur'), dur: $('mp-dur'),
    seek: $('mp-seek'), vol: $('mp-vol'), lyrics: $('mp-lyrics'), info: $('mp-lyricsinfo'),
    wrong: $('mp-wrong'), wrongForm: $('mp-wrongform'), wt: $('mp-wt'), wa: $('mp-wa'),
    status: $('mp-status'), list: $('mp-list'), q: $('mp-q'),
    plPanel: $('mp-playlists'), plHome: $('mp-plhome'), plList: $('mp-pllist'), plDetail: $('mp-pldetail'),
    plName: $('mp-pl-name'), plMeta: $('mp-pl-meta'), plActions: $('mp-pl-actions'), plShare: $('mp-pl-share'),
    plTracks: $('mp-pltracks'), plSearch: $('mp-plsearch'), plQ: $('mp-plq'),
    dialog: $('mp-dialog'), picker: $('mp-picker')
  };
  setIcon(el.prev, 'prev'); setIcon(el.play, 'play'); setIcon(el.next, 'next'); setIcon(el.fav, 'heart');
  if (el.pop) setIcon(el.pop, 'pop');

  var lv = M.createLyricsView(el.lyrics, { onSeek: function (s) { M.seek(s); } });
  var results = [], library = [], favIds = {}, tab = 'search', shownId = '', seeking = false;

  /* ---------------------------------------------------------- list views */
  function say(text) { el.status.textContent = text || ''; }

  function row(track, i, list, extra) {
    var li = document.createElement('li');
    li.className = 'mp-row';
    li.dataset.id = track.id || '';
    var img = document.createElement('img');
    img.className = 'mp-thumb'; img.alt = ''; img.loading = 'lazy'; img.width = 48; img.height = 48;
    if (track.id) img.src = M.thumb(track.id);
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
    li.append(img, main, d, add);
    if (extra) { extra(li); return li; }
    var toPl = document.createElement('button');
    toPl.type = 'button'; toPl.className = 'mp-icon mp-small'; toPl.title = 'Add to playlist'; toPl.setAttribute('aria-label', 'Add to playlist');
    setIcon(toPl, 'listadd');
    toPl.addEventListener('click', function () { pickPlaylist(track); });
    var heart = document.createElement('button');
    heart.type = 'button'; heart.className = 'mp-icon mp-small mp-heart'; setIcon(heart, 'heart');
    syncHeart(heart, track.id);
    heart.addEventListener('click', function () { toggleFav(track).then(function () { syncHeart(heart, track.id); }); });
    li.append(toPl, heart);
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
    el.list.hidden = tab === 'playlists';
    el.plPanel.hidden = tab !== 'playlists';
    if (tab === 'playlists') { renderPlaylists(); return; }
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
    [el.list, el.plTracks].forEach(function (ul) {
      Array.prototype.forEach.call(ul.children, function (li) {
        var mine = !!cur && !!cur.id && li.dataset.id === cur.id;
        li.classList.toggle('mp-playing', mine);
        li.classList.toggle('mp-fetching', mine && !!M.state.buffering && !M.state.playing && !M.state.error);
      });
    });
  }

  function setTab(name) {
    tab = name;
    ['search', 'library', 'playlists', 'queue'].forEach(function (n) {
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
    el.fav.hidden = !t || !t.id;
    if (t && t.id) syncHeart(el.fav, t.id);
  }

  /* ------------------------------------------------------------ dialogs */
  /** Small form dialog. Resolves to {field: value} on submit, null on cancel. */
  function askForm(opts) {
    var d = el.dialog;
    $('mp-dialogtitle').textContent = opts.title;
    $('mp-dialognote').textContent = opts.note || '';
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
      input.value = f.value || '';
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

  /* ---------------------------------------------------------- playlists */
  var plMode = 'mine', plQuery = '', plOpen = null, myPlaylists = null, plReq = 0;

  function on(node, ev, fn) { if (node) node.addEventListener(ev, fn); }
  function post(path, body) { return M.api(path, { body: body }); }

  function playlistRow(p) {
    var li = document.createElement('li');
    li.className = 'mp-row mp-plrow';
    var main = document.createElement('button');
    main.type = 'button'; main.className = 'mp-rowmain';
    var t = document.createElement('span'); t.className = 'mp-rowtitle'; t.textContent = p.name;
    var a = document.createElement('span'); a.className = 'mp-rowartist';
    var bits = [p.trackCount + (p.trackCount === 1 ? ' song' : ' songs'), p.public ? 'Public' : 'Private'];
    if (p.spotify) bits.push('Spotify');
    if (!p.mine) bits.push('by ' + p.owner.name);
    a.textContent = bits.join(' · ');
    main.append(t, a);
    main.addEventListener('click', function () { openPlaylist(p.id); });
    li.appendChild(main);
    return li;
  }

  function renderPlaylists() {
    el.plHome.hidden = !!plOpen;
    el.plDetail.hidden = !plOpen;
    if (plOpen) { renderDetail(); return; }
    $('mp-seg-mine').setAttribute('aria-pressed', plMode === 'mine' ? 'true' : 'false');
    $('mp-seg-public').setAttribute('aria-pressed', plMode === 'public' ? 'true' : 'false');
    el.plSearch.hidden = plMode !== 'public';
    var url = plMode === 'mine' ? '/api/music/playlists' : '/api/music/playlists/public?q=' + encodeURIComponent(plQuery);
    var req = ++plReq;
    M.api(url).then(function (d) {
      if (plOpen || req !== plReq) return;          // a newer request or a detail view took over
      if (plMode === 'mine') myPlaylists = d.playlists;
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

    el.plActions.replaceChildren();
    el.plActions.appendChild(btn('Play', function () { if (tracks.length) M.play(tracks, 0); }, 'mp-primary'));
    if (p.mine) {
      el.plActions.appendChild(btn('Share', toggleSharePanel));
      el.plActions.appendChild(btn('Rename', renamePlaylist));
      var vis = btn(p.public ? 'Make private' : 'Make public', function () {
        post('/api/music/playlist/update', { id: p.id, public: !p.public }).then(reopen).catch(function (e) { say(e.message); });
      });
      el.plActions.appendChild(vis);
      if (p.spotify) el.plActions.appendChild(btn('Sync from Spotify', function () { syncSpotify(false); }));
      el.plActions.appendChild(btn('Delete', deletePlaylist, 'mp-danger'));
    } else {
      el.plActions.appendChild(btn('Save a copy', function () {
        post('/api/music/playlist/copy', { id: p.id, t: plOpen.token }).then(function (d) {
          say('Saved to your playlists');
          plMode = 'mine';
          return openPlaylist(d.id);
        }).catch(function (e) { say(e.message); });
      }));
      el.plActions.appendChild(btn('Message someone', function () { sendInChat(p, plOpen.token); }));
    }
    renderSharePanel();

    el.plTracks.replaceChildren();
    if (!tracks.length) {
      var empty = document.createElement('li');
      empty.className = 'mp-empty';
      empty.textContent = p.mine ? 'Add songs with the playlist button on any search result' : 'This playlist is empty';
      el.plTracks.appendChild(empty);
    }
    tracks.forEach(function (t, i) {
      el.plTracks.appendChild(row(t, i, tracks, p.mine ? function (li) {
        function small(name, label, fn, disabled) {
          var b = document.createElement('button');
          b.type = 'button'; b.className = 'mp-icon mp-small'; b.title = label; b.setAttribute('aria-label', label);
          b.disabled = !!disabled;
          setIcon(b, name);
          b.addEventListener('click', fn);
          return b;
        }
        li.append(
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
      post('/api/music/playlist/delete', { id: plOpen.p.id }).then(function () { plOpen = null; render(); }).catch(function (e) { say(e.message); });
    });
  }

  function syncSpotify(quiet) {
    if (!quiet) say('Syncing with Spotify…');
    return post('/api/music/playlist/spotify/sync', { id: plOpen.p.id }).then(function (d) {
      if (!quiet) say(d.added || d.removed ? 'Added ' + d.added + ', removed ' + d.removed : 'Already up to date');
      return reopen();
    }).catch(function (e) { if (!quiet) say(e.message); });
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
      box.append(note, btn('Create share link', function () {
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
      btn('Copy link', function () {
        function ok() { say('Link copied'); }
        if (navigator.clipboard && navigator.clipboard.writeText) navigator.clipboard.writeText(input.value).then(ok, function () { input.select(); });
        else { input.select(); document.execCommand('copy'); ok(); }
      }),
      btn('Message someone', function () { sendInChat(p, token); })
    );
    if (!p.public) {
      row2.appendChild(btn('Stop sharing', function () {
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
  on($('mp-pl-new'), 'click', function () {
    askForm({ title: 'New playlist', submit: 'Create', fields: [
      { name: 'name', label: 'Name', max: 80 }, { name: 'visibility', label: 'Who can see it', options: VISIBILITY }
    ] }).then(function (v) {
      if (!v) return;
      post('/api/music/playlists/create', { name: v.name, public: v.visibility === 'public' }).then(function (d) {
        plMode = 'mine'; return openPlaylist(d.id);
      }).catch(function (e) { say(e.message); });
    });
  });
  on($('mp-pl-spotify'), 'click', function () {
    askForm({ title: 'Import from Spotify', submit: 'Import',
      note: 'Works with public playlists. Spotify shares the first 100 songs.',
      fields: [{ name: 'url', label: 'Playlist link', placeholder: 'https://open.spotify.com/playlist/…', max: 300 }, { name: 'visibility', label: 'Who can see it', options: VISIBILITY }]
    }).then(function (v) {
      if (!v || !v.url) return;
      say('Importing from Spotify…');
      post('/api/music/playlist/spotify/import', { url: v.url, public: v.visibility === 'public' }).then(function (d) {
        say('Imported ' + d.tracks + ' songs');
        plMode = 'mine'; return openPlaylist(d.id);
      }).catch(function (e) { say(e.message); });
    });
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
        el.info.textContent = 'Lyrics from ' + d.source + ' · ' + how + (d.approximate ? ' · timing is for the full song' : '');
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
    if (t && t.id && shownId !== t.id) {
      shownId = t.id;
      el.art.src = M.thumb(t.id);
      el.wrongForm.hidden = true;
      loadLyrics(t);
    }
    if (t && !t.id) el.art.hidden = true;
    if (!t && shownId) { shownId = ''; lv.set(null); el.info.textContent = ''; el.wrong.hidden = true; }
    var loading = !!s.buffering && !s.playing && !s.error;
    setIcon(el.play, s.playing ? 'pause' : 'play');
    el.play.classList.toggle('mp-loading', loading);
    el.play.setAttribute('aria-busy', loading ? 'true' : 'false');
    var label = loading ? 'Loading' : s.playing ? 'Pause' : 'Play';
    el.play.setAttribute('aria-label', label);
    el.play.title = label;
    if (document.activeElement !== el.vol) el.vol.value = Math.round(s.vol * 100);
    var msg = s.error ? s.error : loading ? 'Loading…' : s.blocked ? 'Press play to continue'
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
  var params = new URLSearchParams(location.search);
  if (!popout && params.get('playlist')) openPlaylist(params.get('playlist'), params.get('t') || '');
  requestAnimationFrame(frame);
})();
