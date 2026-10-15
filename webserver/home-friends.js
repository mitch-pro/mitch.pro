(() => {
  const hero = document.querySelector('#mainpage .hud-topbar');
  if (!hero || document.querySelector('.home-friends')) return;
  const section = document.createElement('section');
  section.className = 'home-friends';
  section.setAttribute('aria-label', 'Friends activity');
  section.innerHTML = '<header><h2>Friends <span class="home-friend-count"></span></h2><a href="https://mitchdog.com/members/">See All</a></header><div class="home-friend-list"></div>';
  hero.after(section);
  const list = section.querySelector('.home-friend-list');
  function activityLabel(friend) {
    if (!friend.online) return 'Offline';
    const activity = String(friend.playing || '').trim().slice(0,100);
    const labels = { '/':'On the homepage',shop:'Browsing the market',marketplace:'Browsing the market',members:'Finding friends',friends:'With friends',profile:'Viewing profiles',preferences:'Customizing their theme',matrix:'In chat',encrypt:'In chat',chat:'In chat',canvas:'Drawing on the canvas','game-portal':'Browsing games',games:'Browsing games',vms:'Using My Computer' };
    if (!activity) return 'Online now';
    if (activity.startsWith('/') || /^https?:\/\//i.test(activity)) {
      try { const path = new URL(activity,location.origin).pathname; return labels[path.split('/').filter(Boolean)[0] || '/'] || 'Online now'; } catch { return 'Online now'; }
    }
    return labels[activity.toLowerCase()] || (/^(playing|browsing|in |using )/i.test(activity) ? activity : 'Playing ' + activity);
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
      empty.innerHTML = '<a class="home-friend-add" href="https://mitchdog.com/members/" aria-label="Add friends"><span aria-hidden="true">+</span></a><div><strong>Add some friends</strong><small>Find people and send a friend request.</small></div>';
      list.append(empty); return;
    }
    friends.sort((a,b) => Number(b.online)-Number(a.online)).forEach(friend => {
      const card = document.createElement('a'); card.className = 'home-friend' + (friend.online ? ' online' : '');
      card.href = '/profile/?u=' + encodeURIComponent(friend.handle || '');
      const avatar = document.createElement('span'); avatar.className = 'home-friend-avatar';
      const name = String(friend.displayName || friend.handle || 'Friend'); avatar.textContent = name.slice(0,1).toUpperCase();
      if (friend.pfp && /^(https?:\/\/|\/(?!\/)|data:image\/(png|jpeg|webp|gif);base64,)/i.test(friend.pfp)) {
        const img = document.createElement('img'); img.src = friend.pfp; img.alt = ''; img.loading = 'lazy'; img.onerror = () => img.remove(); avatar.append(img);
        if (avatar.firstChild.nodeType === Node.TEXT_NODE) avatar.firstChild.remove();
        img.onerror = () => { avatar.textContent = name.slice(0,1).toUpperCase(); };
      }
      const title = document.createElement('strong'); title.textContent = name;
      const activity = document.createElement('small');
      const activityText = document.createElement('span'); activityText.textContent = activityLabel(friend);
      const activityIcon = document.createElement('i'); activityIcon.setAttribute('aria-hidden','true');
      const rawActivity = String(friend.playing || '').toLowerCase();
      activityIcon.textContent = !friend.online ? '' : (/game|chess|casino|playing/.test(rawActivity) ? '▶' : (/chat|matrix|encrypt/.test(rawActivity) ? '●' : '•'));
      activity.append(activityIcon,activityText);
      card.title = name + ' · ' + activity.textContent; card.append(avatar,title,activity); list.append(card);
    });
  }
  let busy = false;
  async function refresh() {
    if (document.hidden || busy) return;
    busy = true;
    try { const response = await fetch('/api/friends/list', { credentials:'same-origin', cache:'no-store' }); if (response.ok) { const data = await response.json(); render(Array.isArray(data.friends) ? data.friends : []); } }
    catch {} finally { busy = false; }
  }
  render([]); refresh();
  let timer = setInterval(refresh, 30000);
  document.addEventListener('visibilitychange', refresh);
  window.addEventListener('pagehide', () => clearInterval(timer));
  window.addEventListener('pageshow', event => { if (event.persisted) { clearInterval(timer); timer = setInterval(refresh,30000); refresh(); } });
})();
