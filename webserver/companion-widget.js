(function () {
  'use strict';
  if (window.top !== window.self) return; // never render inside an iframe (proxy pages, embeds)

  var state = { enabled: false, activeCharacterId: '', characters: [] };
  var el = { root: null, sprite: null, bubble: null };
  var phys = { x: 14, y: 0, vx: 0, vy: 0, dragging: false, lastMove: 0 };
  var imgCache = {};
  var currentMood = 'idle';
  var moods = ['idle'];
  var rafId = null;

  function api(url, opts) {
    opts = opts || {};
    var headers = Object.assign({ 'X-Mitch-Requested-With': '1' }, opts.headers || {});
    if (opts.body) headers['Content-Type'] = 'application/json';
    return fetch(url, Object.assign({ credentials: 'same-origin', cache: 'no-store' }, opts, { headers: headers }))
      .then(function (r) { return r.json().then(function (data) { return { ok: r.ok, data: data }; }); });
  }

  function imageUrl(id, mood) {
    return '/api/companion/character-image?id=' + encodeURIComponent(id) + '&mood=' + encodeURIComponent(mood);
  }

  function preload(id, moodList) {
    moodList.forEach(function (m) {
      if (imgCache[m]) return;
      var img = new Image();
      img.src = imageUrl(id, m);
      imgCache[m] = img;
    });
  }

  function renderCompanion() {
    if (el.root) {
      el.root.remove();
      el.root = null;
      cancelAnimationFrame(rafId);
    }
    if (!state.enabled || !state.activeCharacterId) return;
    var character = state.characters.filter(function (c) { return c.id === state.activeCharacterId; })[0];
    moods = (character && character.moods && character.moods.length) ? character.moods : ['idle'];
    imgCache = {};
    preload(state.activeCharacterId, moods);

    var root = document.createElement('div');
    root.className = 'companion-root';
    var sprite = document.createElement('img');
    sprite.className = 'companion-sprite';
    currentMood = moods.indexOf('idle') !== -1 ? 'idle' : moods[0];
    sprite.src = imageUrl(state.activeCharacterId, currentMood);
    sprite.alt = character ? character.name : 'Companion';
    root.appendChild(sprite);
    document.body.appendChild(root);
    el.root = root;
    el.sprite = sprite;

    phys.x = 14;
    phys.y = window.innerHeight - 110;
    phys.vx = 0;
    phys.vy = 0;
    wireDrag(root);
    wireChat(root);
    startPhysics();
  }

  function setMood(mood) {
    if (!el.sprite || mood === currentMood) return;
    if (moods.indexOf(mood) === -1) return;
    currentMood = mood;
    el.sprite.src = imageUrl(state.activeCharacterId, mood);
  }

  function wireDrag(root) {
    var offsetX = 0, offsetY = 0, lastX = 0, lastY = 0, lastT = 0;
    root.addEventListener('pointerdown', function (e) {
      phys.dragging = true;
      root.classList.add('dragging');
      root.setPointerCapture(e.pointerId);
      offsetX = e.clientX - phys.x;
      offsetY = e.clientY - phys.y;
      lastX = e.clientX;
      lastY = e.clientY;
      lastT = performance.now();
    });
    root.addEventListener('pointermove', function (e) {
      if (!phys.dragging) return;
      phys.x = e.clientX - offsetX;
      phys.y = e.clientY - offsetY;
      var now = performance.now();
      var dt = Math.max(1, now - lastT);
      phys.vx = ((e.clientX - lastX) / dt) * 16;
      phys.vy = ((e.clientY - lastY) / dt) * 16;
      lastX = e.clientX;
      lastY = e.clientY;
      lastT = now;
    });
    function end(e) {
      if (!phys.dragging) return;
      phys.dragging = false;
      root.classList.remove('dragging');
      try { root.releasePointerCapture(e.pointerId); } catch (err) { /* already released */ }
    }
    root.addEventListener('pointerup', end);
    root.addEventListener('pointercancel', end);
  }

  // The bubble is built once and toggled with `hidden`, never torn down —
  // so the last reply is still there if you close it and click again,
  // instead of starting blank every time.
  function wireChat(root) {
    var bubble = document.createElement('div');
    bubble.className = 'companion-bubble';
    bubble.hidden = true;
    bubble.innerHTML =
      '<div class="companion-reply"></div>' +
      '<textarea rows="2" maxlength="500" placeholder="Say something..."></textarea>';
    root.appendChild(bubble);
    el.bubble = bubble;
    var textarea = bubble.querySelector('textarea');
    var replyEl = bubble.querySelector('.companion-reply');

    // Clicks/drags inside the bubble must not reach root's own click
    // (which toggles the bubble) or pointerdown (which starts a drag).
    bubble.addEventListener('click', function (e) { e.stopPropagation(); });
    bubble.addEventListener('pointerdown', function (e) { e.stopPropagation(); });
    textarea.addEventListener('keydown', function (e) {
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        send();
      }
    });

    function send() {
      var message = textarea.value.trim();
      if (!message) return;
      textarea.disabled = true;
      replyEl.textContent = '...';
      api('/api/companion/chat', { method: 'POST', body: JSON.stringify({ message: message, page: location.pathname }) })
        .then(function (res) {
          textarea.disabled = false;
          textarea.value = '';
          if (!res.ok) {
            replyEl.textContent = res.data && res.data.error ? res.data.error : 'Something went wrong.';
            return;
          }
          replyEl.textContent = res.data.reply;
          setMood(res.data.mood);
        });
    }

    root.addEventListener('click', function () {
      if (phys.dragging) return;
      bubble.hidden = !bubble.hidden;
      if (!bubble.hidden) textarea.focus();
    });
    document.addEventListener(
      'click',
      function (e) {
        if (bubble.hidden || root.contains(e.target)) return;
        bubble.hidden = true;
      },
      true
    );
  }

  function startPhysics() {
    var FRICTION = 0.92;
    var GRAVITY = 0.6;
    var BOUNCE = 0.35;
    var lastWander = 0;

    function tick(now) {
      if (!el.root) return;
      var w = el.root.offsetWidth || 96;
      var h = el.root.offsetHeight || 96;
      var floor = window.innerHeight - h;

      if (!phys.dragging) {
        phys.vy += GRAVITY;
        if (now - lastWander > 4000 + Math.random() * 4000) {
          lastWander = now;
          phys.vx += (Math.random() - 0.5) * 6;
        }
        phys.x += phys.vx;
        phys.y += phys.vy;
        phys.vx *= FRICTION;

        if (phys.y > floor) {
          phys.y = floor;
          if (Math.abs(phys.vy) > 1) phys.vy = -phys.vy * BOUNCE;
          else phys.vy = 0;
        }
        if (phys.y < 0) { phys.y = 0; phys.vy = 0; }
        if (phys.x < 0) { phys.x = 0; phys.vx = -phys.vx * BOUNCE; }
        if (phys.x > window.innerWidth - w) { phys.x = window.innerWidth - w; phys.vx = -phys.vx * BOUNCE; }
      }

      el.root.style.transform = 'translate(' + phys.x + 'px,' + phys.y + 'px)';
      rafId = requestAnimationFrame(tick);
    }
    rafId = requestAnimationFrame(tick);
  }

  function init() {
    api('/api/companion/settings').then(function (res) {
      if (!res.ok) return; // not logged in, or companion not enabled here
      state.enabled = !!res.data.enabled;
      state.activeCharacterId = res.data.activeCharacterId || '';
      if (!state.enabled || !state.activeCharacterId) return;
      api('/api/companion/characters').then(function (res2) {
        if (!res2.ok) return;
        state.characters = res2.data.characters || [];
        renderCompanion();
      });
    });
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else {
    init();
  }
})();
