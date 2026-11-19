/* Shows people's equipped shop cosmetics in the chat client: name color, badge,
   chat effect and a profile effect on their message rows. Styles are in
   /cosmetics.css.

   The chat client is a React app, so this only sets data-mx-* attributes on
   existing elements (React leaves unknown attributes alone) and never adds or
   moves nodes. Names are found by [data-user-id]; avatars (no text) are skipped.
   Loadouts come from GET /api/matrix/cosmetics in batches and are cached for the
   page's life; the signed-in user's own site theme in the same response is
   remembered in the shop_theme cookie so site-theme.js can apply it. */
(function () {
  'use strict';

  var BADGES = {
    verified_badge: '✔', og_badge: '📜', artist_badge: '🎨', chess_badge: '♟',
    builder_badge: '🛠', lucky_badge: '🍀', premium_star_badge: '⭐', owner_fan_badge: '👑',
    gamer_badge: '🎮', music_badge: '🎵', rocket_badge: '🚀', flame_badge: '🔥', heart_badge: '❤'
  };
  var cache = Object.create(null);   // user id -> loadout object, or null when none
  var pending = Object.create(null); // ids waiting for the next batch
  var timer = null;
  var failedUntil = 0;

  function nameElements(root) {
    var found = [];
    var list = (root.querySelectorAll ? root.querySelectorAll('[data-user-id]') : []);
    if (root.matches && root.matches('[data-user-id]')) found.push(root);
    for (var i = 0; i < list.length; i++) found.push(list[i]);
    return found.filter(function (el) {
      return el.textContent && el.textContent.trim() && !el.querySelector('img');
    });
  }

  function set(el, name, value) {
    if (value) { if (el.getAttribute(name) !== value) el.setAttribute(name, value); }
    else if (el.hasAttribute(name)) el.removeAttribute(name);
  }

  function apply(el) {
    var id = el.getAttribute('data-user-id');
    var l = cache[id];
    set(el, 'data-mx-color', l && l.color);
    set(el, 'data-mx-effect', l && l.chatEffect);
    set(el, 'data-mx-badge', l && BADGES[l.badge]);
    var row = el.closest && el.closest('[data-message-id]');
    if (row) set(row, 'data-mx-profile', l && l.profileEffect);
  }

  function scan(root) {
    nameElements(root).forEach(function (el) {
      var id = el.getAttribute('data-user-id');
      if (!/^@[^:\s]+:mitch\.pro$/i.test(id || '')) return;
      if (id in cache) { apply(el); return; }
      pending[id] = true;
    });
    if (Object.keys(pending).length && !timer) timer = setTimeout(flush, 250);
  }

  function rememberTheme(theme) {
    try {
      var secure = location.protocol === 'https:' ? '; Secure' : '';
      document.cookie = 'shop_theme=' + encodeURIComponent(theme || '') + '; Path=/; Max-Age=31536000; SameSite=Lax' + secure;
    } catch (_) {}
  }

  async function flush() {
    timer = null;
    if (Date.now() < failedUntil) return;
    var ids = Object.keys(pending).slice(0, 100);
    ids.forEach(function (id) { delete pending[id]; });
    if (!ids.length) return;
    try {
      var res = await fetch('/api/matrix/cosmetics?ids=' + encodeURIComponent(ids.join(',')), { credentials: 'same-origin', cache: 'no-store' });
      if (!res.ok) throw new Error('status ' + res.status);
      var data = await res.json();
      var users = data.users || {};
      ids.forEach(function (id) { cache[id] = users[id] || null; });
      if (data.me) rememberTheme(data.me.theme);
      scan(document.body);
    } catch (_) {
      failedUntil = Date.now() + 30000;   // back off; try again later
    }
    if (Object.keys(pending).length) timer = setTimeout(flush, 250);
  }

  function start() {
    scan(document.body);
    var queued = false;
    new MutationObserver(function () {
      if (queued) return;
      queued = true;
      requestAnimationFrame(function () { queued = false; scan(document.body); });
    }).observe(document.body, { childList: true, subtree: true });
  }

  if (document.body) start(); else document.addEventListener('DOMContentLoaded', start);
})();
