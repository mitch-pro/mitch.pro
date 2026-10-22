(() => {
  'use strict';

  const byId = id => document.getElementById(id);
  const state = {
    games: [],
    category: 'All',
    view: 'all',
    query: new URLSearchParams(location.search).get('q') || '',
    sort: 'popular',
    limit: 60,
    current: null,
    rewardTimer: null,
    authenticated: false,
    dailyEarned: 0,
    dailyCap: 240,
    rewardPerMinute: 2,
    favorites: readList('mitch.games.catalog.favorites'),
    recent: readList('mitch.games.catalog.recent')
  };
  const discovery = window.MitchGameDiscovery;
  const categories = ['All', 'Arcade', 'Action', 'Racing', 'Sports', 'Multiplayer', 'Puzzle', 'Retro', 'Casual', 'Horror'];
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

  function plainText(value) {
    const raw = String(value || '');
    if (!/[<&]/.test(raw)) return raw.replace(/\s+/g, ' ').trim();
    const node = new DOMParser().parseFromString(raw, 'text/html');
    return (node.body.textContent || '').replace(/\s+/g, ' ').trim();
  }

  function imageUrl(value) {
    const raw = String(value || '');
    if (raw.startsWith('/img/games/')) return '/game-portal/icons/' + raw.slice('/img/games/'.length);
    if (raw.startsWith('/img/gamems/')) return '/game-portal/icons/' + raw.slice('/img/gamems/'.length);
    if (raw.startsWith('/game-portal/icons/')) return raw;
    if (raw.startsWith('https://img.gamemonetize.com/')) return '/proxy/gm-icon/' + raw.slice('https://img.gamemonetize.com/'.length);
    if (raw.startsWith('https://lumassets.pages.dev/')) return '/proxy/luma/' + raw.slice('https://lumassets.pages.dev/'.length);
    if (/^https:\/\//i.test(raw)) return raw;
    return '';
  }

  function gameUrl(value) {
    const raw = String(value || '');
    if (raw.startsWith('/games/')) return raw;
    if (raw.startsWith('/proxy/')) return raw;
    if (raw.startsWith('/')) return '/proxy/calculated2' + raw;
    try {
      const url = new URL(raw);
      if (url.protocol !== 'https:') return '';
      if (url.hostname === 'lumassets.pages.dev') return '/proxy/luma' + url.pathname + url.search + url.hash;
      if (url.hostname === 'calculated2.github.io') return '/proxy/calculated2' + url.pathname + url.search + url.hash;
      if (['html5.gamemonetize.co', 'html5.gamemonetize.com'].includes(url.hostname)) return '/proxy/gamemonetize' + url.pathname + url.search + url.hash;
      return url.href;
    } catch { return ''; }
  }

  function canEmbed(game) {
    if (game.external) return false;
    try {
      const url = new URL(game.url, location.origin);
      return url.origin === location.origin && [
        '/proxy/luma/', '/proxy/calculated2/', '/proxy/gamemonetize/',
        '/games/', '/game-portal/ui/open-source/'
      ].some(prefix => url.pathname.startsWith(prefix));
    } catch { return false; }
  }

  function categoryFor(title, description, url) {
    const value = (title + ' ' + description + ' ' + url).toLowerCase();
    if (/fnaf|backrooms|silent hill|dreader|horror/.test(value)) return 'Horror';
    if (/multiplayer|1v1|basket bros|shell shock|smash karts|tetr\.io/.test(value)) return 'Multiplayer';
    if (/race|racing|moto|drift|traffic|car|highway|swerve/.test(value)) return 'Racing';
    if (/basket|football|soccer|golf|bowl|skate|punch|pool party/.test(value)) return 'Sports';
    if (/\/retro\/|pokemon|mario|sonic|zelda|metroid|kirby|donkey kong|final fantasy|star fox|mega ?man|tetris/.test(value)) return 'Retro';
    if (/puzzle|riddle|wordle|2048|bloxorz|alchemy|sort|quiz|calculator|logic/.test(value)) return 'Puzzle';
    if (/shooter|doom|quake|gun|combat|commando|battle|fight|hobo|shark|tank|action/.test(value)) return 'Action';
    if (/papa|idle|learn to fly|duck life|buddy|sandbox|tycoon/.test(value)) return 'Casual';
    return 'Arcade';
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

  function makeCard(game, compact = false) {
    const card = document.createElement('article');
    card.className = compact ? 'quick-card' : 'game-card';
    card.dataset.gameId = game.id;
    const info = document.createElement('div');
    info.className = 'card-info';
    const title = document.createElement('strong');
    title.textContent = game.title;
    const category = document.createElement('small');
    category.textContent = game.category + (canEmbed(game) ? '' : ' · Opens in new tab');
    info.append(title, category);
    const open = document.createElement('button');
    open.className = 'card-open';
    open.type = 'button';
    open.setAttribute('aria-label', 'Play ' + game.title + (canEmbed(game) ? '' : ' (opens in new tab)'));
    open.addEventListener('click', () => launch(game));
    const favorite = document.createElement('button');
    favorite.className = 'favorite-button' + (state.favorites.includes(game.id) ? ' is-saved' : '');
    favorite.type = 'button';
    favorite.setAttribute('aria-label', `${state.favorites.includes(game.id) ? 'Remove' : 'Save'} ${game.title}`);
    favorite.append(icon('heart'));
    favorite.addEventListener('click', () => toggleFavorite(game.id));
    card.append(imageFor(game), info, open, favorite);
    return card;
  }

  function toggleFavorite(id) {
    state.favorites = state.favorites.includes(id) ? state.favorites.filter(item => item !== id) : [id, ...state.favorites];
    saveList('mitch.games.catalog.favorites', state.favorites);
    renderFeatured();
    render();
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
    byId('player-title').textContent = game.title;
    byId('player-new-tab').href = game.url;
    byId('open-source-credit').hidden = !game.openSource;
    frame.src = game.url;
    frame.title = game.title;
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
    document.body.style.overflow = '';
    if (document.fullscreenElement) document.exitFullscreen().catch(() => {});
    render();
    (previousFocus?.isConnected ? previousFocus : search).focus();
  }

  function randomGame() {
    if (!state.games.length) return;
    launch(state.games[Math.floor(Math.random() * state.games.length)]);
  }

  function formatCoins(value) {
    return Number(value || 0).toLocaleString(undefined, { maximumFractionDigits: 2 });
  }

  function updateRewardUi(data) {
    if (!data) return;
    if (data.authenticated === false) {
      state.authenticated = false;
      byId('wallet-balance').textContent = 'Sign in';
      byId('wallet-link').href = '/enroll/?next=' + encodeURIComponent('/game-portal/');
      byId('wallet-link').setAttribute('aria-label', 'Sign in to earn MitchCoins');
      byId('reward-title').textContent = 'Sign in to earn';
      byId('reward-detail').textContent = 'Play games to collect MitchCoins';
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

  function renderFeatured() {
    const seen = new Set();
    const picks = [...state.games].sort(discovery.compare).filter(game => {
      const key = discovery.key(game.title);
      if (discovery.rank(game.title) === Number.MAX_SAFE_INTEGER || seen.has(key)) return false;
      seen.add(key); return true;
    }).slice(0, 12);
    byId('quick-grid').replaceChildren(...picks.map(game => makeCard(game, true)));
  }

  function filteredGames() {
    const query = state.query.trim().toLowerCase();
    const games = state.games.filter(game => {
      if (state.view === 'favorites' && !state.favorites.includes(game.id)) return false;
      if (state.view === 'recent' && !state.recent.includes(game.id)) return false;
      if (state.category !== 'All' && game.category !== state.category) return false;
      const titleQuery = discovery.key(query);
      return !query || game.search.includes(query) || (titleQuery && discovery.key(game.title).includes(titleQuery));
    });
    if (state.view === 'recent') games.sort((a, b) => state.recent.indexOf(a.id) - state.recent.indexOf(b.id));
    else if (state.view === 'favorites') games.sort((a, b) => state.favorites.indexOf(a.id) - state.favorites.indexOf(b.id));
    else games.sort(state.sort === 'alphabetical'
      ? (a, b) => a.title.localeCompare(b.title)
      : discovery.compare);
    return games;
  }

  function renderFilters() {
    const available = categories.filter(name => name === 'All' || state.games.some(game => game.category === name));
    const counts = new Map();
    for (const game of state.games) counts.set(game.category, (counts.get(game.category) || 0) + 1);
    byId('filters').replaceChildren(...available.map(name => {
      const button = document.createElement('button');
      button.className = 'filter-button' + (state.category === name ? ' is-active' : '');
      button.type = 'button';
      button.textContent = name;
      button.setAttribute('aria-pressed', String(state.category === name));
      button.addEventListener('click', () => { state.category = name; state.limit = 60; render(); });
      return button;
    }));
    byId('side-categories').replaceChildren(...available.filter(name => name !== 'All').map(name => {
      const button = document.createElement('button');
      button.className = 'side-category';
      button.type = 'button';
      const label = document.createElement('span');
      label.textContent = name;
      const count = document.createElement('small');
      count.className = 'side-category-count';
      count.textContent = counts.get(name).toLocaleString();
      button.append(label, count);
      button.setAttribute('aria-pressed', String(state.category === name));
      button.addEventListener('click', () => { state.view = 'all'; state.category = name; state.limit = 60; closeMenu(); render(); });
      return button;
    }));
    document.querySelectorAll('[data-view]').forEach(button => button.setAttribute('aria-pressed', String(button.dataset.view === state.view)));
  }

  function renderSidebarRecent() {
    const recentGames = state.recent.map(id => state.games.find(game => game.id === id)).filter(Boolean).slice(0, 2);
    byId('sidebar-recent').hidden = recentGames.length === 0;
    byId('sidebar-recent-list').replaceChildren(...recentGames.map(game => {
      const button = document.createElement('button');
      button.type = 'button';
      button.className = 'sidebar-recent-game';
      button.setAttribute('aria-label', `Play ${game.title} again`);
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
      title.textContent = game.title;
      button.append(art, title);
      button.addEventListener('click', () => { closeMenu(); launch(game); });
      return button;
    }));
  }

  function render() {
    const games = filteredGames();
    const viewName = state.view === 'favorites' ? 'Saved' : state.view === 'recent' ? 'Recently played' : (state.category === 'All' ? 'Games' : state.category);
    byId('current-view-name').textContent = viewName;
    byId('page-title').textContent = state.view === 'favorites' ? 'Saved games' : state.view === 'recent' ? 'Recently played' : 'Games';
    byId('quick-picks-section').hidden = !!state.query || state.view !== 'all' || state.category !== 'All';
    byId('sort-control').hidden = state.view !== 'all';
    byId('catalog-heading').textContent = state.view === 'favorites' ? 'Saved games' : (state.view === 'recent' ? 'Recently played' : 'All games');
    byId('result-count').textContent = `${games.length.toLocaleString()} ${games.length === 1 ? 'game' : 'games'}`;
    byId('game-grid').replaceChildren(...games.slice(0, state.limit).map(game => makeCard(game)));
    byId('empty-state').hidden = games.length > 0;
    byId('load-more').hidden = games.length <= state.limit;
    renderFilters();
    renderSidebarRecent();
    refreshIcons();
  }

  async function loadCatalog() {
    try {
      const response = await fetch('/game-portal/games.json');
      if (!response.ok) throw new Error('Could not load the game catalog');
      const catalog = await response.json();
      let id = 0;
      const games = [];
      for (const section of catalog.links || []) {
        for (const entry of section.games || []) {
          const title = plainText(entry[0]);
          const description = plainText(entry[3]);
          const url = gameUrl(entry[2]);
          if (!title || !url) continue;
          const openSource = title.toLowerCase() === '2048';
          games.push({
            id: id++,
            title,
            category: categoryFor(title, description, url),
            image: imageUrl(entry[1]),
            url: openSource ? '/game-portal/ui/open-source/2048/' : url,
            openSource,
            external: entry[4] === 'IgnoreIframe',
            search: (title + ' ' + description + ' ' + section.title).toLowerCase()
          });
        }
      }
      const artwork = new Map();
      for (const game of games) if (game.image) artwork.set(discovery.key(game.title), game.image);
      for (const game of games) if (!game.image) game.image = artwork.get(discovery.key(game.title)) || '';
      state.games = games;
      byId('library-count').textContent = `${games.length.toLocaleString()} games in the library`;
      renderFeatured();
      render();
      heartbeat(false);
    } catch (error) {
      byId('library-count').textContent = 'Library unavailable';
      byId('empty-state').hidden = false;
      byId('empty-state').querySelector('p').textContent = error.message || 'Could not load games.';
    }
  }

  search.value = state.query;
  byId('sort-games').addEventListener('change', event => { state.sort = event.target.value; state.limit = 60; render(); });
  search.addEventListener('input', () => { state.query = search.value; state.limit = 60; render(); });
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
  document.querySelectorAll('[data-view]').forEach(button => button.addEventListener('click', () => { state.view = button.dataset.view; state.category = 'All'; state.limit = 60; closeMenu(); render(); }));
  byId('clear-filters').addEventListener('click', () => { state.category = 'All'; state.view = 'all'; state.query = ''; search.value = ''; render(); });
  byId('load-more').addEventListener('click', () => { state.limit += 60; render(); });
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
  loadCatalog();
})();
