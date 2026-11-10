(function () {
  'use strict';
  const indicator = document.getElementById('site-active');
  if (!indicator) return;
  // rjuhsd.school counts its visitors (online.js); every other host counts
  // signed-in accounts. localhost can opt in with <html data-visitor-count>
  // so the school page can be tested locally.
  const host = location.hostname;
  const local = host === 'localhost' || host === '127.0.0.1';
  if (/(^|\.)rjuhsd\.school$/.test(host) || (local && document.documentElement.hasAttribute('data-visitor-count'))) {
    if (!window.__rjuhsdOnlineLoaded) {
      window.__rjuhsdOnlineLoaded = true;
      const script = document.createElement('script');
      script.src = '/rjuhsd-assets/online.js?v=3';
      document.head.appendChild(script);
    }
    return;
  }
  let pending = false;

  async function refresh() {
    if (pending || document.hidden) return;
    pending = true;
    try {
      const response = await fetch('/api/presence/count', {
        credentials: 'same-origin',
        cache: 'no-store',
        signal: AbortSignal.timeout(8000)
      });
      if (!response.ok) throw new Error('Presence unavailable');
      const data = await response.json();
      if (!Number.isSafeInteger(data.count) || data.count < 0) throw new Error('Invalid count');
      const label = data.count.toLocaleString() + ' active';
      if (indicator.textContent !== label) indicator.textContent = label;
      indicator.title = data.count.toLocaleString() + (data.count === 1 ? ' member online now' : ' members online now');
      indicator.hidden = false;
    } catch (_) {
      indicator.hidden = true;
    } finally {
      pending = false;
    }
  }

  refresh();
  setInterval(refresh, 20000);
  document.addEventListener('visibilitychange', refresh);
  window.addEventListener('online', refresh);
}());
