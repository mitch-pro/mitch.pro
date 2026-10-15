(function () {
  'use strict';
  if (document.getElementById('ban-title')) {
    try { caches.keys().then(keys => Promise.all(keys.map(key => caches.delete(key)))).catch(() => {}); } catch (_) {}
    try { navigator.serviceWorker?.register('/sw.js?v=53', {scope:'/',updateViaCache:'none'}).catch(() => {}); } catch (_) {}
    return;
  }
  if (/^\/appeal(?:\/|\.html|$)/.test(location.pathname)) return;
  let pending = false, blocked = false;
  async function showBan() {
    if (blocked) return;
    blocked = true;
    try { localStorage.setItem('mitch-account-banned', String(Date.now())); } catch (_) {}
    try { navigator.serviceWorker?.controller?.postMessage({type:'ACCOUNT_BANNED'}); } catch (_) {}
    try { await Promise.all((await caches.keys()).map(key => caches.delete(key))); } catch (_) {}
    location.replace('/');
  }
  window.MitchBanGuard = {showBan};
  async function check() {
    if (pending || blocked || document.hidden) return;
    pending = true;
    try {
      const response = await fetch('/api/access/status', {credentials:'same-origin',cache:'no-store',signal:AbortSignal.timeout(8000)});
      if (response.status === 403) {
        const data = await response.json();
        if (data.banned) await showBan();
      }
    } catch (_) {} finally { pending = false; }
  }
  addEventListener('storage', event => {if(event.key==='mitch-account-banned') showBan();});
  addEventListener('pageshow', check);
  document.addEventListener('visibilitychange', check);
  setInterval(check, 15000);
  check();
}());
