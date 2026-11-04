// Portal page. The list comes from /api/portal/* one page at a time; nothing
// here ever downloads the whole library.
(() => {
  'use strict';

  const $ = id => document.getElementById(id);
  const PAGE = 48;
  // ?probe=<letters> strips the page down for finding what a school filter
  // reacts to: i = no thumbnails, n = hide titles, l = no list at all.
  const probe = new URLSearchParams(location.search).get('probe') || '';
  // Players that can run inside the page. Everything else opens in a new tab.
  const EMBED_PREFIXES = ['/prox/luma/', '/prox/calculated2/', '/prox/gamemonetize/', '/games/', '/game-portal/ui/open-source/'];
  // Same storage keys as the older game-portal page, so saved and recent lists carry over.
  const KEYS = { saved: 'mitch.games.catalog.favorites', recent: 'mitch.games.catalog.recent' };

  const readIds = key => {
    try { const list = JSON.parse(localStorage.getItem(key) || '[]'); return Array.isArray(list) ? list.filter(Number.isInteger) : []; }
    catch { return []; }
  };
  const writeIds = (key, list) => { try { localStorage.setItem(key, JSON.stringify(list)); } catch { /* storage full or blocked */ } };

  const S = {
    view: 'all', category: 'All', query: '', sort: 'popular',
    items: [], total: 0, loading: false, token: 0,
    summary: null, cache: new Map(),
    saved: readIds(KEYS.saved), recent: readIds(KEYS.recent),
    playing: null, rewardTimer: null
  };

  function h(tag, attrs = {}, ...children) {
    const node = document.createElement(tag);
    for (const [name, value] of Object.entries(attrs)) {
      if (name === 'class') node.className = value;
      else if (name.startsWith('on')) node.addEventListener(name.slice(2), value);
      else node.setAttribute(name, value);
    }
    node.append(...children);
    return node;
  }

  async function api(name, params = {}) {
    if (probe.includes('l')) return null;
    const query = new URLSearchParams(params).toString();
    try {
      const response = await fetch(`/api/portal/${name}${query ? '?' + query : ''}`, { credentials: 'same-origin' });
      return response.ok ? await response.json() : null;
    } catch { return null; }
  }

  const remember = item => { S.cache.set(item.id, item); return item; };

  async function itemsById(ids) {
    const missing = ids.filter(id => !S.cache.has(id)).slice(0, 60);
    if (missing.length) ((await api('items', { ids: missing.join(',') }))?.items || []).forEach(remember);
    return ids.map(id => S.cache.get(id)).filter(Boolean);
  }

  const canEmbed = item => !item.external && EMBED_PREFIXES.some(prefix => item.url.startsWith(prefix));

  // ── cards ──────────────────────────────────────────────────────────────
  function makeCard(item) {
    const thumb = h('div', { class: 'pt-thumb' }, h('span', {}, item.title.slice(0, 2).toUpperCase()));
    if (item.image && !probe.includes('i')) {
      const img = h('img', { src: item.image, alt: '', loading: 'lazy', decoding: 'async' });
      img.addEventListener('error', () => img.remove(), { once: true });
      thumb.append(img);
    }
    const saved = S.saved.includes(item.id);
    return h('article', { class: 'pt-card' },
      h('button', { type: 'button', class: 'pt-open', onclick: () => play(item) },
        thumb,
        h('span', { class: 'pt-info' },
          h('span', { class: 'pt-name' }, probe.includes('n') ? '•••' : item.title),
          h('span', { class: 'pt-meta' }, item.category + (canEmbed(item) ? '' : ' · new tab')))),
      h('button', {
        type: 'button', class: 'pt-save', 'aria-pressed': String(saved), 'aria-label': saved ? 'Remove from saved' : 'Save',
        onclick: () => toggleSaved(item.id)
      }, saved ? '♥' : '♡'));
  }

  function toggleSaved(id) {
    S.saved = S.saved.includes(id) ? S.saved.filter(x => x !== id) : [id, ...S.saved];
    writeIds(KEYS.saved, S.saved);
    if (S.view === 'saved') loadPage(true); else render();
  }

  // ── list ───────────────────────────────────────────────────────────────
  async function loadPage(reset) {
    if (S.loading && !reset) return;
    const token = ++S.token;
    if (reset) { S.items = []; S.total = 0; }
    S.loading = true;
    let page;
    if (S.view === 'all') {
      page = await api('list', { cat: S.category, q: S.query.trim(), sort: S.sort, offset: S.items.length, limit: PAGE });
    } else {
      const ids = S.view === 'saved' ? S.saved : S.recent;
      page = { total: ids.length, items: await itemsById(ids.slice(S.items.length, S.items.length + PAGE)) };
    }
    if (token !== S.token) return;
    S.loading = false;
    if (!page) { $('pt-empty').textContent = probe.includes('l') ? '' : 'Could not load the list.'; $('pt-empty').hidden = false; return; }
    page.items.forEach(remember);
    S.total = page.total;
    S.items = reset ? page.items : S.items.concat(page.items);
    render();
  }

  function render() {
    const titles = { all: S.category === 'All' ? 'All' : S.category, saved: 'Saved', recent: 'Recent' };
    $('pt-list-title').textContent = titles[S.view];
    $('pt-grid').replaceChildren(...S.items.map(makeCard));
    $('pt-empty').textContent = 'Nothing found.';
    $('pt-empty').hidden = S.items.length > 0 || S.loading;
    $('pt-more').hidden = S.items.length >= S.total;
    $('pt-featured').hidden = !(S.view === 'all' && S.category === 'All' && !S.query.trim() && S.summary?.featured?.length);
    document.querySelectorAll('.pt-tabs button').forEach(b => b.setAttribute('aria-selected', String(b.dataset.view === S.view)));
    $('pt-sort').closest('.pt-tools').classList.toggle('pt-nosort', S.view !== 'all');
    $('pt-sort').disabled = S.view !== 'all';
    renderChips();
  }

  function renderChips() {
    const names = ['All', ...(S.summary?.categories || []).map(c => c.name)];
    $('pt-chips').replaceChildren(...names.map(name => h('button', {
      type: 'button', class: 'pt-chip', 'aria-pressed': String(S.category === name),
      onclick: () => { S.view = 'all'; S.category = name; loadPage(true); }
    }, name)));
    $('pt-chips').hidden = S.view !== 'all';
  }

  async function loadSummary() {
    const summary = await api('summary');
    if (!summary) { $('pt-count').textContent = probe.includes('l') ? '' : 'The library is unavailable right now.'; return; }
    S.summary = summary;
    $('pt-count').textContent = `${summary.total.toLocaleString()} titles`;
    $('pt-featured-grid').replaceChildren(...summary.featured.map(item => makeCard(remember(item))));
    render();
  }

  // ── player and rewards ────────────────────────────────────────────────
  const dialog = $('pt-player'), frame = $('pt-frame');

  function play(item) {
    if (!canEmbed(item)) { window.open(item.url, '_blank', 'noopener'); return; }
    S.recent = [item.id, ...S.recent.filter(id => id !== item.id)].slice(0, 30);
    writeIds(KEYS.recent, S.recent);
    S.playing = item;
    $('pt-player-title').textContent = item.title;
    $('pt-newtab').href = item.url;
    frame.src = item.url;
    dialog.showModal();
    window.MitchPresence?.setActivity?.('Playing ' + item.title, item.url);
    startRewards();
  }

  dialog.addEventListener('close', () => {
    stopRewards(true);
    S.playing = null;
    frame.src = 'about:blank';
    window.MitchPresence?.setActivity?.(null);
    window._mitchSync?.saveIdb?.();
    if (S.view === 'recent') loadPage(true);
  });
  $('pt-back').addEventListener('click', () => dialog.close());
  $('pt-full').addEventListener('click', () => frame.requestFullscreen?.().catch(() => {}));

  const money = n => Number(n || 0).toLocaleString(undefined, { maximumFractionDigits: 2 });

  function showReward(data) {
    const line = $('pt-reward');
    if (!data || data.authenticated === false) { line.textContent = 'Sign in to earn MitchCoins'; return; }
    const earned = Number(data.dailyEarned || 0), cap = Number(data.dailyCap || 0);
    line.textContent = cap && earned >= cap ? 'Daily MitchCoins complete' : `+${money(data.rewardPerMinute)}/min · ${money(earned)}/${money(cap)} today`;
  }

  function toast(text) {
    const node = $('pt-toast');
    node.textContent = text;
    node.classList.add('show');
    clearTimeout(toast.timer);
    toast.timer = setTimeout(() => node.classList.remove('show'), 2400);
  }

  async function heartbeat(active) {
    try {
      const response = await fetch('/api/game-portal/heartbeat', {
        method: 'POST', credentials: 'include', cache: 'no-store', keepalive: !active,
        headers: { 'Content-Type': 'application/json', 'X-Mitch-Requested-With': '1' },
        body: JSON.stringify({ active, game: active && S.playing ? S.playing.title : '' })
      });
      const data = await response.json().catch(() => null);
      if (response.status === 401) showReward({ authenticated: false });
      else if (response.ok && data) { showReward(data); if (data.earned > 0) toast(`+${money(data.earned)} MitchCoins`); }
    } catch { $('pt-reward').textContent = 'Rewards reconnecting'; }
  }

  function startRewards() {
    stopRewards(false);
    heartbeat(document.visibilityState === 'visible');
    S.rewardTimer = setInterval(() => heartbeat(dialog.open && document.visibilityState === 'visible'), 20000);
  }
  function stopRewards(sendInactive) {
    clearInterval(S.rewardTimer);
    S.rewardTimer = null;
    if (sendInactive) heartbeat(false);
  }
  document.addEventListener('visibilitychange', () => { if (dialog.open) heartbeat(document.visibilityState === 'visible'); });
  window.addEventListener('pagehide', () => { if (dialog.open) stopRewards(true); });

  // ── controls ──────────────────────────────────────────────────────────
  let searchTimer = null;
  $('pt-q').addEventListener('input', () => {
    S.query = $('pt-q').value;
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => { S.view = 'all'; loadPage(true); }, 250);
  });
  $('pt-sort').addEventListener('change', () => { S.sort = $('pt-sort').value; loadPage(true); });
  $('pt-random').addEventListener('click', async () => { const data = await api('random'); if (data?.item) play(remember(data.item)); });
  document.querySelectorAll('.pt-tabs button').forEach(b => b.addEventListener('click', () => { S.view = b.dataset.view; S.category = 'All'; loadPage(true); }));
  $('pt-more').addEventListener('click', () => loadPage(false));
  document.addEventListener('keydown', event => {
    if (event.key === '/' && !dialog.open && !['INPUT', 'TEXTAREA', 'SELECT'].includes(document.activeElement?.tagName)) { event.preventDefault(); $('pt-q').focus(); }
  });

  // Each page of the list loads as the end of the grid scrolls into view.
  new IntersectionObserver(entries => {
    if (entries.some(e => e.isIntersecting) && !S.loading && S.items.length && S.items.length < S.total) loadPage(false);
  }, { rootMargin: '600px' }).observe($('pt-sentinel'));

  const params = new URLSearchParams(location.search);
  if (params.get('q')) { S.query = params.get('q'); $('pt-q').value = S.query; }
  fetch('/api/game-portal/status', { credentials: 'include', cache: 'no-store' })
    .then(r => (r.ok ? r.json() : { authenticated: false })).then(showReward).catch(() => {});
  loadSummary();
  loadPage(true);
})();
