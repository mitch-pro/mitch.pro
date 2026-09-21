(function () {
  'use strict';

  // ── Service worker freshness ────────────────────────────────────────────────
  // The SW keeps caching on (network-first for pages/code, cache-first for
  // media) but a phone can sit on an old worker whose cache predates the
  // current site. Navigation-triggered update checks are throttled by
  // browsers, so every page that runs the app shell forces one: register the
  // worker (updateViaCache:'none' bypasses the HTTP cache for the script
  // itself), call update() on load and every 30 minutes, and reload once when
  // a freshly deployed worker takes control so the page refetches through it.
  if ('serviceWorker' in navigator) {
    var seenSW;
    try { seenSW = JSON.parse(sessionStorage.getItem('sw-scripts') || '{}'); } catch (_) { seenSW = {}; }
    if (navigator.serviceWorker.controller) seenSW[navigator.serviceWorker.controller.scriptURL] = 1;
    navigator.serviceWorker.addEventListener('controllerchange', function () {
      var s = navigator.serviceWorker.controller;
      if (!s || seenSW[s.scriptURL]) return;
      seenSW[s.scriptURL] = 1;
      try { sessionStorage.setItem('sw-scripts', JSON.stringify(seenSW)); } catch (_) {}
      location.reload();
    });
    navigator.serviceWorker.register('/sw.js?v=42', { scope: '/', updateViaCache: 'none' })
      .then(function (reg) {
        try { reg.update(); } catch (_) {}
        setInterval(function () { try { reg.update(); } catch (_) {} }, 30 * 60 * 1000);
      })
      .catch(function () {});
  }

  // rjuhsd.school shares this webroot for its sub-apps (bell, chat,
  // preferences) — its topbar gets the school brand and only links that
  // exist there (the mitch-only sections 404 under that host).
  var IS_RJUHSD = /(^|\.)rjuhsd\.school$/.test(location.hostname || '');

  var NAV_MITCH = [
    { href: '/', label: 'Home', icon: '⌂', match: function (p) { return p === '/' || p === '/index.html'; } },
    { href: '/vms/', label: 'VM Lab', badge: 'FREE', icon: '▣', match: function (p) { return p.indexOf('/vms') === 0; } },
    { href: '/matrix/', label: 'Chat', icon: '◉', match: function (p) { return p.indexOf('/matrix') === 0 || p.indexOf('/encrypt') === 0 || p.indexOf('/public-chat') === 0; } },
    { href: '/game-portal/', label: 'Games', icon: '◆', match: function (p) { return p.indexOf('/games') === 0 || p.indexOf('/game-portal') === 0; } },
    { href: 'https://woodcreek.site/', label: 'Blooket Bot', icon: '▣', match: function () { return false; } },
    { href: '/members/', label: 'People', icon: '●', match: function (p) { return p.indexOf('/members') === 0 || p.indexOf('/friends') === 0 || p.indexOf('/profile') === 0; } },
    { href: 'https://rjuhsd.school/', label: 'Schedule', icon: '◷', match: function () { return false; } }
  ];

  var NAV_RJUHSD = [
    { href: '/', label: 'Home', icon: '⌂', match: function (p) { return p === '/' || p === '/index.html'; } },
    { href: '/matrix/', label: 'Chat', icon: '◉', match: function (p) { return p.indexOf('/matrix') === 0 || p.indexOf('/encrypt') === 0 || p.indexOf('/public-chat') === 0; } },
    { href: '/#schedule-panel', label: 'Bell', icon: '◷', match: function () { return false; } },
    { href: '/preferences/', label: 'Settings', icon: '⚙', match: function (p) { return p.indexOf('/preferences') === 0; } }
  ];

  var NAV = IS_RJUHSD ? NAV_RJUHSD : NAV_MITCH;

  var profileRefreshTimer = null;
  function refreshProfileSurfaces(detail) {
    window.dispatchEvent(new CustomEvent('mitch-profile-updated', { detail: detail || {} }));
    clearTimeout(profileRefreshTimer);
    profileRefreshTimer = setTimeout(function () {
      if (typeof window.loadMembers === 'function') window.loadMembers();
      if (typeof window.loadOnlineMembers === 'function') window.loadOnlineMembers();
    }, 40);
  }
  window.addEventListener('ws-broadcast-message', function (event) {
    if (event.detail && event.detail.type === 'profile_updated') refreshProfileSurfaces(event.detail);
  });
  window.addEventListener('storage', function (event) {
    if (event.key !== '_mitchProfileUpdated' || !event.newValue) return;
    try { refreshProfileSurfaces(JSON.parse(event.newValue)); }
    catch (_) { refreshProfileSurfaces({}); }
  });

  function ensureRelaunchStyles() {
    if (document.getElementById('mitch-relaunch') || document.querySelector('link[href="/relaunch.css"], link[href^="/relaunch.css?"]')) return;
    var link = document.createElement('link');
    link.id = 'mitch-relaunch';
    link.rel = 'stylesheet';
    link.href = '/relaunch.css';
    (document.head || document.getElementsByTagName('head')[0]).appendChild(link);
  }

  function ensurePortalStyles() {
    if (document.querySelector('link[href="/portal-redesign.css"], link[href^="/portal-redesign.css?"]')) return;
    var link = document.createElement('link');
    link.rel = 'stylesheet';
    link.href = '/portal-redesign.css?v=16';
    (document.head || document.getElementsByTagName('head')[0]).appendChild(link);
  }

  function enhanceMobileShell() {
    if (IS_RJUHSD || location.pathname.indexOf('/rjuhsd/') === 0 || document.getElementById('mobile-dock')) return;
    document.body.classList.add('mitch-next');
    var homeBar = document.querySelector('.home-masthead');
    if (homeBar) {
      homeBar.classList.add('unified-masthead');
      // broadcast.js builds its floating bell toolbar during parsing, before
      // this deferred script runs — fold it into the masthead instead of
      // leaving a second fixed #site-topbar stacked in the top-right corner.
      var floatBar = document.getElementById('site-topbar');
      if (floatBar && floatBar !== homeBar) {
        while (floatBar.firstChild) homeBar.appendChild(floatBar.firstChild);
        floatBar.remove();
      }
      homeBar.id = 'site-topbar';
      ['theme-btn', 'sw-notif-wrap'].forEach(function (id) { var control = document.getElementById(id); if (control) homeBar.appendChild(control); });
    }
    var style = document.createElement('link');
    style.rel = 'stylesheet';
    style.href = '/mitch-ui.css?v=3';
    document.head.appendChild(style);
    var paths = {
      home: '<path d="m3 10 9-7 9 7v10H3Z"/><path d="M9 20v-7h6v7"/>',
      games: '<path d="M7 7h10c3 0 5 11 3 12-2 1-5-3-5-3H9s-3 4-5 3C2 18 4 7 7 7Z"/><path d="M7 10v5m-2-2h4m6-2h.01M18 14h.01"/>',
      chat: '<path d="M4 4h16v12H9l-5 4Z"/><path d="M8 8h8M8 12h5"/>',
      people: '<circle cx="9" cy="8" r="3"/><path d="M3 21v-3a6 6 0 0 1 12 0v3m2-16a3 3 0 0 1 0 6m1 3a5 5 0 0 1 3 5v2"/>',
      more: '<rect x="4" y="4" width="6" height="6" rx="1"/><rect x="14" y="4" width="6" height="6" rx="1"/><rect x="4" y="14" width="6" height="6" rx="1"/><rect x="14" y="14" width="6" height="6" rx="1"/>'
    };
    function icon(name) { return '<svg viewBox="0 0 24 24" aria-hidden="true">' + paths[name] + '</svg>'; }
    var dock = document.createElement('nav');
    dock.id = 'mobile-dock';
    dock.setAttribute('aria-label', 'Mobile navigation');
    [['/', 'Home', 'home'], ['/game-portal/', 'Games', 'games'], ['/matrix/', 'Chat', 'chat'], ['/members/', 'People', 'people']].forEach(function (item) {
      var link = document.createElement('a');
      link.href = item[0];
      link.innerHTML = icon(item[2]) + '<span>' + item[1] + '</span>';
      var path = currentPath();
      if (path === item[0].replace(/\/$/, '') || (item[0] === '/' && path === '/') || (item[2] === 'games' && /^\/(games|game-portal|msn-games)/.test(path)) || (item[2] === 'chat' && /^\/(matrix|encrypt|public-chat)/.test(path))) link.setAttribute('aria-current', 'page');
      dock.appendChild(link);
    });
    var more = document.createElement('button');
    more.type = 'button';
    more.innerHTML = icon('more') + '<span>More</span>';
    more.setAttribute('aria-haspopup', 'dialog');
    more.setAttribute('aria-controls', 'mobile-menu');
    dock.appendChild(more);
    var menu = document.createElement('dialog');
    menu.id = 'mobile-menu';
    menu.setAttribute('aria-labelledby', 'mobile-menu-title');
    menu.innerHTML = '<header><h2 id="mobile-menu-title">mitch.pro</h2><button type="button" aria-label="Close menu">×</button></header><nav aria-label="All sections"></nav>';
    var links = menu.querySelector('nav');
    [['https://woodcreek.site/', 'Blooket Bot'], ['/profile/', 'My profile'], ['/preferences/', 'Customizer'], ['/friends/', 'Friends'], ['/coins/', 'MitchCoins'], ['/shop/', 'Shop'], ['/marketplace/', 'Marketplace'], ['/inventory/', 'Inventory'], ['/leaderboard/', 'Leaderboard'], ['https://rjuhsd.school/', 'Bell schedule'], ['/public-chat/', 'Public chat'], ['/canvas/', 'Canvas'], ['/vms/', 'VM Lab'], ['/notifications/', 'Notifications'], ['/blog/', 'Blog'], ['/invite/', 'Invite friends'], ['/feedback/', 'Feedback'], ['/faq/', 'Help']].forEach(function (item) {
      var link = document.createElement('a'); link.href = item[0]; link.textContent = item[1]; links.appendChild(link);
    });
    more.addEventListener('click', function () { menu.showModal(); });
    menu.querySelector('button').addEventListener('click', function () { menu.close(); });
    menu.addEventListener('click', function (event) { if (event.target === menu) { var box = menu.getBoundingClientRect(); if (event.clientY < box.top || event.clientY > box.bottom || event.clientX < box.left || event.clientX > box.right) menu.close(); } });
    menu.addEventListener('close', function () { more.focus(); });
    var mobile = matchMedia('(max-width: 760px)');
    mobile.addEventListener('change', function () { if (!mobile.matches && menu.open) menu.close(); });
    document.body.appendChild(dock);
    document.body.appendChild(menu);
    if (window.visualViewport) {
      function syncMobileKeyboard() {
        var editing = document.activeElement && document.activeElement.matches('input, textarea, [contenteditable="true"]');
        document.body.classList.toggle('mobile-keyboard', !!editing && innerHeight - visualViewport.height > 140);
      }
      visualViewport.addEventListener('resize', syncMobileKeyboard);
      document.addEventListener('focusin', syncMobileKeyboard);
      document.addEventListener('focusout', function () { setTimeout(syncMobileKeyboard, 0); });
    }
    var profile = document.getElementById('member-profile-panel');
    if (profile) {
      var close = document.createElement('button');
      close.className = 'chat-profile-close'; close.type = 'button'; close.textContent = 'Close details';
      close.addEventListener('click', function () {
        document.body.classList.remove('chat-details-open');
        var trigger = document.getElementById('chat-details-btn');
        if (trigger) { trigger.setAttribute('aria-expanded', 'false'); trigger.focus(); }
      });
      profile.prepend(close);
      new MutationObserver(function () { if (!profile.contains(close)) profile.prepend(close); }).observe(profile, { childList: true });
    }
  }

  function ensureViewport() {
    var current = document.querySelector('meta[name="viewport"]');
    if (current) {
      if (current.content.indexOf('viewport-fit=cover') === -1) current.content += ', viewport-fit=cover';
      return;
    }
    var meta = document.createElement('meta');
    meta.name = 'viewport';
    meta.content = 'width=device-width, initial-scale=1, viewport-fit=cover';
    var head = document.head || document.getElementsByTagName('head')[0];
    if (head) head.insertBefore(meta, head.firstChild);
  }

  function ensureInstallMetadata() {
    var metas = {
      'theme-color': IS_RJUHSD ? '#0c0809' : '#0b0e14',
      'mobile-web-app-capable': 'yes',
      'apple-mobile-web-app-capable': 'yes',
      'apple-mobile-web-app-status-bar-style': 'black-translucent',
      'apple-mobile-web-app-title': IS_RJUHSD ? 'RJUHSD Hub' : 'mitch.pro'
    };
    Object.keys(metas).forEach(function (name) {
      if (document.querySelector('meta[name="' + name + '"]')) return;
      var meta = document.createElement('meta');
      meta.name = name;
      meta.content = metas[name];
      document.head.appendChild(meta);
    });
    if (!document.querySelector('link[rel="manifest"]')) {
      var manifest = document.createElement('link');
      manifest.rel = 'manifest';
      manifest.href = '/manifest.json';
      document.head.appendChild(manifest);
    }
    if (!document.querySelector('link[rel="apple-touch-icon"]')) {
      var touchIcon = document.createElement('link');
      touchIcon.rel = 'apple-touch-icon';
      touchIcon.sizes = '180x180';
      touchIcon.href = IS_RJUHSD ? '/rjuhsd-assets/apple-touch-icon.png' : '/apple-touch-icon.png';
      document.head.appendChild(touchIcon);
    }
  }

  function ensureFonts() {
    if (document.getElementById('app-fonts')) return;
    if (document.querySelector('link[href*="fonts.googleapis.com"][href*="Figtree"]')) return;
    var link = document.createElement('link');
    link.id = 'app-fonts';
    link.rel = 'stylesheet';
    link.href = 'https://fonts.googleapis.com/css2?family=Figtree:wght@400;500;600;700;800&display=swap';
    var head = document.head || document.getElementsByTagName('head')[0];
    if (head) head.appendChild(link);
  }

  function currentPath() {
    try {
      return (location.pathname || '/').replace(/\/+$/, '') || '/';
    } catch (e) {
      return '/';
    }
  }

  function buildTopbar() {
    var path = currentPath();
    var bar = document.createElement('header');
    bar.id = 'app-topbar';
    bar.className = 'app-topbar unified-masthead';
    bar.setAttribute('role', 'banner');

    var brand = document.createElement('a');
    brand.className = 'app-brand';
    brand.href = '/';
    if (IS_RJUHSD) {
      brand.innerHTML = '<img class="site-logo" src="/icon-192.png" alt="" width="35" height="35"><b>RJUHSD<span>.school</span></b>';
      brand.setAttribute('aria-label', 'rjuhsd.school home');
    } else {
      brand.innerHTML = '<img class="site-logo" src="/icon-192.png" alt="" width="35" height="35"><b>mitch<span>.pro</span></b>';
      brand.setAttribute('aria-label', 'mitch.pro home');
    }

    var nav = document.createElement('nav');
    nav.className = 'app-nav';
    nav.setAttribute('aria-label', 'Primary');

    for (var i = 0; i < NAV.length; i++) {
      var item = NAV[i];
      var a = document.createElement('a');
      a.href = item.href;
      a.textContent = item.label;
      if (item.badge) {
        var badge = document.createElement('span');
        badge.className = 'nav-pill-free';
        badge.textContent = item.badge;
        a.appendChild(badge);
      }
      if (item.match(path)) a.setAttribute('aria-current', 'page');
      nav.appendChild(a);
    }

    bar.appendChild(brand);
    bar.appendChild(nav);

    var account = document.createElement('a');
    account.className = 'app-account-link';
    account.href = IS_RJUHSD ? '/preferences/' : '/profile/';
    account.innerHTML = '<svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="8" r="3.5"/><path d="M5 21v-2a7 7 0 0 1 14 0v2"/></svg><span>My account</span>';
    account.setAttribute('aria-label', 'Open your profile');
    bar.appendChild(account);
    return bar;
  }

  function bindScrollState(bar) {
    var ticking = false;
    function update() {
      ticking = false;
      if (window.scrollY > 8) bar.classList.add('is-scrolled');
      else bar.classList.remove('is-scrolled');
    }
    window.addEventListener('scroll', function () {
      if (ticking) return;
      ticking = true;
      window.requestAnimationFrame(update);
    }, { passive: true });
    update();
  }

  function enhanceRelaunchMotion() {
    if (!document.body) return;
    document.body.classList.add('relaunch-ready');
    if (window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches) return;
    var items = document.querySelectorAll('.page-head, .panel, .card, .category-group, .listing-item, .rank-row, .admin-day-card');
    if (!('IntersectionObserver' in window)) {
      for (var fallbackIndex = 0; fallbackIndex < items.length; fallbackIndex++) items[fallbackIndex].classList.add('is-visible');
      return;
    }
    var observer = new IntersectionObserver(function (entries) {
      for (var i = 0; i < entries.length; i++) {
        if (!entries[i].isIntersecting) continue;
        entries[i].target.classList.add('is-visible');
        window.setTimeout(function (node) {
          node.classList.remove('relaunch-reveal', 'is-visible');
          node.style.removeProperty('--reveal-order');
        }, 1100, entries[i].target);
        observer.unobserve(entries[i].target);
      }
    }, { rootMargin: '0px 0px -5% 0px', threshold: 0.06 });
    for (var itemIndex = 0; itemIndex < items.length; itemIndex++) {
      items[itemIndex].classList.add('relaunch-reveal');
      items[itemIndex].style.setProperty('--reveal-order', String(itemIndex % 8));
      observer.observe(items[itemIndex]);
    }
  }

  function shouldInject() {
    var body = document.body;
    if (!body) return false;
    if (body.dataset.shell === 'off') return false;
    if (document.getElementById('app-topbar')) return false;
    return true;
  }

  function inject() {
    document.body.classList.add('mitch-design');
    document.body.dataset.page = currentPath().split('/')[1] || 'home';
    ensureViewport();
    ensureInstallMetadata();
    ensureFonts();
    ensureRelaunchStyles();
    ensurePortalStyles();
    enhanceMobileShell();
    enhanceInterface();
    if (!IS_RJUHSD && !document.querySelector('script[src^="/mitch-coins.js"]')) {
      var coins = document.createElement('script');
      coins.src = '/mitch-coins.js?v=2';
      document.head.appendChild(coins);
    }
    if (!shouldInject()) {
      window.MitchShell = { ready: true, injected: false };
      return;
    }

    var bar = buildTopbar();
    var body = document.body;
    body.insertBefore(bar, body.firstChild);
    var floating = document.querySelector('#site-topbar.sw-standalone');
    if (floating) {
      while (floating.firstChild) bar.appendChild(floating.firstChild);
      floating.remove();
    }
    if (!body.classList.contains('app-shell')) {
      body.classList.add('app-shell');
    }
    bindScrollState(bar);
    window.MitchShell = { ready: true, injected: true };
  }

  function enhanceInterface() {
    var main = document.querySelector('main, #mainpage');
    if (main) {
      if (!main.id) main.id = 'main-content';
      main.setAttribute('tabindex', '-1');
      var skip = document.createElement('a');
      skip.href = '#' + main.id;
      skip.className = 'skip-link';
      skip.textContent = 'Skip to content';
      document.body.prepend(skip);
    }
    var search = document.getElementById('home-search');
    if (search) {
      search.setAttribute('aria-label', 'Search pages');
      document.addEventListener('keydown', function (event) {
        if (event.key === '/' && !event.ctrlKey && !event.metaKey && !event.altKey && !event.target.closest('input, textarea, select, [contenteditable]')) {
          event.preventDefault();
          search.focus();
          search.scrollIntoView({ block: 'center' });
        }
        if (event.key === 'Escape' && document.activeElement === search) {
          search.value = '';
          search.dispatchEvent(new Event('input', { bubbles: true }));
          search.blur();
        }
      });
    }
    var details = document.getElementById('chat-details-btn');
    if (details) {
      details.addEventListener('click', function () {
        var open = document.body.classList.toggle('chat-details-open');
        details.setAttribute('aria-expanded', String(open));
      });
      document.addEventListener('keydown', function (event) {
        if (event.key === 'Escape' && document.body.classList.contains('chat-details-open')) {
          document.body.classList.remove('chat-details-open');
          details.setAttribute('aria-expanded', 'false');
          details.focus();
        }
      });
    }
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', inject);
  } else {
    inject();
  }
})();
