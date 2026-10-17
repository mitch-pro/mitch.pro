(() => {
  'use strict';
  const metrics = [
    ['coins', 'MitchCoins'], ['wins', 'Chess wins'], ['puzzles', 'Puzzles'],
    ['pixels', 'Pixels'], ['clicker_pts', 'Clicker'],
    ['typing_races', 'Typing races'], ['logic_puzzles', 'Logic puzzles'],
  ];
  // Match the profile page's shop cosmetics, including its existing colors.
  const colors = {
    neon_purple: 'Neon Purple Name', electric_blue: 'Electric Blue Name', mint_flash: 'Mint Flash Name',
    rose_spark: 'Rose Spark Name', ember_red: 'Ember Red Name', void_white: 'Void White Name',
    gold_glow: 'Golden Glow Name', rainbow_name: 'Rainbow Name',
  };
  const effects = {
    profile_grid: 'Profile Grid Background', profile_stars: 'Profile Starfield',
    profile_scanlines: 'Profile Scanlines', profile_gold_frame: 'Gold Profile Frame', profile_neon_frame: 'Neon Profile Frame',
  };
  const badges = {
    verified_badge: ['✔', 'Verified Badge'], og_badge: ['📜', 'OG Badge'], artist_badge: ['🎨', 'Artist Badge'],
    chess_badge: ['♟', 'Chess Badge'], builder_badge: ['🛠', 'Builder Badge'], lucky_badge: ['🍀', 'Lucky Badge'],
    premium_star_badge: ['⭐', 'Premium Star Badge'], owner_fan_badge: ['👑', 'Mitch Fan Badge'],
  };
  for (const catalog of [colors, effects, badges]) Object.setPrototypeOf(catalog, null);
  const $ = id => document.getElementById(id);
  const esc = value => String(value ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);
  const number = value => Number.isFinite(Number(value)) ? Number(value) : 0;
  const exact = value => number(value).toLocaleString(undefined, { maximumFractionDigits: 2 });
  const compact = value => Intl.NumberFormat(undefined, { notation: Math.abs(number(value)) >= 1e15 ? 'scientific' : 'compact', maximumFractionDigits: 1 }).format(number(value));
  const initials = name => Array.from(String(name || '?').trim())[0]?.toUpperCase() || '?';
  const label = metric => metrics.find(([key]) => key === metric)?.[1] || 'MitchCoins';
  const profileLink = user => '/profile/?u=' + encodeURIComponent(user.profileId || user.email || user.handle || user.name);
  let activeMetric = 'coins', search = '', data = null, request = null, sequence = 0, debounce;

  function imageSource(value, allowData = false) {
    if (typeof value !== 'string' || /[\u0000-\u001f\u007f]/.test(value)) return '';
    if (allowData && /^data:image\/(png|jpe?g|gif|webp|avif);base64,[a-z0-9+/=\s]+$/i.test(value)) return value;
    try {
      const url = new URL(value);
      return ['https:', 'http:'].includes(url.protocol) && !url.username && !url.password ? url.href : '';
    } catch { return ''; }
  }
  function avatar(user) {
    const source = imageSource(user.pfp, true);
    return `<span class="player-avatar"><span aria-hidden="true">${esc(initials(user.name))}</span>${source ? `<img src="${esc(source)}" alt="" loading="lazy" decoding="async" referrerpolicy="no-referrer">` : ''}</span>`;
  }
  function cover(user) {
    const source = imageSource(user.background);
    return source ? `<img class="player-cover" src="${esc(source)}" alt="" loading="lazy" decoding="async" referrerpolicy="no-referrer">` : '';
  }
  function surface(user) {
    return `data-effect="${effects[user.activeProfileEffect] ? user.activeProfileEffect : ''}"`;
  }
  function name(user) {
    const badge = badges[user.activeBadge || user.badge];
    return `<span class="player-name-line"><span class="player-name" data-color="${colors[user.activeColor] ? user.activeColor : ''}">${esc(user.name || 'Player')}</span>${badge ? `<span class="player-badge" title="${badge[1]}" aria-label="${badge[1]}">${badge[0]}</span>` : ''}${user.isPremium ? '<span class="player-premium" title="Premium member" aria-label="Premium member">✦</span>' : ''}${user.isMe ? '<span class="player-you">You</span>' : ''}</span>`;
  }
  function handle(user) {
    return user.handle ? '@' + user.handle : '';
  }
  function loadout(user) {
    const equipped = [
      colors[user.activeColor] && ['◈', colors[user.activeColor]],
      badges[user.activeBadge || user.badge],
      effects[user.activeProfileEffect] && ['✧', effects[user.activeProfileEffect]],
    ].filter(Boolean);
    return equipped.length ? `<span class="player-loadout" aria-label="Equipped cosmetics">${equipped.map(([icon, title]) => `<span class="loadout-item" title="${title}"><span aria-hidden="true">${icon}</span><span>${esc(title.replace(/ Name$/, '').replace(/^Profile /, '').replace(/ Profile Frame$/, ' frame').replace(/ Badge$/, ''))}</span></span>`).join('')}</span>` : '<span class="loadout-default" aria-label="No equipped items">—</span>';
  }
  function renderMine() {
    const user = data.me;
    $('my-rank').hidden = !user && !data.isTester;
    if (data.isTester) {
      $('my-rank').innerHTML = '<span class="personal-label">Your rank</span><span class="personal-unranked">Test accounts are excluded from rankings.</span>';
    } else if (!user) {
      $('my-rank').innerHTML = '';
    } else {
      $('my-rank').innerHTML = `<span class="personal-label">Your rank</span><a href="${profileLink(user)}" class="personal-identity">${avatar(user)}<span class="personal-copy">${name(user)}</span></a><div class="personal-numbers"><div><strong>#${exact(user.rank)}</strong><span>of ${exact(data.total)}</span></div><div><strong title="${exact(user[data.metric])}">${compact(user[data.metric])}</strong><span>${esc(label(data.metric))}</span></div></div><a class="personal-link" href="${profileLink(user)}">Profile</a>`;
    }
  }
  function renderRows() {
    const secondary = metrics.filter(([key]) => key !== data.metric).slice(0, 2);
    const columns = [[data.metric, label(data.metric)], ...secondary];
    $('rankings-title').textContent = label(data.metric) + ' rankings';
    $('standings-head').innerHTML = `<span>Rank</span><span>Player</span><span>Equipped</span>${columns.map(([, title], index) => `<span class="${index === 0 ? 'selected-column' : ''}">${esc(title)}</span>`).join('')}`;
    $('rank-list').innerHTML = (data.players || []).map(user => `
      <li class="standing-row profile-surface ${user.isMe ? 'standing-me' : ''}" ${surface(user)} value="${number(user.rank)}">
        ${cover(user)}<span class="standing-place ${user.rank <= 3 ? 'standing-top' : ''}" aria-label="Rank ${number(user.rank)}">${number(user.rank)}</span>
        <a class="standing-identity" href="${profileLink(user)}">${avatar(user)}<span class="standing-person">${name(user)}<span class="player-handle">${esc(handle(user))}</span></span></a>
        <div class="standing-style">${loadout(user)}</div>
        ${columns.map(([key, title], index) => `<div class="standing-stat ${index === 0 ? 'selected-column' : 'secondary-stat'}"><span class="mobile-stat-label">${esc(title)}</span><strong title="${exact(user[key])}">${compact(user[key])}</strong></div>`).join('')}
      </li>`).join('') || `<li class="board-empty"><p>${search ? 'No players found.' : 'No rankings yet.'}</p>${search ? '<button type="button" id="clear-search">Clear search</button>' : ''}</li>`;
    const shown = (data.players || []).length;
    const total = number(search ? data.matchCount : data.total);
    $('rank-count').textContent = search ? `${exact(total)} ${total === 1 ? 'match' : 'matches'}` : `${exact(data.total)} players`;
    const notice = $('board-notice');
    notice.hidden = total <= shown;
    notice.textContent = `Showing ${shown} of ${exact(total)} ${search ? 'matches' : 'players'}.`;
    $('clear-search')?.addEventListener('click', () => { $('rank-search').value = ''; search = ''; load(); $('rank-search').focus(); });
  }
  function render() {
    renderMine(); renderRows();
    $('board-updated').textContent = 'Updated ' + new Date().toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' });
    $('board-status').textContent = `${label(data.metric)} rankings loaded. ${exact(data.matchCount ?? data.total)} players${search ? ' match your search' : ''}.`;
  }
  async function load() {
    clearTimeout(debounce);
    debounce = null;
    request?.abort();
    request = new AbortController();
    const version = ++sequence;
    $('board-content').setAttribute('aria-busy', 'true');
    $('refresh-board').classList.add('is-loading');
    try {
      const params = new URLSearchParams({ metric: activeMetric });
      if (search) params.set('q', search);
      const response = await fetch('/api/leaderboard?' + params, { credentials: 'include', cache: 'no-store', signal: request.signal });
      if (!response.ok) throw new Error('Rankings unavailable');
      const result = await response.json();
      if (version !== sequence) return;
      data = result;
      render();
    } catch (error) {
      if (version !== sequence || error.name === 'AbortError') return;
      const message = data ? 'Could not refresh. Showing previous results.' : 'Unable to load rankings.';
      $('board-status').textContent = message;
      $('board-updated').textContent = message;
      if (!data) {
        $('my-rank').hidden = true;
        $('rank-count').textContent = 'Rankings unavailable';
        $('rank-list').innerHTML = `<li class="board-empty"><p>${message}</p><button type="button" id="retry-board">Try again</button></li>`;
        $('retry-board').addEventListener('click', load);
      }
    } finally {
      if (version === sequence) {
        $('board-content').setAttribute('aria-busy', 'false');
        $('refresh-board').classList.remove('is-loading');
      }
    }
  }
  $('metric-tabs').innerHTML = metrics.map(([key, title]) => `<button type="button" class="board-category" data-metric="${key}" aria-pressed="${key === activeMetric}">${title}</button>`).join('');
  $('metric-tabs').addEventListener('click', event => {
    const button = event.target.closest('[data-metric]');
    if (!button || button.dataset.metric === activeMetric) return;
    activeMetric = button.dataset.metric;
    document.querySelectorAll('.board-category').forEach(tab => tab.setAttribute('aria-pressed', String(tab === button)));
    load();
  });
  $('rank-search').addEventListener('input', event => {
    search = event.target.value.trim();
    // Immediately invalidate pending responses while waiting for the debounce.
    request?.abort(); sequence++;
    clearTimeout(debounce); debounce = setTimeout(load, 220);
  });
  $('refresh-board').addEventListener('click', load);
  // A failed external profile image falls back to the same initial as profiles.
  $('board-content').addEventListener('error', event => {
    if (event.target instanceof HTMLImageElement) event.target.remove();
  }, true);
  setInterval(() => { if (!document.hidden && !debounce && $('board-content').getAttribute('aria-busy') !== 'true') load(); }, 60000);
  load();
})();
