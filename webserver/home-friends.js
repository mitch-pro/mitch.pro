(() => {
  const hero = document.querySelector('#mainpage .hud-topbar');
  if (!hero || document.querySelector('.home-friends')) return;
  const section = document.createElement('section');
  section.className = 'home-friends';
  section.setAttribute('aria-label', 'Friends activity');
  section.innerHTML = '<header><h2>Friends <span class="home-friend-count"></span></h2><a href="/friends/">Manage friends</a></header><div class="home-friend-list"></div>';
  hero.after(section);
  const list = section.querySelector('.home-friend-list');
  function activityLabel(friend) {
    if (!friend.online) return 'Offline';
    const activity = String(friend.activity || friend.playing || 'Online now').trim().slice(0,120);
    return (friend.status === 'away' ? 'Away · ' : '') + activity;
  }
  function render(friends) {
    list.replaceChildren();
    const onlineCount = friends.filter(f => f.online).length;
    const countEl = section.querySelector('.home-friend-count');
    countEl.textContent = onlineCount ? `${onlineCount} online` : '';
    countEl.title = friends.length ? `${friends.length} total friend${friends.length === 1 ? '' : 's'}` : '';
    section.classList.toggle('is-empty',!friends.length);
    if (!friends.length) {
      const empty = document.createElement('div'); empty.className = 'home-friends-empty';
      empty.innerHTML = '<a class="home-friend-add" href="/friends/" aria-label="Add friends"><span aria-hidden="true">+</span></a><div><strong>Find friends</strong><small></small></div>';
      list.append(empty); return;
    }
    friends.sort(window.MitchPeople?.compare || ((a,b) => Number(b.online)-Number(a.online))).forEach(friend => {
      const card = document.createElement('a'); card.className = 'home-friend' + (friend.online ? ' online' : '');
      card.href = '/profile/?u=' + encodeURIComponent(friend.handle || '');
      const avatar = document.createElement('span'); avatar.className = 'home-friend-avatar';
      const name = String(friend.displayName || friend.handle || 'Friend'); avatar.textContent = name.slice(0,1).toUpperCase();
      if (friend.pfp && /^(https?:\/\/|\/(?!\/)|data:image\/(png|jpeg|webp|gif);base64,)/i.test(friend.pfp)) {
        const img = document.createElement('img'); img.src = friend.pfp; img.alt = ''; img.loading = 'lazy'; img.onerror = () => img.remove(); avatar.append(img);
        if (avatar.firstChild.nodeType === Node.TEXT_NODE) avatar.firstChild.remove();
        img.onerror = () => { avatar.textContent = name.slice(0,1).toUpperCase(); };
      }
      const title = document.createElement('strong'); title.textContent = name; title.className = 'name ' + (friend.color || '');
      const activity = document.createElement('small');
      const activityText = document.createElement('span'); activityText.textContent = activityLabel(friend);
      const activityIcon = document.createElement('i'); activityIcon.setAttribute('aria-hidden','true');
      const rawActivity = String(friend.activity || friend.playing || '').toLowerCase();
      activityIcon.textContent = !friend.online ? '' : (/game|chess|playing/.test(rawActivity) ? '▶' : (/chat|matrix|encrypt/.test(rawActivity) ? '●' : '•'));
      activity.append(activityIcon,activityText);
      card.title = name + ' · ' + activity.textContent; card.append(avatar,title,activity); list.append(card);
    });
  }
  let busy = false, refreshAgain = false;
  async function refresh() {
    if (document.hidden) return;
    if (busy) { refreshAgain = true; return; }
    busy = true;
    try { const response = await fetch('/api/friends/list', { credentials:'same-origin', cache:'no-store' }); if (!response.ok) throw new Error('Friends unavailable'); const data = await response.json(); render(Array.isArray(data.friends) ? data.friends : []); }
    catch { list.querySelectorAll('.home-friend small span').forEach(el => { el.textContent = 'Status unavailable'; }); section.querySelector('.home-friend-count').textContent = ''; list.querySelectorAll('.home-friend.online').forEach(el => el.classList.remove('online')); }
    finally { busy = false; if (refreshAgain) { refreshAgain = false; setTimeout(refresh, 500); } }
  }
  render([]); refresh();
  let timer = setInterval(refresh, 30000);
  document.addEventListener('visibilitychange', refresh);
  let pending;
  window.addEventListener('ws-broadcast-message', event => {
    if (event.detail?.type !== 'presence_changed') return;
    if (!pending) pending = setTimeout(() => { pending = null; refresh(); }, 1000);
  });
  window.addEventListener('ws-broadcast-status', refresh);
  window.addEventListener('pagehide', () => clearInterval(timer));
  window.addEventListener('pageshow', event => { if (event.persisted) { clearInterval(timer); timer = setInterval(refresh,30000); refresh(); } });
})();
