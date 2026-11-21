/* Music follow-along card (docs/music.md). Injected on every page by the
 * server. It does nothing, and makes no requests, until the visitor has
 * started the player; after that it shows a small card on each page with the
 * playback controls and the current lyric line. */
(function () {
  'use strict';
  if (window.top !== window.self) return;                    // not inside iframes
  if (window.__mitchMusicWidget) return;
  window.__mitchMusicWidget = true;
  // The player pages are the player: no second one on them. Checked three ways
  // (the path, the page's class, the stage element) so a different URL or a
  // cached older page can't slip through.
  if (location.pathname.indexOf('/music') === 0) return;
  if (document.body && document.body.classList.contains('music-page')) return;
  if (document.getElementById('mp-stage')) return;

  var saved = null;
  try { saved = JSON.parse(localStorage.getItem('mitch.music.v1') || 'null'); } catch (e) { /* ignore */ }
  if (!saved || !saved.queue || !saved.queue.length || saved.index < 0) return;

  function load(tag, attrs) {
    var el = document.createElement(tag);
    Object.keys(attrs).forEach(function (k) { el[k] = attrs[k]; });
    document.head.appendChild(el);
    return el;
  }

  var ICONS = {
    play: 'M8 5v14l11-7z',
    pause: 'M6 5h4v14H6zm8 0h4v14h-4z',
    prev: 'M6 6h2v12H6zm3.5 6L18 18V6z',
    next: 'M16 6h2v12h-2zM6 18l8.5-6L6 6z',
    popout: 'M14 3h7v7h-2V6.4l-8.3 8.3-1.4-1.4L17.6 5H14zM5 5h6v2H6v11h11v-5h2v7H4V5z',
    open: 'M12 3v10.55A4 4 0 1 0 14 17V7h4V3z',
    min: 'M5 11h14v2H5z',
    max: 'M5 5h14v14H5V5zm2 2v10h10V7z',
    close: 'M6.4 5 5 6.4 10.6 12 5 17.6 6.4 19 12 13.4 17.6 19 19 17.6 13.4 12 19 6.4 17.6 5 12 10.6z'
  };
  function icon(name) {
    var svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
    svg.setAttribute('viewBox', '0 0 24 24');
    svg.setAttribute('aria-hidden', 'true');
    var path = document.createElementNS('http://www.w3.org/2000/svg', 'path');
    path.setAttribute('d', ICONS[name]);
    svg.appendChild(path);
    return svg;
  }
  function button(name, label, onClick) {
    var b = document.createElement('button');
    b.type = 'button';
    b.className = 'mm-btn mm-' + name;
    b.setAttribute('aria-label', label);
    b.title = label;
    b.appendChild(icon(name));
    b.addEventListener('click', onClick);
    return b;
  }

  function build() {
    var M = window.MitchMusic;
    var card = document.createElement('div');
    card.className = 'mm-card';
    card.setAttribute('role', 'region');
    card.setAttribute('aria-label', 'Music player');
    card.hidden = true;

    var artBox = document.createElement('div');
    artBox.className = 'mm-artbox';
    var art = document.createElement('img');
    art.className = 'mm-art';
    art.alt = '';
    artBox.appendChild(art);
    var meta = document.createElement('div');
    meta.className = 'mm-meta';
    var title = document.createElement('div'); title.className = 'mm-title';
    var artist = document.createElement('div'); artist.className = 'mm-artist';
    meta.append(title, artist);

    var play = button('play', 'Play', function () { M.toggle(); });
    var prev = button('prev', 'Previous', function () { M.prev(); });
    var next = button('next', 'Next', function () { M.next(); });
    var pop = button('popout', 'Pop out player', function () {
      if (!M.popout()) status.textContent = 'Pop-up was blocked by the browser';
    });
    var openPage = button('open', 'Open music page', function () { location.href = '/music/'; });
    var min = button('min', 'Minimize', function () { setMin(!card.classList.contains('mm-min')); });
    var close = button('close', 'Close player', function () { M.stop(); });

    var btns = document.createElement('div');
    btns.className = 'mm-btns';
    btns.append(prev, play, next);
    var tools = document.createElement('div');
    tools.className = 'mm-tools';
    tools.append(pop, openPage, min, close);

    var bar = document.createElement('div');
    bar.className = 'mm-bar';
    bar.setAttribute('role', 'slider');
    bar.setAttribute('aria-label', 'Seek');
    bar.tabIndex = 0;
    var fill = document.createElement('i');
    bar.appendChild(fill);
    bar.addEventListener('click', function (e) {
      var r = bar.getBoundingClientRect(), s = M.state;
      if (s.dur > 0) M.seek(((e.clientX - r.left) / r.width) * s.dur);
    });
    bar.addEventListener('keydown', function (e) {
      if (e.key === 'ArrowRight') M.seek(M.getTime() + 5);
      else if (e.key === 'ArrowLeft') M.seek(Math.max(0, M.getTime() - 5));
    });

    var lyric = document.createElement('div');
    lyric.className = 'mm-lyric';
    lyric.setAttribute('aria-live', 'off');
    var status = document.createElement('div');
    status.className = 'mm-status';
    status.setAttribute('role', 'status');

    var head = document.createElement('div');
    head.className = 'mm-head';
    head.append(artBox, meta, btns, tools);
    card.append(head, bar, lyric, status);
    document.body.appendChild(card);

    function setMin(v) {
      card.classList.toggle('mm-min', v);
      min.replaceChildren(icon(v ? 'max' : 'min'));
      min.title = v ? 'Expand' : 'Minimize';
      try { localStorage.setItem('mitch.music.card.min', v ? '1' : '0'); } catch (e) { /* ignore */ }
    }
    var wasMin = false;
    try { wasMin = localStorage.getItem('mitch.music.card.min') === '1'; } catch (e) { /* ignore */ }
    setMin(wasMin);

    var shownId = '', lyrics = null, lastLine = -2;
    function render(s) {
      var t = M.current();
      if (!t) { card.hidden = true; return; }
      card.hidden = false;
      if (title.textContent !== t.title) title.textContent = t.title;
      if (artist.textContent !== t.artist) artist.textContent = t.artist;
      if (t.id && shownId !== t.id) {          // tracks from Spotify have no id until matched
        shownId = t.id; lastLine = -2; lyrics = null; lyric.textContent = '';
        art.src = M.thumb(t.id);
        M.getLyrics(t).then(function (d) { if (shownId === t.id && d && d.found && d.sync !== 'none') lyrics = d; });
      }
      var loading = !!s.loading && !s.playing && !s.error;
      var label = loading ? 'Loading' : s.playing ? 'Pause' : 'Play';
      if (play.getAttribute('aria-label') !== label) { play.setAttribute('aria-label', label); play.title = label; }
      play.setAttribute('aria-busy', loading ? 'true' : 'false');
      play.classList.toggle('mm-loading', loading);
      var want = s.playing ? 'pause' : 'play';
      if (play.dataset.icon !== want) { play.dataset.icon = want; play.replaceChildren(icon(want)); }   // not on every update: it can swallow a click
      card.classList.toggle('mm-busy', !!s.buffering && !s.error);
      var msg = s.error ? s.error
        : loading ? 'Loading…'
        : s.blocked ? 'Press play to continue'
        : s.popoutOpen && s.role === 'remote' ? 'Playing in the pop-out window' : '';
      if (!status.textContent || status.textContent !== msg) status.textContent = msg;
      status.hidden = !msg;
    }

    function frame() {
      var s = M.state;
      if (!card.hidden && s.dur > 0) fill.style.width = Math.min(100, (M.getTime() / s.dur) * 100) + '%';
      if (!card.hidden && lyrics) {
        var i = M.lineAt(lyrics.lines, M.getTime() * 1000);
        if (i !== lastLine) {
          lastLine = i;
          lyric.textContent = i >= 0 ? lyrics.lines[i].text : '';
        }
      }
      requestAnimationFrame(frame);
    }

    M.onChange(render);
    requestAnimationFrame(frame);
  }

  load('link', { rel: 'stylesheet', href: '/music-widget.css?v=4' });
  var core = load('script', { src: '/music-core.js?v=10' });
  core.onload = function () {
    window.MitchMusic.init();
    build();
  };
})();
