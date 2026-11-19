/* Tells Premium members they can get a free @student.mitch.pro address.
   Shown once per browser tab session until they apply or press Dismiss.
   Dismiss is saved on the account (POST /api/premium/email/dismiss), so it
   never comes back, on any device. Loaded by app-shell.js. */
(function () {
  'use strict';
  if (window.top !== window) return;
  var CHECKED = 'premium-email-checked';
  try { if (sessionStorage.getItem(CHECKED)) return; } catch (_) {}

  function remember() { try { sessionStorage.setItem(CHECKED, '1'); } catch (_) {} }

  function show() {
    if (document.getElementById('premium-email-prompt')) return;
    var box = document.createElement('div');
    box.id = 'premium-email-prompt';
    box.setAttribute('role', 'dialog');
    box.setAttribute('aria-label', 'Free email address');
    box.style.cssText = 'position:fixed;right:16px;bottom:16px;z-index:2147483000;max-width:340px;padding:16px;border-radius:12px;background:var(--t-bg2,#1e1a17);color:var(--t-fg,#e8e6e3);border:1px solid var(--t-bd,#3d3a37);box-shadow:0 8px 32px rgba(0,0,0,.4);font:14px/1.5 "DM Sans",system-ui,sans-serif';

    var title = document.createElement('strong');
    title.textContent = 'You can get a free email address';
    title.style.display = 'block';

    var body = document.createElement('p');
    body.style.cssText = 'margin:6px 0 12px;color:var(--t-fg2,#bababa)';
    body.textContent = 'Premium includes your own name@student.mitch.pro address.';

    var row = document.createElement('div');
    row.style.cssText = 'display:flex;gap:8px;flex-wrap:wrap';

    var get = document.createElement('a');
    get.href = '/premium-email/';
    get.textContent = 'Get my email';
    get.style.cssText = 'padding:6px 14px;border-radius:8px;border:1px solid var(--t-ac,#8b5cf6);background:var(--t-ac,#8b5cf6);color:#fff;font:inherit;font-weight:600;text-decoration:none';

    var dismiss = document.createElement('button');
    dismiss.type = 'button';
    dismiss.textContent = 'Dismiss';
    dismiss.style.cssText = 'padding:6px 14px;border-radius:8px;border:1px solid var(--t-bd,#3d3a37);background:transparent;color:inherit;font:inherit;cursor:pointer';
    dismiss.addEventListener('click', function () {
      box.remove();
      fetch('/api/premium/email/dismiss', {
        method: 'POST',
        credentials: 'include',
        headers: { 'Content-Type': 'application/json', 'X-Mitch-Requested-With': '1' },
        body: '{}'
      }).catch(function () {});
    });

    row.append(get, dismiss);
    box.append(title, body, row);
    document.body.appendChild(box);
  }

  fetch('/api/premium/email/status', { credentials: 'include', cache: 'no-store' })
    .then(function (r) {
      // Signed out (401) or not Premium (403): nothing to offer, don't ask again this session.
      if (!r.ok) { remember(); return null; }
      return r.json();
    })
    .then(function (data) {
      if (!data) return;
      if (!data.showPrompt) { remember(); return; }
      if (document.body) show(); else document.addEventListener('DOMContentLoaded', show);
    })
    .catch(function () {});
})();
