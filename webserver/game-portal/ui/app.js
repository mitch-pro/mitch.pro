(() => {
  'use strict';

  const byId = id => document.getElementById(id);
  const state = {
    items: new Map(),
    list: [],
    total: 0,
    summary: null,
    loading: false,
    category: 'All',
    view: 'all',
    query: new URLSearchParams(location.search).get('q') || '',
    sort: 'popular',
    current: null,
    rewardTimer: null,
    authenticated: false,
    dailyEarned: 0,
    dailyCap: 240,
    rewardPerMinute: 2,
    favorites: readList('mitch.games.catalog.favorites'),
    recent: readList('mitch.games.catalog.recent')
  };
  const PAGE = 60;
  const player = byId('player');
  const frame = byId('player-frame');
  const search = byId('search');
  let previousFocus = null;

  function readList(key) {
    try {
      const list = JSON.parse(localStorage.getItem(key) || '[]');
      return Array.isArray(list) ? list.filter(value => Number.isInteger(value)) : [];
    } catch { return []; }
  }

  function saveList(key, list) {
    try { localStorage.setItem(key, JSON.stringify(list)); } catch {}
  }

  function canEmbed(game) {
    if (game.external) return false;
    try {
      const url = new URL(game.url, location.origin);
      return url.origin === location.origin && [
        '/prox/g/', '/prox/luma/', '/prox/calculated2/', '/prox/gamemonetize/',
        '/games/', '/game-portal/ui/open-source/'
      ].some(prefix => url.pathname.startsWith(prefix));
    } catch { return false; }
  }

  function icon(name) {
    const element = document.createElement('i');
    element.dataset.lucide = name;
    element.setAttribute('aria-hidden', 'true');
    return element;
  }

  function refreshIcons() {
    if (window.lucide?.createIcons) window.lucide.createIcons();
  }

  function imageFor(game) {
    const thumb = document.createElement('div');
    thumb.className = 'thumb';
    const fallback = document.createElement('span');
    fallback.className = 'thumb-fallback';
    fallback.textContent = game.title.slice(0, 2).toUpperCase();
    if (game.image) {
      const image = document.createElement('img');
      image.src = game.image;
      image.alt = '';
      image.loading = 'lazy';
      image.decoding = 'async';
      image.addEventListener('error', () => image.remove(), { once: true });
      thumb.append(image);
    }
    thumb.append(fallback);
    return thumb;
  }

  // Names are drawn on canvases (see canvas-text.js), not written into the page.
  function setTitle(element, text) {
    element.replaceChildren(window.MitchCanvasText.create(text, { block: true }));
  }

  function makeCard(game, compact = false) {
    const card = document.createElement('article');
    card.className = compact ? 'quick-card' : 'item-card';
    card.dataset.itemId = game.id;
    const info = document.createElement('div');
    info.className = 'card-info';
    const title = document.createElement('div');
    title.className = 'card-title';
    setTitle(title, game.title);
    const category = document.createElement('small');
    category.textContent = game.category + (canEmbed(game) ? '' : ' · Opens in new tab');
    info.append(title, category);
    const open = document.createElement('button');
    open.className = 'card-open';
    open.type = 'button';
    open.setAttribute('aria-label', canEmbed(game) ? 'Play' : 'Play (opens in new tab)');
    open.addEventListener('click', () => launch(game));
    const favorite = document.createElement('button');
    favorite.className = 'favorite-button' + (state.favorites.includes(game.id) ? ' is-saved' : '');
    favorite.type = 'button';
    favorite.setAttribute('aria-label', state.favorites.includes(game.id) ? 'Remove from saved' : 'Save');
    favorite.append(icon('heart'));
    favorite.addEventListener('click', () => toggleFavorite(game.id));
    card.append(imageFor(game), info, open, favorite);
    return card;
  }

  function toggleFavorite(id) {
    state.favorites = state.favorites.includes(id) ? state.favorites.filter(item => item !== id) : [id, ...state.favorites];
    saveList('mitch.games.catalog.favorites', state.favorites);
    if (state.view === 'favorites') loadList(true); else render();
  }

  function launch(game) {
    if (!game?.url) return;
    if (!canEmbed(game)) {
      window.open(game.url, '_blank', 'noopener');
      return;
    }
    previousFocus = document.activeElement;
    state.recent = [game.id, ...state.recent.filter(id => id !== game.id)].slice(0, 30);
    saveList('mitch.games.catalog.recent', state.recent);
    setTitle(byId('player-title'), game.title);
    byId('player-new-tab').href = game.url;
    byId('open-source-credit').hidden = !game.openSource;
    frame.src = game.url;
    frame.title = 'Player';
    player.hidden = false;
    document.body.style.overflow = 'hidden';
    byId('player-back').focus();
    state.current = game;
    window.MitchPresence?.setActivity("Playing " + game.title, game.url);
    startRewardLoop();
  }

  function closePlayer() {
    if (player.hidden) return;
    stopRewardLoop(true);
    state.current = null;
    window.MitchPresence?.setActivity(null);
    player.hidden = true;
    frame.src = 'about:blank';
    window._mitchSync?.saveIdb();
    document.body.style.overflow = '';
    if (document.fullscreenElement) document.exitFullscreen().catch(() => {});
    render();
    (previousFocus?.isConnected ? previousFocus : search).focus();
  }

  async function randomGame() {
    const button = byId('shuffle');
    if (button.disabled) return;
    button.disabled = true;
    const label = button.textContent;
    button.textContent = 'Picking…';
    const data = await api('random');
    button.disabled = false;
    button.textContent = label;
    if (data?.item) { remember(data.item); launch(data.item); }
    else showToast('Could not pick a title. Try again.');
  }

  function formatCoins(value) {
    const num = Number(value || 0);
    if (!Number.isFinite(num)) return '∞';
    if (num >= 1e15) return num.toExponential(2).replace('e+', 'e');
    if (num >= 10000) return new Intl.NumberFormat(undefined, { notation: 'compact', maximumFractionDigits: 1 }).format(num);
    return num.toLocaleString(undefined, { maximumFractionDigits: 2 });
  }

  function updateRewardUi(data) {
    if (!data) return;
    if (data.authenticated === false) {
      state.authenticated = false;
      byId('wallet-balance').textContent = 'Sign in';
      byId('wallet-link').href = '/enroll/?next=' + encodeURIComponent('/game-portal/');
      byId('wallet-link').setAttribute('aria-label', 'Sign in to earn MitchCoins');
      byId('reward-title').textContent = 'Sign in to earn';
      byId('reward-detail').textContent = 'Play to collect MitchCoins';
      byId('reward-progress').style.width = '0%';
      byId('player-reward').textContent = 'Sign in to earn MitchCoins';
      return;
    }
    state.authenticated = true;
    state.dailyEarned = Number(data.dailyEarned || 0);
    state.dailyCap = Number(data.dailyCap || 240);
    state.rewardPerMinute = Number(data.rewardPerMinute || 2);
    byId('wallet-balance').textContent = formatCoins(data.coins);
    byId('wallet-link').href = '/coins/?v=2';
    byId('wallet-link').setAttribute('aria-label', `${formatCoins(data.coins)} MitchCoins. Open wallet`);
    byId('reward-title').textContent = state.dailyEarned >= state.dailyCap
      ? 'Daily rewards complete'
      : `Earn ${state.rewardPerMinute} per minute`;
    byId('reward-detail').textContent = `${formatCoins(state.dailyEarned)} of ${formatCoins(state.dailyCap)} earned today`;
    byId('reward-progress').style.width = `${Math.min(100, state.dailyEarned / state.dailyCap * 100)}%`;
    byId('player-reward').textContent = state.dailyEarned >= state.dailyCap
      ? 'Daily MitchCoins complete'
      : `+${state.rewardPerMinute}/min · ${formatCoins(state.dailyEarned)}/${formatCoins(state.dailyCap)} today`;
  }

  let toastTimer = null;
  function showToast(text) {
    const toast = byId('coin-toast');
    toast.textContent = text;
    toast.classList.add('is-visible');
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => toast.classList.remove('is-visible'), 3500);
  }

  function showCoinToast(amount) {
    const toast = byId('coin-toast');
    toast.textContent = `+${formatCoins(amount)} MitchCoins`;
    toast.classList.add('is-visible');
    clearTimeout(toast._timer);
    toast._timer = setTimeout(() => toast.classList.remove('is-visible'), 2400);
  }

  async function heartbeat(active) {
    try {
      const response = await fetch('/api/game-portal/heartbeat', {
        method: 'POST',
        credentials: 'include',
        cache: 'no-store',
        keepalive: !active,
        headers: { 'Content-Type': 'application/json', 'X-Mitch-Requested-With': '1' },
        body: JSON.stringify({ active, game: active && state.current ? state.current.title : '' })
      });
      const data = await response.json().catch(() => null);
      if (response.status === 401) updateRewardUi({ authenticated: false });
      else if (response.ok && data) {
        updateRewardUi(data);
        if (data.earned > 0) showCoinToast(data.earned);
      }
    } catch {
      byId('player-reward').textContent = 'Rewards reconnecting';
    }
  }

  function startRewardLoop() {
    stopRewardLoop(false);
    heartbeat(document.visibilityState === 'visible');
    state.rewardTimer = setInterval(() => {
      heartbeat(!player.hidden && document.visibilityState === 'visible');
    }, 20_000);
  }

  function stopRewardLoop(sendInactive) {
    if (state.rewardTimer) clearInterval(state.rewardTimer);
    state.rewardTimer = null;
    if (sendInactive) heartbeat(false);
  }

  // The page never gets the whole catalog: it asks the server for what is on
  // screen (featured row, one page of the list, saved or recent items by id).
  async function api(name, params = {}) {
    const query = new URLSearchParams(params).toString();
    try {
      const response = await fetch(`/api/portal/${name}${query ? '?' + query : ''}`, { credentials: 'same-origin' });
      return response.ok ? await response.json() : null;
    } catch { return null; }
  }

  // Fills the shared empty-state box: 'error' offers Retry, anything else
  // describes why the list is empty and offers a way out.
  let retryAction = null;
  function showEmpty(kind) {
    const box = byId('empty-state');
    const h = box.querySelector('h3');
    const p = box.querySelector('p');
    const btn = byId('clear-filters');
    btn.hidden = false;
    if (kind === 'error') {
      h.textContent = 'Could not load titles';
      p.textContent = 'Check your connection and try again.';
      btn.textContent = 'Retry';
      retryAction = () => { loadSummary(); loadList(true); };
    } else if (state.view === 'favorites') {
      h.textContent = 'Nothing saved yet';
      p.textContent = 'Use the heart on a title to save it here.';
      btn.textContent = 'Browse all titles';
      retryAction = null;
    } else if (state.view === 'recent') {
      h.textContent = 'Nothing played yet';
      p.textContent = 'Titles you play will show up here.';
      btn.textContent = 'Browse all titles';
      retryAction = null;
    } else {
      h.textContent = 'Nothing found';
      p.textContent = state.query.trim() ? `No titles match "${state.query.trim()}".` : 'No titles in this category.';
      btn.textContent = 'Clear filters';
      retryAction = null;
    }
    box.hidden = false;
    byId('item-grid').replaceChildren();
    byId('load-more').hidden = true;
    byId('result-count').textContent = '';
  }

  function remember(item) { state.items.set(item.id, item); return item; }

  async function itemsById(ids) {
    const missing = ids.filter(id => !state.items.has(id));
    if (missing.length) {
      const data = await api('items', { ids: missing.slice(0, 60).join(',') });
      (data?.items || []).forEach(remember);
    }
    return ids.map(id => state.items.get(id)).filter(Boolean);
  }

  function renderFeatured(items) {
    byId('quick-grid').replaceChildren(...items.map(game => makeCard(remember(game), true)));
    refreshIcons();
  }

  let listToken = 0;
  // Loads the first page (reset) or the next one of the current view.
  async function loadList(reset) {
    const token = ++listToken;
    const grid = byId('item-grid');
    const more = byId('load-more');
    if (reset) {
      state.list = []; state.total = 0;
      byId('empty-state').hidden = true;
      grid.setAttribute('aria-busy', 'true');
      grid.classList.add('is-loading');
      byId('result-count').textContent = 'Loading…';
      more.hidden = true;
    } else {
      more.disabled = true;
      more.textContent = 'Loading…';
    }
    state.loading = true;
    let page = null;
    if (state.view === 'all') {
      page = await api('list', { cat: state.category, q: state.query.trim(), sort: state.sort, offset: state.list.length, limit: PAGE });
    } else {
      const ids = state.view === 'favorites' ? state.favorites : state.recent;
      const items = await itemsById(ids.slice(state.list.length, state.list.length + PAGE));
      page = { total: ids.length, items };
    }
    if (token !== listToken) return;
    state.loading = false;
    grid.removeAttribute('aria-busy');
    grid.classList.remove('is-loading');
    more.disabled = false;
    more.textContent = 'Show more';
    if (!page) {
      if (reset) { renderFilters(); showEmpty('error'); }
      else { more.textContent = 'Could not load more. Try again'; }
      return;
    }
    state.total = page.total;
    page.items.forEach(remember);
    state.list = reset ? page.items : state.list.concat(page.items);
    render();
  }

  function renderFilters() {
    const categories = state.summary?.categories || [];
    byId('filters').replaceChildren(...[{ name: 'All' }, ...categories].map(({ name }) => {
      const button = document.createElement('button');
      button.className = 'filter-button' + (state.category === name ? ' is-active' : '');
      button.type = 'button';
      button.textContent = name;
      button.setAttribute('aria-pressed', String(state.category === name));
      button.addEventListener('click', () => { state.category = name; loadList(true); });
      return button;
    }));
    byId('side-categories').replaceChildren(...categories.map(({ name, count }) => {
      const button = document.createElement('button');
      button.className = 'side-category';
      button.type = 'button';
      const label = document.createElement('span');
      label.textContent = name;
      const total = document.createElement('small');
      total.className = 'side-category-count';
      total.textContent = count.toLocaleString();
      button.append(label, total);
      button.setAttribute('aria-pressed', String(state.category === name));
      button.addEventListener('click', () => { state.view = 'all'; state.category = name; closeMenu(); loadList(true); });
      return button;
    }));
    document.querySelectorAll('[data-view]').forEach(button => button.setAttribute('aria-pressed', String(button.dataset.view === state.view)));
  }

  async function renderSidebarRecent() {
    const recentGames = await itemsById(state.recent.slice(0, 2));
    byId('sidebar-recent').hidden = recentGames.length === 0;
    byId('sidebar-recent-list').replaceChildren(...recentGames.map(game => {
      const button = document.createElement('button');
      button.type = 'button';
      button.className = 'sidebar-recent-game';
      button.setAttribute('aria-label', 'Play again');
      const art = document.createElement('span');
      art.className = 'sidebar-recent-art';
      art.textContent = game.title.slice(0, 1);
      if (game.image) {
        const image = document.createElement('img');
        image.src = game.image;
        image.alt = '';
        image.loading = 'lazy';
        image.addEventListener('error', () => image.remove(), { once: true });
        art.append(image);
      }
      const title = document.createElement('span');
      title.className = 'sidebar-recent-title';
      setTitle(title, game.title);
      button.append(art, title);
      button.addEventListener('click', () => { closeMenu(); launch(game); });
      return button;
    }));
  }

  function render() {
    const viewName = state.view === 'favorites' ? 'Saved' : state.view === 'recent' ? 'Recently played' : (state.category === 'All' ? 'Entertainment' : state.category);
    byId('current-view-name').textContent = viewName;
    byId('page-title').textContent = state.view === 'favorites' ? 'Saved' : state.view === 'recent' ? 'Recently played' : 'Entertainment Portal';
    byId('quick-picks-section').hidden = !!state.query || state.view !== 'all' || state.category !== 'All';
    byId('sort-control').hidden = state.view !== 'all';
    byId('catalog-heading').textContent = state.view === 'favorites' ? 'Saved' : (state.view === 'recent' ? 'Recently played' : 'All titles');
    byId('result-count').textContent = `${state.total.toLocaleString()} ${state.total === 1 ? 'title' : 'titles'}`;
    byId('item-grid').replaceChildren(...state.list.map(game => makeCard(game)));
    if (state.total === 0 && !state.loading) showEmpty(); else byId('empty-state').hidden = true;
    byId('load-more').hidden = state.list.length >= state.total;
    renderFilters();
    renderSidebarRecent();
    refreshIcons();
  }

  async function loadSummary() {
    const summary = await api('summary');
    if (!summary) {
      byId('library-count').textContent = 'Library unavailable';
      return;
    }
    state.summary = summary;
    byId('library-count').textContent = `${summary.total.toLocaleString()} titles in the library`;
    renderFeatured(summary.featured);
    heartbeat(false);
  }

  search.value = state.query;
  byId('sort-items').addEventListener('change', event => { state.sort = event.target.value; loadList(true); });
  let searchTimer = null;
  search.addEventListener('input', () => { state.query = search.value; clearTimeout(searchTimer); searchTimer = setTimeout(() => loadList(true), 250); });
  function closeMenu() {
    byId('sidebar').classList.remove('is-open');
    byId('menu-scrim').hidden = true;
    byId('mobile-menu').setAttribute('aria-expanded', 'false');
  }
  byId('mobile-menu').addEventListener('click', () => {
    const open = !byId('sidebar').classList.contains('is-open');
    byId('sidebar').classList.toggle('is-open', open);
    byId('menu-scrim').hidden = !open;
    byId('mobile-menu').setAttribute('aria-expanded', String(open));
  });
  byId('menu-scrim').addEventListener('click', closeMenu);
  document.querySelectorAll('[data-view]').forEach(button => button.addEventListener('click', () => { state.view = button.dataset.view; state.category = 'All'; closeMenu(); loadList(true); }));
  byId('clear-filters').addEventListener('click', () => {
    if (retryAction) { retryAction(); return; }
    state.category = 'All'; state.view = 'all'; state.query = ''; search.value = ''; loadList(true);
  });
  byId('load-more').addEventListener('click', () => loadList(false));
  byId('shuffle').addEventListener('click', randomGame);
  byId('player-back').addEventListener('click', () => closePlayer());
  byId('player-fullscreen').addEventListener('click', () => frame.requestFullscreen?.().catch(() => {}));
  document.addEventListener('visibilitychange', () => { if (!player.hidden) heartbeat(document.visibilityState === 'visible'); });
  window.addEventListener('pagehide', () => { if (!player.hidden) stopRewardLoop(true); });
  document.addEventListener('keydown', event => {
    if (event.key === 'Escape' && !player.hidden) closePlayer();
    else if (event.key === 'Escape') closeMenu();
    if (event.key === '/' && player.hidden && !['INPUT', 'TEXTAREA'].includes(document.activeElement?.tagName)) { event.preventDefault(); search.focus(); }
  });
  refreshIcons();
  fetch('/api/game-portal/status', { credentials: 'include', cache: 'no-store' })
    .then(response => response.ok ? response.json() : { authenticated: false })
    .then(updateRewardUi)
    .catch(() => { byId('player-reward').textContent = 'Rewards temporarily unavailable'; });
  loadSummary();
  loadList(true);
})();
