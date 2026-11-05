// Catch the Turkey: a small homepage game for the Thanksgiving season
// (site-theme.js's holidaySeason(), or ?holiday=thanksgiving). A
// "Catch the turkey" pill next to the holiday pills starts it; turkeys then
// fall over the whole page and a basket follows the mouse, finger or arrow
// keys along the bottom of the screen. The canvas ignores the pointer, so
// the page stays usable; only the small HUD takes clicks. Esc quits.
//
// Golden turkeys are worth 5, burnt ones cost a life if caught, and every
// other turkey that reaches the bottom costs a life. Three lives.
//
// Companions: if the visitor's companion is active (companion-widget.js
// exposes window.__mitchCompanion), it helps from the bottom of the screen
// and comments on the result through its normal chat. It's deliberately a
// weak helper: slow, short reach, sometimes fumbles, and it tires with every
// catch in a round. Sandevistan strain (David Martinez) weakens it further,
// and his Sandevistan (S) slows the turkeys at the cost of more strain, and
// for a few grabs he flies anywhere on screen at full speed to snatch them
// (fewer grabs the weaker he is; the cyberskeleton adds two). In
// cyberpsychosis he's erratic and grabs burnt turkeys too. Chrome arms give a
// little more speed and reach; the cyberskeleton doubles the slow-down.
//
// No coins: the best score only lives in localStorage ('turkey_best').
(function () {
  'use strict';
  var launch = document.getElementById('turkey-launch');
  if (!launch) return;
  var theme = window.__siteTheme;
  var season = theme && theme.holidaySeason ? theme.holidaySeason() : '';
  if (season !== 'thanksgiving' && document.documentElement.getAttribute('data-holiday') !== 'thanksgiving') return;
  launch.hidden = false;

  var BASKET_W = 92, BASKET_H = 24, TURKEY_R = 19, HELPER_SIZE = 48;
  // Sandevistan burst: flight speed (px/s) and grab radius.
  var BURST_SPEED = 1500, BURST_REACH = 30;
  var canvas = null, ctx = null, hud = null, card = null;
  var W = 0, H = 0;
  var running = false, raf = 0, last = 0;
  var score = 0, lives = 3, best = 0, spawnIn = 0, helperCatches = 0;
  var turkeys = [];
  var basket = { x: 0, target: 0 };
  var keys = { left: false, right: false };
  var slowUntil = 0;
  var companion = null;
  // Helper state for the round. `fatigue` starts at 1 and drops with each
  // catch; `cooldown` is the pause after a grab before it moves again.
  var helper = null;

  try { best = Number(localStorage.getItem('turkey_best')) || 0; } catch (_) {}

  function el(tag, cls, text) {
    var n = document.createElement(tag);
    if (cls) n.className = cls;
    if (text) n.textContent = text;
    return n;
  }

  function build() {
    canvas = el('canvas', 'turkey-canvas');
    canvas.setAttribute('aria-hidden', 'true');
    ctx = canvas.getContext('2d');
    hud = el('div', 'turkey-hud');
    hud.setAttribute('role', 'group');
    hud.setAttribute('aria-label', 'Catch the turkey');
    hud.innerHTML = '<span class="turkey-score" aria-live="polite"></span><span class="turkey-lives"></span>' +
      '<button type="button" class="turkey-sande" hidden>Sandevistan (S)</button>' +
      '<button type="button" class="turkey-quit" aria-label="Quit (Esc)">Quit</button>';
    card = el('div', 'turkey-card');
    card.setAttribute('role', 'dialog');
    card.setAttribute('aria-label', 'Round over');
    card.innerHTML = '<p class="turkey-msg"></p><div><button type="button" class="turkey-again">Play again</button><button type="button" class="turkey-close">Close</button></div>';
    card.hidden = true;
    document.body.appendChild(canvas);
    document.body.appendChild(hud);
    document.body.appendChild(card);
    hud.querySelector('.turkey-quit').addEventListener('click', close);
    hud.querySelector('.turkey-sande').addEventListener('click', useSandevistan);
    card.querySelector('.turkey-again').addEventListener('click', start);
    card.querySelector('.turkey-close').addEventListener('click', close);
    window.addEventListener('resize', resize);
    resize();
  }

  function resize() {
    if (!canvas) return;
    var dpr = window.devicePixelRatio || 1;
    W = window.innerWidth;
    H = window.innerHeight;
    canvas.width = W * dpr;
    canvas.height = H * dpr;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    basket.x = Math.min(Math.max(basket.x || W / 2, BASKET_W / 2), W - BASKET_W / 2);
  }

  // The bottom edge the basket rides on, above the fixed footer band and
  // the mobile nav dock.
  function groundY() {
    var bottom = H;
    var band = document.getElementById('site-credit');
    var dock = document.querySelector('.mobile-nav-dock, #mobile-dock');
    if (band) bottom = Math.min(bottom, band.getBoundingClientRect().top);
    // offsetParent is null for fixed elements, so check for a layout box instead.
    if (dock && dock.getClientRects().length) bottom = Math.min(bottom, dock.getBoundingClientRect().top);
    return bottom - 10;
  }

  // ---- companion ----------------------------------------------------------

  function setupCompanion() {
    companion = window.__mitchCompanion || null;
    if (!hud) return;
    var sande = hud.querySelector('.turkey-sande');
    sande.hidden = !companion || companion.specialAbility !== 'sandevistan';
    if (companion) {
      var img = new Image();
      img.src = companion.imageUrl('idle');
      // y is the sprite's centre; it only leaves the floor during a Sandevistan burst.
      helper = { x: W * 0.2, y: groundY() - HELPER_SIZE / 2, img: img, fatigue: 1, cooldown: 0, aim: null, burstLeft: 0, trail: [] };
    } else {
      helper = null;
    }
  }
  window.addEventListener('mitch:companion-ready', setupCompanion);

  function hasArms() { return !!(companion && companion.hasUpgrade && companion.hasUpgrade('chrome_arms')); }
  function psycho() { return !!(companion && companion.inPsychosis && companion.inPsychosis()); }

  // 1 when fresh. Sandevistan strain takes up to 70% off, and this round's
  // fatigue compounds on top.
  function helperPower() {
    if (!helper) return 0;
    var strain = companion && companion.strain ? companion.strain() : 0;
    return Math.max(0.15, (1 - Math.min(strain, 100) / 100 * 0.7) * helper.fatigue);
  }

  function useSandevistan() {
    if (!companion || companion.specialAbility !== 'sandevistan' || !running) return;
    var btn = hud.querySelector('.turkey-sande');
    btn.disabled = true;
    companion.sandevistan().then(function (s) {
      btn.disabled = false;
      if (!s) return;
      var longer = s.upgrades && s.upgrades.indexOf('cyberskeleton') !== -1;
      slowUntil = Date.now() + (longer ? 6000 : 3000);
      // He flies for a few grabs; fewer the more tired or strained he is.
      if (helper) helper.burstLeft = Math.max(1, Math.round(4 * helperPower())) + (longer ? 2 : 0);
      paintHud();
    });
  }

  // ---- game ---------------------------------------------------------------

  function paintHud() {
    if (!hud) return;
    hud.querySelector('.turkey-score').textContent = 'Score ' + score;
    hud.querySelector('.turkey-lives').textContent = 'Lives ' + Math.max(0, lives);
    var sande = hud.querySelector('.turkey-sande');
    if (!sande.hidden && companion) {
      var strain = Math.round(companion.strain ? companion.strain() : 0);
      sande.textContent = 'Sandevistan (S)' + (strain ? ' · strain ' + strain : '');
    }
  }

  function spawn() {
    var r = Math.random();
    var kind = r < 0.07 ? 'golden' : (score >= 8 && r < 0.17 ? 'burnt' : 'normal');
    turkeys.push({
      x: TURKEY_R + Math.random() * (W - TURKEY_R * 2),
      y: -TURKEY_R,
      vy: 120 + Math.min(score, 60) * 5 + Math.random() * 50,
      spin: (Math.random() - 0.5) * 2.4,
      angle: Math.random() * 0.6 - 0.3,
      kind: kind,
      fumbled: false,
    });
  }

  function catchTurkey(t, byHelper) {
    if (t.kind === 'burnt') {
      lives -= 1;
    } else {
      score += t.kind === 'golden' ? 5 : 1;
      if (byHelper) {
        helperCatches += 1;
        helper.fatigue = Math.max(0.2, helper.fatigue * 0.85);
        if (t.kind === 'golden' && companion) companion.act('jump');
      }
    }
    paintHud();
  }

  function update(dt) {
    var slow = Date.now() < slowUntil ? 0.3 : 1;
    if (keys.left) basket.target -= 600 * dt;
    if (keys.right) basket.target += 600 * dt;
    basket.target = Math.min(Math.max(basket.target, BASKET_W / 2), W - BASKET_W / 2);
    basket.x += (basket.target - basket.x) * Math.min(1, dt * 18);

    spawnIn -= dt * slow;
    if (spawnIn <= 0) {
      spawn();
      spawnIn = Math.max(0.45, 1.15 - score * 0.012) * (0.75 + Math.random() * 0.5);
    }

    var floor = groundY();
    var basketTop = floor - BASKET_H;

    var bursting = !!(helper && companion && helper.burstLeft > 0 && Date.now() < slowUntil);
    var restY = floor - HELPER_SIZE / 2;
    if (helper && companion && bursting) {
      var crazyB = psycho();
      var near = null, nearD = Infinity;
      turkeys.forEach(function (t) {
        if (!crazyB && t.kind === 'burnt') return;
        var d = Math.hypot(t.x - helper.x, t.y - helper.y);
        if (d < nearD) { near = t; nearD = d; }
      });
      if (near) {
        var stepB = BURST_SPEED * dt;
        if (nearD <= stepB || nearD < BURST_REACH) {
          helper.x = near.x; helper.y = near.y;
          turkeys.splice(turkeys.indexOf(near), 1);
          catchTurkey(near, true);
          helper.burstLeft -= 1;
        } else {
          helper.x += (near.x - helper.x) / nearD * stepB;
          helper.y += (near.y - helper.y) / nearD * stepB;
        }
      }
      helper.trail.push({ x: helper.x, y: helper.y, t: Date.now() });
    } else if (helper && companion && helper.y < restY - 1) {
      // Drop back to the floor after a burst.
      helper.y = Math.min(restY, helper.y + 900 * dt);
      helper.trail.push({ x: helper.x, y: helper.y, t: Date.now() });
    } else if (helper) {
      helper.y = restY;
    }
    if (helper) helper.trail = helper.trail.filter(function (p) { return Date.now() - p.t < 260; });

    if (helper && companion && !bursting && helper.y >= restY - 1) {
      var power = helperPower();
      var crazy = psycho();
      helper.cooldown = Math.max(0, helper.cooldown - dt);
      // Re-pick a target only now and then: a slow reaction, not perfect tracking.
      if (!helper.aim || Math.random() < dt * 1.5) {
        helper.aim = null;
        turkeys.forEach(function (t) {
          if (t.fumbled || (!crazy && t.kind === 'burnt')) return;
          if (Math.abs(t.x - basket.x) < BASKET_W) return; // leaves the player's turkeys alone
          if (t.y < floor * 0.45) return;
          if (!helper.aim || t.y > helper.aim.y) helper.aim = t;
        });
      }
      if (turkeys.indexOf(helper.aim) === -1) helper.aim = null;
      var goal = helper.aim ? helper.aim.x : (crazy ? helper.x + (Math.random() - 0.5) * 300 : helper.x);
      var speed = (hasArms() ? 190 : 130) * power * (Date.now() < slowUntil ? 1.5 : 1) * (crazy ? 0.5 + Math.random() : 1);
      if (helper.cooldown <= 0) {
        var step = speed * dt;
        helper.x += Math.max(-step, Math.min(step, goal - helper.x));
      }
      helper.x = Math.min(Math.max(helper.x, 24), W - 24);
    }

    for (var i = turkeys.length - 1; i >= 0; i--) {
      var t = turkeys[i];
      t.y += t.vy * dt * slow;
      t.angle += t.spin * dt * slow;
      var gone = false;
      if (t.y + TURKEY_R >= basketTop && t.y < floor && Math.abs(t.x - basket.x) < BASKET_W / 2 + TURKEY_R * 0.4) {
        catchTurkey(t, false); gone = true;
      } else if (helper && companion && !bursting && helper.y >= restY - 1 && !t.fumbled && t.y + TURKEY_R >= floor - 34 &&
        Math.abs(t.x - helper.x) < (hasArms() ? 26 : 18) &&
        (t.kind !== 'burnt' || psycho())) {
        // A tired or strained helper fumbles more often.
        if (Math.random() < 0.25 + (1 - helperPower()) * 0.5) {
          t.fumbled = true;
          helper.cooldown = 0.6;
        } else {
          catchTurkey(t, true); gone = true;
          helper.cooldown = 0.8;
        }
      } else if (t.y - TURKEY_R > floor) {
        if (t.kind !== 'burnt') { lives -= 1; paintHud(); }
        gone = true;
      }
      if (gone) turkeys.splice(i, 1);
    }
    if (lives <= 0) end();
  }

  // ---- drawing ------------------------------------------------------------

  function drawTurkey(t) {
    ctx.save();
    ctx.translate(t.x, t.y);
    ctx.rotate(t.angle);
    var r = TURKEY_R;
    var feathers = t.kind === 'golden' ? ['#f7d154', '#f2b632', '#ffe58a', '#e8a91e', '#ffd84d']
      : t.kind === 'burnt' ? ['#2b2522', '#3a302b', '#1f1a17', '#332a25', '#28211d']
      : ['#c0392b', '#e67e22', '#f1c40f', '#d35400', '#e74c3c'];
    for (var i = 0; i < 5; i++) {
      var a = -Math.PI / 2 + (i - 2) * 0.42;
      ctx.fillStyle = feathers[i];
      ctx.beginPath();
      ctx.ellipse(Math.cos(a) * r * 0.95, Math.sin(a) * r * 0.95, r * 0.38, r * 0.75, a + Math.PI / 2, 0, Math.PI * 2);
      ctx.fill();
    }
    ctx.fillStyle = t.kind === 'golden' ? '#d9a21b' : t.kind === 'burnt' ? '#171310' : '#7a4a2a';
    ctx.beginPath();
    ctx.ellipse(0, r * 0.15, r * 0.78, r * 0.72, 0, 0, Math.PI * 2);
    ctx.fill();
    ctx.fillStyle = t.kind === 'burnt' ? '#241d19' : '#9b6236';
    ctx.beginPath();
    ctx.arc(0, -r * 0.45, r * 0.34, 0, Math.PI * 2);
    ctx.fill();
    ctx.fillStyle = '#f39c12';
    ctx.beginPath();
    ctx.moveTo(-r * 0.1, -r * 0.42); ctx.lineTo(r * 0.12, -r * 0.42); ctx.lineTo(0, -r * 0.22);
    ctx.fill();
    ctx.fillStyle = '#c0392b';
    ctx.beginPath();
    ctx.ellipse(r * 0.08, -r * 0.18, r * 0.07, r * 0.14, 0, 0, Math.PI * 2);
    ctx.fill();
    ctx.fillStyle = '#111';
    ctx.beginPath(); ctx.arc(-r * 0.12, -r * 0.52, r * 0.06, 0, Math.PI * 2); ctx.arc(r * 0.12, -r * 0.52, r * 0.06, 0, Math.PI * 2); ctx.fill();
    ctx.restore();
  }

  function drawBasket(x, floor) {
    var top = floor - BASKET_H;
    ctx.save();
    ctx.shadowColor = 'rgba(0,0,0,.3)';
    ctx.shadowBlur = 8;
    ctx.fillStyle = '#8b5a2b';
    ctx.beginPath();
    ctx.moveTo(x - BASKET_W / 2, top);
    ctx.lineTo(x + BASKET_W / 2, top);
    ctx.lineTo(x + BASKET_W / 2 - 10, floor);
    ctx.lineTo(x - BASKET_W / 2 + 10, floor);
    ctx.closePath();
    ctx.fill();
    ctx.restore();
    ctx.strokeStyle = 'rgba(0,0,0,.25)';
    ctx.lineWidth = 2;
    for (var i = 1; i < 4; i++) {
      ctx.beginPath();
      ctx.moveTo(x - BASKET_W / 2 + i * 4, top + i * 6);
      ctx.lineTo(x + BASKET_W / 2 - i * 4, top + i * 6);
      ctx.stroke();
    }
    ctx.fillStyle = '#a8703a';
    ctx.fillRect(x - BASKET_W / 2 - 3, top - 4, BASKET_W + 6, 6);
  }

  function draw() {
    ctx.clearRect(0, 0, W, H);
    var floor = groundY();
    if (Date.now() < slowUntil) {
      ctx.fillStyle = 'rgba(40, 220, 160, 0.08)';
      ctx.fillRect(0, 0, W, H);
    }
    turkeys.forEach(function (t) {
      if (t.fumbled) ctx.globalAlpha = 0.6;
      drawTurkey(t);
      ctx.globalAlpha = 1;
    });
    if (helper && companion) {
      var size = HELPER_SIZE;
      // Afterimages from a Sandevistan burst.
      helper.trail.forEach(function (p, i) {
        ctx.save();
        ctx.globalAlpha = 0.35 * (1 - (Date.now() - p.t) / 260);
        ctx.filter = 'hue-rotate(' + (i % 2 ? 150 : 300) + 'deg) saturate(3)';
        if (helper.img.complete && helper.img.naturalWidth) ctx.drawImage(helper.img, p.x - size / 2, p.y - size / 2, size, size);
        ctx.restore();
      });
      ctx.save();
      // Fades as it tires or strains.
      ctx.globalAlpha = 0.55 + 0.45 * helperPower();
      if (psycho()) { ctx.shadowColor = 'rgba(255,30,60,.9)'; ctx.shadowBlur = 14; }
      if (helper.img.complete && helper.img.naturalWidth) {
        ctx.drawImage(helper.img, helper.x - size / 2, helper.y - size / 2, size, size);
      }
      ctx.restore();
    }
    drawBasket(basket.x, floor);
  }

  function frame(now) {
    if (!running) return;
    var dt = Math.min(0.05, (now - last) / 1000);
    last = now;
    update(dt);
    draw();
    if (running) raf = requestAnimationFrame(frame);
  }

  function start() {
    if (!canvas) build();
    setupCompanion();
    score = 0; lives = 3; helperCatches = 0; turkeys = []; spawnIn = 0.5; slowUntil = 0;
    if (helper) { helper.burstLeft = 0; helper.trail = []; }
    basket.x = basket.target = W / 2;
    card.hidden = true;
    card.querySelector('.turkey-again').textContent = 'Play again';
    hud.hidden = false;
    canvas.hidden = false;
    document.body.classList.add('turkey-playing');
    paintHud();
    running = true;
    last = performance.now();
    cancelAnimationFrame(raf);
    raf = requestAnimationFrame(frame);
  }

  function stop() {
    running = false;
    cancelAnimationFrame(raf);
  }

  function close() {
    stop();
    if (!canvas) return;
    canvas.hidden = true;
    hud.hidden = true;
    card.hidden = true;
    document.body.classList.remove('turkey-playing');
    launch.focus();
  }

  function end() {
    stop();
    draw();
    var newBest = score > best;
    if (newBest) {
      best = score;
      try { localStorage.setItem('turkey_best', String(best)); } catch (_) {}
    }
    card.querySelector('.turkey-msg').textContent = 'Score ' + score + (newBest ? '. New best.' : '. Best ' + best + '.') +
      (companion && helperCatches ? ' ' + companion.name + ' caught ' + helperCatches + '.' : '');
    card.hidden = false;
    card.querySelector('.turkey-again').focus();
    if (companion && score > 0) {
      companion.react('(The person just finished a round of Catch the Turkey on the homepage: they scored ' + score +
        ', their best is ' + best + ', and you caught ' + helperCatches + ' of those turkeys for them. React briefly.)');
    }
  }

  launch.addEventListener('click', start);

  // The basket follows the pointer anywhere on the page while playing.
  window.addEventListener('pointermove', function (e) { if (running) basket.target = e.clientX; });
  window.addEventListener('pointerdown', function (e) { if (running) basket.target = e.clientX; });
  document.addEventListener('keydown', function (e) {
    if (!canvas || canvas.hidden) return;
    if (e.key === 'Escape') { close(); return; }
    var tag = (e.target && e.target.tagName) || '';
    if (!running || /^(INPUT|TEXTAREA|SELECT)$/.test(tag) || (e.target && e.target.isContentEditable)) return;
    if (e.key === 'ArrowLeft' || e.key === 'a' || e.key === 'A') { keys.left = true; e.preventDefault(); }
    if (e.key === 'ArrowRight' || e.key === 'd' || e.key === 'D') { keys.right = true; e.preventDefault(); }
    if (e.key === 's' || e.key === 'S') { useSandevistan(); e.preventDefault(); }
  });
  document.addEventListener('keyup', function (e) {
    if (e.key === 'ArrowLeft' || e.key === 'a' || e.key === 'A') keys.left = false;
    if (e.key === 'ArrowRight' || e.key === 'd' || e.key === 'D') keys.right = false;
  });
  document.addEventListener('visibilitychange', function () {
    if (document.hidden && running) {
      stop();
      card.querySelector('.turkey-msg').textContent = 'Paused at ' + score + '.';
      card.querySelector('.turkey-again').textContent = 'Start over';
      card.hidden = false;
    }
  });
})();
