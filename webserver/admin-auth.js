// Shared admin passphrase handling for every staff page. Load it first in
// <head>, before any script that talks to /api/admin/*.
//
// How it works (details in docs/admin-panel.md):
//  - Typing the admin passphrase once calls POST /api/admin/unlock, which sets
//    an HttpOnly cookie. That cookie unlocks every admin page and tab for
//    several hours; the passphrase itself is never kept in the browser.
//  - Older page scripts still read/write sessionStorage['admin_passphrase'] and
//    send it as X-Admin-Passphrase. This file keeps that working: the real
//    value is swapped for a placeholder (the cookie does the real work), and a
//    localStorage marker lets a new tab skip the lock screen.
//  - Large actions (bans, deletes, role changes, mass sends...) answer
//    `reauth_required`. A fetch wrapper catches that, asks for the passphrase
//    in a dialog and retries the request once. Nothing is remembered.
(function () {
  'use strict';
  if (window.MitchAdminAuth) return;

  var KEY = 'admin_passphrase';
  var PLACEHOLDER = '__unlocked__';
  var MARK = 'admin_unlock_until';
  var nativeFetch = window.fetch.bind(window);
  var proto = Storage.prototype;
  var nativeSet = proto.setItem;
  var pendingUnlock = null;
  var dialogPromise = null;

  function store(k, v) { try { nativeSet.call(sessionStorage, k, v); } catch (_) {} }
  function mark(until) {
    try { localStorage.setItem(MARK, String(until || 0)); } catch (_) {}
  }
  function markedUnlocked() {
    try { return Number(localStorage.getItem(MARK) || 0) > Date.now(); } catch (_) { return false; }
  }

  // New tab: the cookie is shared, so skip the per-page lock screens.
  try {
    if (markedUnlocked() && !sessionStorage.getItem(KEY)) store(KEY, PLACEHOLDER);
  } catch (_) {}

  function apiHeaders(pass) {
    return { 'Content-Type': 'application/json', 'X-Mitch-Requested-With': '1', 'X-Admin-Passphrase': pass };
  }

  // Exchange a passphrase for the unlock cookie. Resolves true/false.
  function unlock(pass) {
    return nativeFetch('/api/admin/unlock', {
      method: 'POST', credentials: 'same-origin', headers: apiHeaders(pass), body: JSON.stringify({ passphrase: pass })
    }).then(function (res) {
      return res.json().catch(function () { return {}; }).then(function (d) {
        if (res.ok && d.ok) { mark(d.unlockExpiresAt); store(KEY, PLACEHOLDER); return true; }
        return false;
      });
    }).catch(function () { return false; });
  }

  // Page scripts save the passphrase they just verified. Keep it out of
  // storage: trade it for the unlock cookie and store the placeholder.
  proto.setItem = function (k, v) {
    if (this === sessionStorage && k === KEY && v && v !== PLACEHOLDER) {
      var p = unlock(String(v));
      pendingUnlock = p;
      p.then(function () { if (pendingUnlock === p) pendingUnlock = null; });
      return nativeSet.call(this, k, PLACEHOLDER);
    }
    return nativeSet.call(this, k, v);
  };

  function lock() {
    mark(0);
    try { sessionStorage.removeItem(KEY); } catch (_) {}
    return nativeFetch('/api/admin/lock', {
      method: 'POST', credentials: 'same-origin', headers: { 'X-Mitch-Requested-With': '1' }
    }).catch(function () {});
  }

  // Passphrase dialog. `reason` is 'unlock' or 'confirm'. Resolves with the
  // verified passphrase, or null when cancelled.
  function ask(reason, what) {
    if (dialogPromise) return dialogPromise;
    var confirmMode = reason === 'confirm';
    dialogPromise = new Promise(function (resolve) {
      var overlay = document.createElement('div');
      overlay.setAttribute('role', 'dialog');
      overlay.setAttribute('aria-modal', 'true');
      overlay.style.cssText = 'position:fixed;inset:0;z-index:2147483000;display:flex;align-items:center;justify-content:center;padding:16px;background:rgba(0,0,0,.55);font-family:"DM Sans",system-ui,sans-serif';
      var card = document.createElement('form');
      card.style.cssText = 'width:min(380px,100%);box-sizing:border-box;padding:22px;border-radius:14px;background:var(--t-bg2,#1c1c22);color:var(--t-fg,#f2f2f5);border:1px solid var(--t-bd,rgba(255,255,255,.14));box-shadow:0 18px 50px rgba(0,0,0,.45)';
      var h = document.createElement('h2');
      h.style.cssText = 'margin:0 0 6px;font-size:18px';
      h.textContent = confirmMode ? 'Confirm with your passphrase' : 'Admin passphrase';
      var p = document.createElement('p');
      p.style.cssText = 'margin:0 0 14px;font-size:14px;color:var(--t-fg2,#a8a8b3);line-height:1.4';
      p.textContent = confirmMode
        ? (what ? what + ' needs your passphrase again.' : 'This action needs your passphrase again.')
        : 'Enter it once to unlock the admin pages for a few hours.';
      var input = document.createElement('input');
      input.type = 'password';
      input.autocomplete = 'current-password';
      input.setAttribute('aria-label', 'Admin passphrase');
      input.style.cssText = 'width:100%;box-sizing:border-box;padding:10px 12px;border-radius:9px;border:1px solid var(--t-bd,rgba(255,255,255,.2));background:var(--t-bg,#101014);color:inherit;font:inherit';
      var err = document.createElement('p');
      err.setAttribute('role', 'status');
      err.style.cssText = 'min-height:18px;margin:8px 0 0;font-size:13px;color:#ff6b7a';
      var row = document.createElement('div');
      row.style.cssText = 'display:flex;gap:8px;justify-content:flex-end;margin-top:10px';
      function btn(text, primary) {
        var b = document.createElement('button');
        b.type = primary ? 'submit' : 'button';
        b.textContent = text;
        b.style.cssText = 'padding:9px 16px;border-radius:9px;font:inherit;cursor:pointer;border:1px solid var(--t-bd,rgba(255,255,255,.2));' +
          (primary ? 'background:var(--t-ac,#5b8cff);color:#fff;border-color:transparent' : 'background:transparent;color:inherit');
        return b;
      }
      var cancel = btn('Cancel', false);
      var ok = btn(confirmMode ? 'Confirm' : 'Unlock', true);
      var link = document.createElement('a');
      link.href = '/staff-dashboard/passphrase-reset/';
      link.textContent = 'Forgot your passphrase?';
      link.style.cssText = 'display:inline-block;margin-top:10px;font-size:13px;color:var(--t-fg2,#a8a8b3)';
      row.appendChild(cancel); row.appendChild(ok);
      [h, p, input, err, row, link].forEach(function (n) { card.appendChild(n); });
      overlay.appendChild(card);
      document.body.appendChild(overlay);
      input.focus();
      function done(v) {
        overlay.remove();
        dialogPromise = null;
        resolve(v);
      }
      cancel.addEventListener('click', function () { done(null); });
      overlay.addEventListener('keydown', function (e) { if (e.key === 'Escape') done(null); });
      card.addEventListener('submit', function (e) {
        e.preventDefault();
        var pass = input.value.trim();
        if (!pass) return;
        ok.disabled = true;
        unlock(pass).then(function (good) {
          if (good) { done(pass); return; }
          ok.disabled = false;
          err.textContent = 'Incorrect passphrase.';
          input.select();
        });
      });
    });
    return dialogPromise;
  }

  function urlOf(input) {
    try { return new URL(typeof input === 'string' ? input : input.url, location.href); } catch (_) { return null; }
  }

  function describe(url) {
    var name = (url.pathname.split('/').filter(Boolean).pop() || '').replace(/[-_]/g, ' ');
    return name ? 'The "' + name + '" action' : '';
  }

  function retryWith(input, init, pass) {
    var headers = new Headers((init && init.headers) || (input instanceof Request ? input.headers : undefined));
    headers.set('X-Admin-Passphrase', pass);
    if (input instanceof Request) return nativeFetch(new Request(input, { headers: headers }));
    return nativeFetch(input, Object.assign({}, init, { headers: headers }));
  }

  window.fetch = function (input, init) {
    var url = urlOf(input);
    var isApi = !!url && url.origin === location.origin && url.pathname.indexOf('/api/') === 0;
    if (!isApi) return nativeFetch(input, init);
    // A passphrase handed over a moment ago is still becoming a cookie.
    var ready = pendingUnlock ? pendingUnlock.then(function () {}) : Promise.resolve();
    return ready.then(function () { return nativeFetch(input, init); }).then(function (res) {
      // The verify/unlock calls report wrong passphrases to their own callers.
      var own = url.pathname === '/api/admin/passphrase-status' || url.pathname === '/api/admin/unlock' || url.pathname.indexOf('/api/admin/passphrase-reset/') === 0;
      if (res.status !== 403 || own || url.pathname.indexOf('/api/admin/') !== 0) return res;
      return res.clone().json().catch(function () { return {}; }).then(function (d) {
        var code = d && d.error;
        if (code !== 'reauth_required' && code !== 'invalid_passphrase') return res;
        return ask(code === 'reauth_required' ? 'confirm' : 'unlock', describe(url)).then(function (pass) {
          return pass ? retryWith(input, init, pass) : res;
        });
      });
    });
  };

  window.MitchAdminAuth = { lock: lock, unlock: unlock, ask: ask };
})();
