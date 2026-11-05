// Catch the Turkey: a small homepage game shown during the Thanksgiving
// season (site-theme.js's holidaySeason(), or ?holiday=thanksgiving).
// Turkeys fall; move the basket to catch them. Golden turkeys are worth 5,
// burnt ones cost a life if caught, and every turkey that hits the ground
// costs a life. Three lives.
//
// Companions: if the visitor's companion is active (companion-widget.js
// exposes window.__mitchCompanion), it joins as a second catcher and
// comments on the result through its normal chat. A companion with the
// Sandevistan (David Martinez) can slow every turkey for a few seconds; that
// use goes through the server like any other and builds his strain, so in
// cyberpsychosis he's erratic and grabs burnt turkeys too. Chrome arms make
// him faster with a longer reach, the cyberskeleton doubles the slow-down.
//
// No coins: the score only lives in localStorage ('turkey_best').
(function () {
  'use strict';
  var section = document.getElementById('turkey-game');
  if (!section) return;
  var theme = window.__siteTheme;
  var season = theme && theme.holidaySeason ? theme.holidaySeason() : '';
  if (season !== 'thanksgiving' && document.documentElement.getAttribute('data-holiday') !== 'thanksgiving') return;
  section.hidden = false;

  var canvas = document.getElementById('turkey-canvas');
  var ctx = canvas.getContext('2d');
  var overlay = document.getElementById('turkey-overlay');
  var msg = document.getElementById('turkey-msg');
  var startBtn = document.getElementById('turkey-start');
  var scoreEl = document.getElementById('turkey-score');
  var livesEl = document.getElementById('turkey-lives');
  var bestEl = document.getElementById('turkey-best');
  var helperRow = document.getElementById('turkey-helper');
  var helperImg = document.getElementById('turkey-helper-img');
  var helperText = document.getElementById('turkey-helper-text');
  var sandeBtn = document.getElementById('turkey-sande');

  var W = 0, H = 0, dpr = 1;
  var BASKET_W = 96, BASKET_H = 26, TURKEY_R = 21;
  var running = false, paused = false, raf = 0, last = 0;
  var score = 0, lives = 3, best = 0, spawnIn = 0, helperCatches = 0;
  var turkeys = [];
  var basket = { x: 0, target: 0 };
  var keys = { left: false, right: false };
  var slowUntil = 0;
  var helper = null; // { x, img, speed, reach }
  var companion = null;

  try { best = Number(localStorage.getItem('turkey_best')) || 0; } catch (_) {}
  function paintBest() { bestEl.textContent = best ? 'Best ' + best : ''; }
  paintBest();

  function cssVar(name, fallback) {
    var v = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
    return v || fallback;
  }

  function resize() {
    var rect = canvas.parentElement.getBoundingClientRect();
    dpr = window.devicePixelRatio || 1;
    W = Math.max(280, Math.round(rect.width));
    H = W < 520 ? 320 : 380;
    canvas.width = W * dpr;
    canvas.height = H * dpr;
    canvas.style.height = H + 'px';
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    basket.x = Math.min(Math.max(basket.x || W / 2, BASKET_W / 2), W - BASKET_W / 2);
    basket.target = basket.x;
    if (!running) draw(0);
  }

  // ---- companion ----------------------------------------------------------

  function setupCompanion() {
    companion = window.__mitchCompanion || null;
    if (!companion) { helperRow.hidden = true; return; }
    helperRow.hidden = false;
    helperImg.src = companion.imageUrl('idle');
    helperImg.alt = '';
    var img = new Image();
    img.src = companion.imageUrl('idle');
    helper = { x: W * 0.25, img: img };
    sandeBtn.hidden = companion.specialAbility !== 'sandevistan';
    paintHelper();
  }

  function paintHelper() {
    if (!companion) return;
    var name = companion.name;
    if (companion.inPsychosis && companion.inPsychosis()) {
      helperText.textContent = name + ' is in cyberpsychosis and grabs anything, burnt turkeys too.';
    } else {
      helperText.textContent = name + ' catches turkeys with you.';
    }
    if (!sandeBtn.hidden) {
      var strain = Math.round(companion.strain ? companion.strain() : 0);
      sandeBtn.textContent = 'Sandevistan (S)' + (strain ? ', strain ' + strain : '');
    }
  }

  window.addEventListener('mitch:companion-ready', setupCompanion);
  if (window.__mitchCompanion) setupCompanion();

  function helperSpeed() {
    var base = companion && companion.hasUpgrade && companion.hasUpgrade('chrome_arms') ? 430 : 270;
    return Date.now() < slowUntil ? base * 2.5 : base;
  }
  function helperReach() {
    return companion && companion.hasUpgrade && companion.hasUpgrade('chrome_arms') ? 46 : 30;
  }

  function useSandevistan() {
    if (!companion || companion.specialAbility !== 'sandevistan' || !running) return;
    sandeBtn.disabled = true;
    companion.sandevistan().then(function (s) {
      sandeBtn.disabled = false;
      if (!s) return; // cooldown, or the server refused
      var longer = s.upgrades && s.upgrades.indexOf('cyberskeleton') !== -1;
      slowUntil = Date.now() + (longer ? 6000 : 3000);
      paintHelper();
    });
  }
  sandeBtn.addEventListener('click', useSandevistan);

  // ---- game ---------------------------------------------------------------

  function reset() {
    score = 0; lives = 3; helperCatches = 0; turkeys = []; spawnIn = 0.4; slowUntil = 0;
    basket.x = basket.target = W / 2;
    if (helper) helper.x = W * 0.25;
    paintHud();
  }

  function paintHud() {
    scoreEl.textContent = String(score);
    livesEl.textContent = 'Lives ' + Math.max(0, lives);
  }

  function spawn() {
    var r = Math.random();
    var kind = r < 0.07 ? 'golden' : (score >= 8 && r < 0.17 ? 'burnt' : 'normal');
    turkeys.push({
      x: TURKEY_R + Math.random() * (W - TURKEY_R * 2),
      y: -TURKEY_R,
      vy: 110 + Math.min(score, 60) * 4.5 + Math.random() * 40,
      spin: (Math.random() - 0.5) * 2.4,
      angle: Math.random() * 0.6 - 0.3,
      kind: kind,
    });
  }

  function catchTurkey(t, byHelper) {
    if (t.kind === 'burnt') {
      lives -= 1;
    } else {
      score += t.kind === 'golden' ? 5 : 1;
      if (byHelper) {
        helperCatches += 1;
        if (t.kind === 'golden' && companion) companion.act('jump');
      }
    }
    paintHud();
  }

  function update(dt) {
    var slow = Date.now() < slowUntil ? 0.25 : 1;
    // Basket: pointer target or keys.
    if (keys.left) basket.target -= 520 * dt;
    if (keys.right) basket.target += 520 * dt;
    basket.target = Math.min(Math.max(basket.target, BASKET_W / 2), W - BASKET_W / 2);
    basket.x += (basket.target - basket.x) * Math.min(1, dt * 18);

    spawnIn -= dt * slow;
    if (spawnIn <= 0) {
      spawn();
      spawnIn = Math.max(0.42, 1.1 - score * 0.012) * (0.75 + Math.random() * 0.5);
    }

    // Helper: heads for the lowest catchable turkey away from the basket.
    if (helper && companion) {
      var psych = companion.inPsychosis && companion.inPsychosis();
      var target = null;
      turkeys.forEach(function (t) {
        if (!psych && t.kind === 'burnt') return;
        if (Math.abs(t.x - basket.x) < BASKET_W * 0.6) return;
        if (!target || t.y > target.y) target = t;
      });
      var goal = target ? target.x : (psych ? helper.x + (Math.random() - 0.5) * 400 : W * 0.25);
      var step = helperSpeed() * dt * (psych ? 0.7 + Math.random() : 1);
      helper.x += Math.max(-step, Math.min(step, goal - helper.x));
      helper.x = Math.min(Math.max(helper.x, 24), W - 24);
    }

    var groundY = H - 14;
    var basketTop = groundY - BASKET_H;
    for (var i = turkeys.length - 1; i >= 0; i--) {
      var t = turkeys[i];
      t.y += t.vy * dt * slow;
      t.angle += t.spin * dt * slow;
      var caught = false;
      if (t.y + TURKEY_R >= basketTop && t.y < groundY && Math.abs(t.x - basket.x) < BASKET_W / 2 + TURKEY_R * 0.4) {
        catchTurkey(t, false); caught = true;
      } else if (helper && companion && t.y + TURKEY_R >= groundY - 40 && Math.abs(t.x - helper.x) < helperReach() &&
        (t.kind !== 'burnt' || (companion.inPsychosis && companion.inPsychosis()))) {
        catchTurkey(t, true); caught = true;
      } else if (t.y - TURKEY_R > groundY) {
        if (t.kind !== 'burnt') { lives -= 1; paintHud(); }
        caught = true;
      }
      if (caught) turkeys.splice(i, 1);
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

  function drawBasket(x, groundY) {
    var top = groundY - BASKET_H;
    ctx.fillStyle = '#8b5a2b';
    ctx.beginPath();
    ctx.moveTo(x - BASKET_W / 2, top);
    ctx.lineTo(x + BASKET_W / 2, top);
    ctx.lineTo(x + BASKET_W / 2 - 10, groundY);
    ctx.lineTo(x - BASKET_W / 2 + 10, groundY);
    ctx.closePath();
    ctx.fill();
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
    var bg = cssVar('--t-bg2', '#222');
    var line = cssVar('--t-bd', '#444');
    ctx.fillStyle = bg;
    ctx.fillRect(0, 0, W, H);
    var groundY = H - 14;
    ctx.fillStyle = line;
    ctx.fillRect(0, groundY, W, 2);
    if (Date.now() < slowUntil) {
      ctx.fillStyle = 'rgba(40, 220, 160, 0.10)';
      ctx.fillRect(0, 0, W, H);
    }
    turkeys.forEach(drawTurkey);
    if (helper && companion) {
      var size = 54;
      var psych = companion.inPsychosis && companion.inPsychosis();
      if (psych) { ctx.save(); ctx.shadowColor = 'rgba(255,30,60,.9)'; ctx.shadowBlur = 14; }
      if (helper.img.complete && helper.img.naturalWidth) {
        ctx.drawImage(helper.img, helper.x - size / 2, groundY - size, size, size);
      } else {
        ctx.fillStyle = cssVar('--t-ac', '#f57');
        ctx.beginPath(); ctx.arc(helper.x, groundY - size / 2, size / 2.4, 0, Math.PI * 2); ctx.fill();
      }
      if (psych) ctx.restore();
    }
    drawBasket(basket.x, groundY);
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
    resize();
    reset();
    if (companion) paintHelper();
    overlay.hidden = true;
    running = true;
    last = performance.now();
    raf = requestAnimationFrame(frame);
    canvas.focus();
  }

  function end() {
    running = false;
    cancelAnimationFrame(raf);
    draw();
    var newBest = score > best;
    if (newBest) {
      best = score;
      try { localStorage.setItem('turkey_best', String(best)); } catch (_) {}
      paintBest();
    }
    msg.textContent = 'Score ' + score + (newBest ? '. New best.' : '. Best ' + best + '.') +
      (companion && helperCatches ? ' ' + companion.name + ' caught ' + helperCatches + '.' : '');
    startBtn.textContent = 'Play again';
    overlay.hidden = false;
    if (companion && score > 0) {
      companion.react('(The person just finished a round of Catch the Turkey on the homepage: they scored ' + score +
        ', their best is ' + best + ', and you caught ' + helperCatches + ' of those turkeys for them. React briefly.)');
      paintHelper();
    }
  }

  startBtn.addEventListener('click', function () {
    if (!paused) { start(); return; }
    paused = false;
    overlay.hidden = true;
    running = true;
    last = performance.now();
    raf = requestAnimationFrame(frame);
    canvas.focus();
  });

  // Pointer anywhere on the stage steers the basket.
  canvas.addEventListener('pointermove', function (e) {
    var rect = canvas.getBoundingClientRect();
    basket.target = e.clientX - rect.left;
  });
  canvas.addEventListener('pointerdown', function (e) {
    var rect = canvas.getBoundingClientRect();
    basket.target = e.clientX - rect.left;
  });
  canvas.addEventListener('keydown', function (e) {
    if (e.key === 'ArrowLeft' || e.key === 'a' || e.key === 'A') { keys.left = true; e.preventDefault(); }
    if (e.key === 'ArrowRight' || e.key === 'd' || e.key === 'D') { keys.right = true; e.preventDefault(); }
    if (e.key === 's' || e.key === 'S') { useSandevistan(); e.preventDefault(); }
  });
  canvas.addEventListener('keyup', function (e) {
    if (e.key === 'ArrowLeft' || e.key === 'a' || e.key === 'A') keys.left = false;
    if (e.key === 'ArrowRight' || e.key === 'd' || e.key === 'D') keys.right = false;
  });
  document.addEventListener('visibilitychange', function () {
    if (document.hidden && running) {
      running = false;
      paused = true;
      cancelAnimationFrame(raf);
      msg.textContent = 'Paused.';
      startBtn.textContent = 'Resume';
      overlay.hidden = false;
    }
  });

  window.addEventListener('resize', resize);
  resize();
})();
