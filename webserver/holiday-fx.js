// Falling leaves (Thanksgiving) or snow (Christmas) behind the page. Loaded by
// site-theme.js only while a holiday theme is active. It draws on a fixed canvas
// that ignores the pointer, stops when the tab is hidden, and does not run if
// the visitor prefers reduced motion or turned animations off in Preferences.
(function () {
  'use strict';
  if (window.__holidayFx) return;

  var canvas = null, ctx = null, frame = 0, last = 0, kind = '', particles = [], light = false;
  var LEAF_COLORS = ['#d9822b', '#b85414', '#c9a227', '#8c4a1c', '#a33b20'];

  function allowed() {
    var off = false;
    try { off = localStorage.getItem('theme_motion') === 'off'; } catch (_) {}
    var reduced = window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    return !off && !reduced && location.pathname.indexOf('/vms/desktop') !== 0;
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
    var target = Math.max(18, Math.min(kind === 'christmas' ? 90 : 38, Math.round(canvas.width * canvas.height / 22000)));
    while (particles.length < target) particles.push(spawn(true));
    particles.length = Math.min(particles.length, target);
  }

  function spawn(anywhere) {
    var leaf = kind === 'thanksgiving';
    return {
      x: Math.random() * (canvas ? canvas.width : 1000),
      y: anywhere ? Math.random() * (canvas ? canvas.height : 800) : -12,
      r: leaf ? 4 + Math.random() * 4 : 1 + Math.random() * 2.6,
      speed: leaf ? 22 + Math.random() * 28 : 24 + Math.random() * 42,
      sway: 14 + Math.random() * 26,
      phase: Math.random() * Math.PI * 2,
      spin: (Math.random() - 0.5) * 2,
      angle: Math.random() * Math.PI * 2,
      color: leaf ? LEAF_COLORS[Math.floor(Math.random() * LEAF_COLORS.length)] : ''
    };
  }

  function tick(now) {
    frame = requestAnimationFrame(tick);
    var dt = Math.min(0.05, (now - last) / 1000);
    last = now;
    ctx.clearRect(0, 0, canvas.width, canvas.height);
    ctx.globalAlpha = kind === 'christmas' ? (light ? 0.75 : 0.85) : 0.8;
    for (var i = 0; i < particles.length; i++) {
      var p = particles[i];
      p.y += p.speed * dt;
      p.phase += dt * 1.1;
      p.angle += p.spin * dt;
      var x = p.x + Math.sin(p.phase) * p.sway;
      if (p.y > canvas.height + 12) { particles[i] = spawn(false); continue; }
      ctx.beginPath();
      if (kind === 'christmas') {
        ctx.fillStyle = light ? '#8fa6c4' : '#ffffff';
        ctx.arc(x, p.y, p.r, 0, Math.PI * 2);
      } else {
        ctx.fillStyle = p.color;
        ctx.ellipse(x, p.y, p.r * 1.5, p.r * 0.85, p.angle, 0, Math.PI * 2);
      }
      ctx.fill();
    }
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
