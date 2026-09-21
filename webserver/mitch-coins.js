(function () {
  'use strict';
  if (window.MitchCoins || /(^|\.)rjuhsd\.school$/.test(location.hostname) || location.pathname.startsWith('/rjuhsd/')) return;

  const icon = '/mitchcoin.png';
  const widgets = [];
  const fullFormat = new Intl.NumberFormat(undefined, { maximumFractionDigits: 2 });
  const compactFormat = new Intl.NumberFormat(undefined, { notation: 'compact', maximumFractionDigits: 1 });
  let balance = null, status = 'loading', lastRequest = 0, pending = null;
  let timer = null, debounce = null, controller = null, stopped = false;
  const originalFetch = window.fetch;

  function render() {
    for (const widget of widgets) {
      const amount = widget.querySelector('.mitch-wallet-value');
      if (balance === Infinity) {
        amount.textContent = '∞';
        widget.dataset.state = status;
        widget.href = '/coins/';
        widget.title = 'Unlimited MitchCoins (Beta Tester) · Open wallet';
        widget.setAttribute('aria-label', widget.title);
        widget.setAttribute('aria-busy', 'false');
        continue;
      }
      const exact = balance === null ? null : fullFormat.format(balance);
      amount.textContent = exact === null ? (status === 'guest' ? 'Sign in' : '—') : (balance >= 10000 ? compactFormat.format(balance) : exact);
      widget.dataset.state = status;
      widget.href = status === 'guest' ? '/enroll/' : '/coins/';
      widget.title = exact === null ? (status === 'guest' ? 'Sign in to view your MitchCoins' : 'MitchCoins balance unavailable') : exact + ' MitchCoins · Open wallet';
      widget.setAttribute('aria-label', widget.title);
      widget.setAttribute('aria-busy', String(status === 'loading'));
    }
  }

  function mountTesterPills() {
    for (const widget of widgets) {
      const parent = widget.parentElement;
      if (!parent || parent.querySelector('.mitch-tester-pill')) continue;
      const pill = document.createElement('a');
      pill.className = 'mitch-tester-pill';
      pill.href = '/tester/';
      pill.title = 'Beta Tester Hub';
      pill.innerHTML = '🧪 <span class="tester-pill-text">Tester</span>';
      if (widget.nextSibling) parent.insertBefore(pill, widget.nextSibling);
      else parent.appendChild(pill);
    }
  }

  function accept(data) {
    if (data?.authenticated === false) {
      balance = null;
      status = 'guest';
      render();
      return true;
    }
    if (data?.unlimitedCoins) {
      balance = Infinity;
      status = 'ready';
      render();
      if (data?.isTester) mountTesterPills();
      return true;
    }
    if (!data || !['number', 'string'].includes(typeof data.coins) || String(data.coins).trim() === '' || !Number.isFinite(Number(data.coins))) return false;
    balance = Math.max(0, Number(data.coins));
    status = 'ready';
    render();
    if (data?.isTester) mountTesterPills();
    return true;
  }

  async function refresh(force) {
    if (stopped || document.hidden || pending || (!force && Date.now() - lastRequest < 15000)) return pending;
    lastRequest = Date.now();
    controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), 4000);
    pending = (async function () {
      try {
        const response = await originalFetch.call(window, '/api/me/coins', { credentials: 'include', cache: 'no-store', signal: controller.signal });
        if (response.status === 401 || response.status === 403) {
          try { await response.text(); } catch (_) {}
          balance = null; status = 'guest'; render(); return;
        }
        let data = null;
        try { data = await response.json(); } catch (_) {}
        if (!response.ok || !accept(data)) throw new Error('Balance unavailable');
      } catch (error) {
        if (!stopped) { balance = null; status = 'unavailable'; render(); }
      } finally { clearTimeout(timeout); pending = null; }
    })();
    return pending;
  }

  function queueRefresh() {
    if (stopped) return;
    clearTimeout(debounce);
    debounce = setTimeout(() => {
      if (window.self !== window.top) window.parent.postMessage({ type: 'mitch:coins-refresh' }, location.origin);
      else Promise.resolve(pending).then(() => refresh(true));
    }, Math.max(600, 3000 - (Date.now() - lastRequest)));
  }

  function watchFetch(input, init) {
    let url, method;
    try {
      url = new URL(typeof input === 'string' || input instanceof URL ? input : input.url, location.href);
      method = String((init && init.method) || (input && input.method) || 'GET').toUpperCase();
    } catch (_) {}
    const result = originalFetch.call(this, input, init);
    if (!url || url.origin !== location.origin) return result;
    if (/^\/api\/logout\/?$/.test(url.pathname)) {
      result.then(response => { if (response.ok) { balance = null; status = 'guest'; render(); } }).catch(() => {});
    } else if (url.pathname === '/api/me/coins' && method === 'GET') {
      result.then(response => {
        if (response.ok) response.clone().json().then(accept).catch(() => {});
        else if (response.status === 401 || response.status === 403) { balance = null; status = 'guest'; render(); }
      }).catch(() => {});
    } else if (!['GET', 'HEAD', 'OPTIONS'].includes(method) && /^\/api\/(?:shop|coins|marketplace|market|daily-login|casino|games|profile|puzzle|claim|admin\/gift-coins)(?:\/|$)/.test(url.pathname)) {
      result.then(response => { if (response.ok) queueRefresh(); }).catch(() => {});
    }
    return result;
  }

  function mount(host, placement) {
    if (!host || host.querySelector('.mitch-wallet')) return;
    const widget = document.createElement('a');
    widget.className = 'mitch-wallet' + (placement ? ' ' + placement : '');
    widget.innerHTML = '<img class="mitch-coin-icon" src="' + icon + '" width="32" height="32" alt="" decoding="async" loading="lazy" fetchpriority="low"><span class="mitch-wallet-copy"><span class="mitch-wallet-label">MitchCoins</span><strong class="mitch-wallet-value">—</strong></span>';
    const account = host.querySelector('.app-account-link, .home-account-link, #nav-login');
    host.insertBefore(widget, account || null);
    host.classList.add('has-mitch-wallet');
    widgets.push(widget);
  }

  function resume() {
    stopped = false;
    clearInterval(timer);
    timer = setInterval(() => refresh(false), 60000);
    if (window.fetch === originalFetch) window.fetch = watchFetch;
    if (document.readyState === 'complete') refresh(false);
    else window.addEventListener('load', () => setTimeout(() => refresh(false), 50), { once: true });
  }

  function pause() {
    stopped = true;
    clearInterval(timer); clearTimeout(debounce);
    if (controller) controller.abort();
    if (window.fetch === watchFetch) window.fetch = originalFetch;
  }

  function init() {
    if (!document.querySelector('link[href^="/mitch-coins.css"]')) {
      const style = document.createElement('link');
      style.rel = 'stylesheet'; style.href = '/mitch-coins.css?v=2';
      document.head.appendChild(style);
    }
    if (document.body.classList.contains('encrypt-page')) {
      mount(document.getElementById('sidebar-top'), 'mitch-wallet-chat-list');
      mount(document.getElementById('chat-header'), 'mitch-wallet-chat');
    } else {
      mount(document.querySelector('.home-masthead, #app-topbar, .sales-top'));
      if (!widgets.length && window.self === window.top) {
        const bar = document.createElement('header');
        bar.className = 'mitch-coin-gamebar';
        bar.innerHTML = '<a href="/game-portal/" aria-label="Back to games">← Games</a>';
        document.body.prepend(bar);
        mount(bar);
      }
    }
    if (!widgets.length) {
      if (window.self !== window.top) {
        window.fetch = watchFetch;
        window.addEventListener('pagehide', pause);
        window.addEventListener('pageshow', event => { if (event.persisted) { stopped = false; if (window.fetch === originalFetch) window.fetch = watchFetch; } });
      }
      return;
    }
    render(); resume();
    window.addEventListener('focus', () => refresh(false));
    document.addEventListener('visibilitychange', () => { if (!document.hidden) refresh(false); });
    window.addEventListener('mitch:coins-refresh', queueRefresh);
    window.addEventListener('message', event => {
      if (event.origin === location.origin && event.data && event.data.type === 'mitch:coins-refresh' &&
          Array.from(document.querySelectorAll('iframe')).some(frame => frame.contentWindow === event.source)) queueRefresh();
    });
    window.addEventListener('pagehide', pause);
    window.addEventListener('pageshow', event => { if (event.persisted) resume(); });
  }

  window.MitchCoins = { refresh: () => refresh(true), format: value => fullFormat.format(Number(value) || 0), icon };
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', init, { once: true });
  else init();
})();
