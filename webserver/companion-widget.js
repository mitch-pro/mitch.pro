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

  // Fixed preset reactions — must match ACTIONS in
  // rust/crates/mitch-server/src/routes/companion/deepseek.rs. Each one is
  // a pre-written CSS animation (see companion-widget.css); there is no
  // path from a character's `action` field to arbitrary code.
  var ACTIONS = ['spin', 'jump', 'shake', 'grow', 'shrink', 'wave', 'dance', 'peekaboo', 'confetti', 'nap'];

  // How hard a release has to be (px/frame, matching phys.vx/vy's units)
  // before it counts as a "throw" worth reacting to, and how long to wait
  // before the character can react to another one — otherwise a little
  // nudge on every drag would spam the chat endpoint.
  var THROW_SPEED_THRESHOLD = 28;
  var THROW_COOLDOWN_MS = 8000;
  var lastThrowReactionAt = 0;

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
    sprite.onerror = function () {
      if (!this.dataset.fallback) {
        this.dataset.fallback = '1';
        this.src = 'data:image/svg+xml;utf8,<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 120 120"><circle cx="60" cy="60" r="54" fill="%233b82f6"/><ellipse cx="60" cy="72" rx="34" ry="24" fill="%23ffffff" opacity="0.9"/><circle cx="44" cy="52" r="7" fill="%231e293b"/><circle cx="76" cy="52" r="7" fill="%231e293b"/><path d="M 49 66 Q 60 76 71 66" fill="none" stroke="%231e293b" stroke-width="4" stroke-linecap="round"/></svg>';
      }
    };
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

  function spawnConfetti(root) {
    var colors = ['#ff5c5c', '#ffd166', '#06d6a0', '#4cc9f0', '#c77dff'];
    for (var i = 0; i < 16; i++) {
      var piece = document.createElement('div');
      piece.className = 'companion-confetti-piece';
      piece.style.background = colors[i % colors.length];
      piece.style.setProperty('--cx', (Math.random() * 120 - 60) + 'px');
      piece.style.setProperty('--cr', (Math.random() * 360) + 'deg');
      piece.style.animationDelay = (Math.random() * 0.15) + 's';
      root.appendChild(piece);
      (function (p) { setTimeout(function () { p.remove(); }, 1200); })(piece);
    }
  }

  // Dispatches one of the fixed `ACTIONS` names to its pre-written
  // animation. Never touches anything but a CSS class (plus, for
  // confetti, a handful of inert decorative divs) — this is the "safe
  // preset" path, kept entirely separate from `runCustomJs`.
  //
  // Applied to el.sprite, NOT el.root: el.root's own `transform` is
  // rewritten every animation frame by startPhysics() for drag/gravity
  // positioning (translate(x,y)), so an animation targeting that same
  // element's `transform` would fight it and the companion would
  // visually snap to the position implied by the animation's transform
  // alone (e.g. `translateY(-28px)` with no x/y, near the viewport's
  // top-left) for as long as it runs. The sprite has no positioning
  // transform of its own, so animating it there is conflict-free.
  function playAction(name) {
    if (!el.sprite || ACTIONS.indexOf(name) === -1) return;
    var cls = 'action-' + name;
    el.sprite.classList.remove.apply(el.sprite.classList, ACTIONS.map(function (a) { return 'action-' + a; }));
    // Force a reflow so re-triggering the same action restarts its animation.
    void el.sprite.offsetWidth;
    el.sprite.classList.add(cls);
    if (name === 'confetti') spawnConfetti(el.root);
    el.sprite.addEventListener('animationend', function handler() {
      el.sprite.classList.remove(cls);
      el.sprite.removeEventListener('animationend', handler);
    });
  }

  // Auto-repairs common LLM JavaScript syntax errors:
  // - Strips markdown code blocks (```javascript ... ```)
  // - Strips stray unbalanced trailing braces (fixes "Unexpected token '}'")
  // - Closes unclosed parentheses (fixes "missing ) after argument list")
  // - Wraps bare anonymous function statements (function() { ... }) as IIFEs
  function autoRepairJs(code) {
    if (typeof code !== 'string') return '';
    var str = code.trim();
    if (str.startsWith('```')) {
      var nl = str.indexOf('\n');
      if (nl !== -1) str = str.slice(nl + 1);
      var end = str.lastIndexOf('```');
      if (end !== -1) str = str.slice(0, end);
      str = str.trim();
    }
    // If anonymous function expression, wrap as IIFE
    if (/^function\s*\(/.test(str)) {
      str = '(' + str + ')()';
    }
    // Strip trailing stray closing braces if unbalanced
    var opens = (str.match(/{/g) || []).length;
    var closes = (str.match(/}/g) || []).length;
    while (closes > opens && str.endsWith('}')) {
      str = str.slice(0, -1).trim();
      closes--;
    }
    // Balance missing closing parentheses
    var openParens = (str.match(/\(/g) || []).length;
    var closeParens = (str.match(/\)/g) || []).length;
    while (openParens > closeParens) {
      str = str + ')';
      closeParens++;
    }
    return str;
  }

  // Runs raw JS the server sent back. The server only ever populates this
  // field at all when the active character has been admin-reviewed (see
  // `deepseek.rs`'s `Capabilities` gate) — an unreviewed custom character
  // never gets this far, including for its own creator testing it. Still
  // wrapped in try/catch: a reviewed character's code should never be
  // able to take down the rest of the page just because it threw.
  function runCustomJs(code) {
    if (!code) return;
    var repaired = autoRepairJs(code);
    if (!repaired) return;
    try {
      new Function(repaired)();
    } catch (err) {
      if (err instanceof SyntaxError) {
        try {
          var stripped = repaired.replace(/[}\s;]+$/, '');
          new Function(stripped)();
          return;
        } catch (_) {}
      }
      console.error('[companion] customJs threw:', err);
    }
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

      var speed = Math.sqrt(phys.vx * phys.vx + phys.vy * phys.vy);
      var now = performance.now();
      if (speed >= THROW_SPEED_THRESHOLD && now - lastThrowReactionAt >= THROW_COOLDOWN_MS) {
        lastThrowReactionAt = now;
        sendToCompanion('(you were just thrown/flung across the screen by the user)', { silent: true, showBubbleOnReply: true });
      }
    }
    root.addEventListener('pointerup', end);
    root.addEventListener('pointercancel', end);
  }

  // Shared by the textarea's send button and the throw reaction — one
  // path for "ask the character something and act on what comes back."
  // `opts.silent` skips echoing the outgoing message anywhere (the throw
  // reaction isn't something the user "said"); `opts.showBubbleOnReply`
  // opens the bubble to show the reaction even if it was closed.
  function sendToCompanion(message, opts) {
    opts = opts || {};
    return api('/api/companion/chat', { method: 'POST', body: JSON.stringify({ message: message, page: location.pathname }) })
      .then(function (res) {
        if (!res.ok) {
          if (!opts.silent && el.replyEl) {
            el.replyEl.textContent = res.data && res.data.error ? res.data.error : 'Something went wrong.';
          }
          return;
        }
        if (opts.showBubbleOnReply && el.bubble) el.bubble.hidden = false;
        if (el.replyEl) el.replyEl.textContent = res.data.reply;
        setMood(res.data.mood);
        if (res.data.action) playAction(res.data.action);
        if (res.data.customJs) runCustomJs(res.data.customJs);
        if (res.data.coinsTaken > 0 || res.data.coinRequestPercent > 0) {
          showCoinTaken(res.data.coinsTaken || 0, res.data.coinRequestPercent || 0);
        } else if (el.coinRequestEl) {
          el.coinRequestEl.hidden = true;
        }
      });
  }

  // Notifies that the character took a percentage of MitchCoins — no
  // confirmation prompt, money is deducted directly by the server.
  function showCoinTaken(amount, percent) {
    if (!el.coinRequestEl) return;
    var pctText = Math.round(percent * 10) / 10;
    var amtText = Math.round(amount * 100) / 100;
    el.coinRequestEl.innerHTML =
      '<div>Took ' + pctText + '% of your MitchCoins (' + amtText + ' coins).</div>';
    el.coinRequestEl.hidden = false;
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
      '<div class="companion-coin-request" hidden></div>' +
      '<textarea rows="2" maxlength="500" placeholder="Say something..."></textarea>';
    root.appendChild(bubble);
    el.bubble = bubble;
    var textarea = bubble.querySelector('textarea');
    var replyEl = bubble.querySelector('.companion-reply');
    el.replyEl = replyEl;
    el.coinRequestEl = bubble.querySelector('.companion-coin-request');

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
      el.coinRequestEl.hidden = true;
      sendToCompanion(message, {}).then(function () {
        textarea.disabled = false;
        textarea.value = '';
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
