// Falling leaves (Thanksgiving), snow (Christmas), rising embers (Halloween), confetti (New Year's) or
// hearts (Valentine's) over the page. Loaded by
// site-theme.js only while a holiday theme is active. It draws on a fixed canvas
// that ignores the pointer, stops when the tab is hidden, and does not run if
// the visitor prefers reduced motion, turned animations off in Preferences, or
// turned off just the holiday effects (theme_holidayfx = 'off'; the holiday
// colors stay). It never runs on the VM desktop or the games page (/game-portal,
// which /games redirects to), where it would draw over the content.
(function () {
  'use strict';
  if (window.__holidayFx) return;

  var canvas = null, ctx = null, frame = 0, last = 0, kind = '', particles = [], light = false;
  var LEAF_COLORS = ['#d9822b', '#b85414', '#c9a227', '#8c4a1c', '#a33b20'];
  var EMBER_COLORS = ['#ff8a1f', '#ffb347', '#ff5a1f', '#8e4bd6'];
  var CONFETTI_COLORS = ['#f5c451', '#ff5d8f', '#4aa3ff', '#6fe3a1', '#c58cff'];
  var HEART_COLORS = ['#ff5d8f', '#ff85ab', '#d6336c', '#ffb3c8'];
  var COUNTS = { christmas: 90, thanksgiving: 38, halloween: 46, newyear: 70, valentines: 34 };

  function allowed() {
    var off = false;
    try { off = localStorage.getItem('theme_motion') === 'off' || localStorage.getItem('theme_holidayfx') === 'off'; } catch (_) {}
    var reduced = window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    var path = location.pathname;
    var excluded = path.indexOf('/game-portal') === 0 || /^\/games\/?$/.test(path);
    return !off && !reduced && !excluded;
  }

  function isLightBackground() {
    var bg = getComputedStyle(document.documentElement).getPropertyValue('--t-bg').trim();
    var m = /^#?([0-9a-f]{6})$/i.exec(bg);
    if (!m) return false;
    var n = parseInt(m[1], 16);
    return (0.299 * (n >> 16) + 0.587 * ((n >> 8) & 255) + 0.114 * (n & 255)) > 150;
  }

  function resize() {
    if (!canvas) return;
    canvas.width = window.innerWidth;
    canvas.height = window.innerHeight;
    var target = Math.max(18, Math.min(COUNTS[kind] || 40, Math.round(canvas.width * canvas.height / 22000)));
    while (particles.length < target) particles.push(spawn(true));
    particles.length = Math.min(particles.length, target);
  }

  function pick(list) { return list[Math.floor(Math.random() * list.length)]; }

  function spawn(anywhere) {
    var w = canvas ? canvas.width : 1000, h = canvas ? canvas.height : 800;
    var rises = kind === 'halloween' || kind === 'valentines';
    var p = {
      x: Math.random() * w,
      y: anywhere ? Math.random() * h : (rises ? h + 12 : -12),
      r: 1 + Math.random() * 2.6,
      speed: 24 + Math.random() * 42,
      sway: 14 + Math.random() * 26,
      phase: Math.random() * Math.PI * 2,
      spin: (Math.random() - 0.5) * 2,
      angle: Math.random() * Math.PI * 2,
      color: ''
    };
    if (kind === 'thanksgiving') { p.r = 4 + Math.random() * 4; p.speed = 22 + Math.random() * 28; p.color = pick(LEAF_COLORS); }
    else if (kind === 'halloween') { p.r = 1.2 + Math.random() * 2.4; p.speed = 18 + Math.random() * 30; p.color = pick(EMBER_COLORS); }
    else if (kind === 'newyear') { p.r = 2.5 + Math.random() * 3; p.speed = 34 + Math.random() * 50; p.spin *= 4; p.color = pick(CONFETTI_COLORS); }
    else if (kind === 'valentines') { p.r = 5 + Math.random() * 6; p.speed = 20 + Math.random() * 26; p.color = pick(HEART_COLORS); }
    return p;
  }

  function heart(x, y, size, angle) {
    ctx.save();
    ctx.translate(x, y);
    ctx.rotate(angle * 0.15);
    ctx.beginPath();
    ctx.moveTo(0, size * 0.35);
    ctx.bezierCurveTo(-size, -size * 0.3, -size * 0.55, -size, 0, -size * 0.4);
    ctx.bezierCurveTo(size * 0.55, -size, size, -size * 0.3, 0, size * 0.35);
    ctx.fill();
    ctx.restore();
  }

  function tick(now) {
    frame = requestAnimationFrame(tick);
    var dt = Math.min(0.05, (now - last) / 1000);
    last = now;
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    var rises = kind === 'halloween' || kind === 'valentines';
    ctx.globalAlpha = kind === 'christmas' ? (light ? 0.75 : 0.85) : 0.8;
    for (var i = 0; i < particles.length; i++) {
      var p = particles[i];
      p.y += (rises ? -p.speed : p.speed) * dt;
      p.phase += dt * 1.1;
      p.angle += p.spin * dt;
      var x = p.x + Math.sin(p.phase) * p.sway;
      if (rises ? p.y < -14 : p.y > canvas.height + 14) { particles[i] = spawn(false); continue; }
      if (kind === 'halloween') ctx.globalAlpha = 0.45 + 0.4 * Math.abs(Math.sin(p.phase * 1.7));
      ctx.beginPath();
      if (kind === 'christmas') {
        ctx.fillStyle = light ? '#8fa6c4' : '#ffffff';
        ctx.arc(x, p.y, p.r, 0, Math.PI * 2);
        ctx.fill();
      } else if (kind === 'thanksgiving') {
        ctx.fillStyle = p.color;
        ctx.ellipse(x, p.y, p.r * 1.5, p.r * 0.85, p.angle, 0, Math.PI * 2);
        ctx.fill();
      } else if (kind === 'halloween') {
        ctx.fillStyle = p.color;
        ctx.arc(x, p.y, p.r, 0, Math.PI * 2);
        ctx.fill();
      } else if (kind === 'newyear') {
        ctx.fillStyle = p.color;
        ctx.save();
        ctx.translate(x, p.y);
        ctx.rotate(p.angle);
        ctx.fillRect(-p.r, -p.r * 0.5, p.r * 2, p.r);
        ctx.restore();
      } else {
        ctx.fillStyle = p.color;
        heart(x, p.y, p.r, Math.sin(p.phase));
      }
    }
    ctx.globalAlpha = 1;
  }

  function start() {
    if (canvas) return;
    canvas = document.createElement('canvas');
    canvas.setAttribute('aria-hidden', 'true');
    canvas.style.cssText = 'position:fixed;inset:0;width:100%;height:100%;pointer-events:none;z-index:2147483000';
    document.body.appendChild(canvas);
    ctx = canvas.getContext('2d');
    particles = [];
    resize();
    last = performance.now();
    frame = requestAnimationFrame(tick);
  }

  function stop() {
    cancelAnimationFrame(frame);
    if (canvas) canvas.remove();
    canvas = ctx = null;
    particles = [];
  }

  function refresh() {
    var next = document.documentElement.getAttribute('data-holiday') || '';
    if (!next || !allowed() || document.hidden) { kind = next; stop(); return; }
    if (next !== kind) stop();
    kind = next;
    light = isLightBackground();
    start();
  }

  window.addEventListener('resize', resize);
  document.addEventListener('visibilitychange', refresh);
  window.__holidayFx = { refresh: refresh };
  if (document.body) refresh(); else document.addEventListener('DOMContentLoaded', refresh, { once: true });
})();
