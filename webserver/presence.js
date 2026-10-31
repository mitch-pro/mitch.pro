(() => {
  if (window.MitchPresence) return;
  const initialPage = document.currentScript?.dataset.presencePage;
  const siteOrigin = document.currentScript?.src ? new URL(document.currentScript.src).origin : location.origin;
  const nativeFetch = window.fetch.bind(window);
  const clientId = crypto.randomUUID?.() || 'page-' + Date.now().toString(36) + '-' + Math.random().toString(36).slice(2);
  let sequence = 0, interaction = 0, timer, pending, transport, explicit, started = false, alive = true;
  let owner;
  try { if (parent !== window && parent.MitchPresence) owner = parent.MitchPresence; } catch (_) {}
  const frames = new Map();
  const watched = new WeakSet();
  const labels = {
    matrix: 'In Matrix chat',
    members: 'Viewing people', friends: 'Viewing friends', profile: 'Viewing a profile',
    shop: 'Browsing the shop', marketplace: 'Browsing the market', inventory: 'Viewing inventory',
    coins: 'Viewing MitchCoins', leaderboard: 'Viewing the leaderboard',
    preferences: 'Changing preferences', notifications: 'Viewing notifications',
    vms: 'Managing Cloud Linux', bell: 'Viewing the bell schedule', rjuhsd: 'Viewing school schedules',
    admin: 'Using admin tools', moderator: 'Using moderator tools', tester: 'Using tester tools',
    tabs: "Using Tyler's Tabs", canvas: 'Drawing on the canvas',
    team: 'Viewing the team', feedback: 'Viewing feedback', blog: 'Reading the blog',
    enroll: 'Viewing account access', 'index-sales.html': 'Viewing the site introduction'
  };
  function clean(value, limit = 120) { return String(value || '').replace(/[\u0000-\u001f\u007f]/g, '').replace(/\s+/g, ' ').trim().slice(0, limit); }
  function pathOf(value) { try { return new URL(value, siteOrigin + '/').pathname; } catch (_) { return location.pathname; } }
  if (initialPage) explicit = { page: pathOf(initialPage) };
  function describe(page, title) {
    const root = page.split('/').filter(Boolean)[0] || '';
    if (!root || root === 'index.html') return 'On the homepage';
    if (page.startsWith('/proxy/rivestream')) return page.includes('/watch') ? 'Viewing a movie' : 'Browsing movies';
    if (page.startsWith('/vms/desktop')) return 'Using a VM desktop';
    if (page.startsWith('/matrix/public/element-call/')) return 'In a Matrix call';
    if (root === 'game-portal' || /^\/games\/(?:index.html)?$/.test(page)) return 'Browsing games';
    if (root === 'games' || /^\/proxy\/(luma|calculated2|gamemonetize)\//.test(page)) {
      const name = clean(title).replace(/\s*[—|]\s*(?:mitch\.pro|games?).*$/i, '').trim();
      return 'Playing ' + (name || page.split('/').filter(Boolean).at(-1)?.replace(/[-_]/g, ' ') || 'a game');
    }
    return labels[root] || 'Viewing ' + root.replace(/[-_]/g, ' ').replace(/\.html$/, '');
  }
  function usable(frame) {
    const rect = frame.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0 && !frame.closest('[hidden]') && getComputedStyle(frame).visibility !== 'hidden';
  }
  function selectedFrame() {
    return [...document.querySelectorAll('iframe')].find(frame => usable(frame) &&
      (document.activeElement === frame || frame.matches('#fullframe, .mitch-iab-frame, #player-frame')));
  }
  function isLocationHidden() {
    try {
      const priv = JSON.parse(localStorage.getItem('_prefPrivacy') || '{}');
      return priv.shareActivity !== true || !!(priv.hideLocation || priv.hidePageLocation);
    } catch (_) { return false; }
  }
  function details() {
    const hideLocation = isLocationHidden();
    let page = hideLocation ? '' : (explicit?.page || location.pathname || '/');
    let activity = hideLocation ? '' : (explicit?.activity || describe(page, document.title));
    const frame = hideLocation ? null : selectedFrame();
    if (frame && frame.src && frame.src !== 'about:blank') {
      const reported = frames.get(frame.contentWindow);
      page = reported?.page || pathOf(frame.src);
      activity = explicit?.activity || reported?.activity || describe(page, frame.title);
    }
    return { type: 'presence_ping', clientId, seq: ++sequence, interaction,
      page: hideLocation ? '' : pathOf(page), activity: hideLocation ? '' : clean(activity), visible: !document.hidden,
      focused: !document.hidden && document.hasFocus(), ended: !alive, hideLocation,
      shareActivity: !hideLocation, hideOnline: (() => { try { return !!JSON.parse(localStorage.getItem('_prefPrivacy') || '{}').hideOnline; } catch { return false; } })() };
  }
  function send(ended = false) {
    if (!alive && !ended) return;
    const packet = details(); packet.ended = ended;
    if (owner) { owner.reportFrame(window, packet); return; }
    let sent = false;
    try { sent = !!transport?.(packet); } catch (_) {}
    if (sent && !ended) return;
    nativeFetch(siteOrigin + '/api/presence/heartbeat', {
      method: 'POST', credentials: 'same-origin', keepalive: true, cache: 'no-store',
      headers: { 'Content-Type': 'application/json', 'X-Mitch-Requested-With': '1' }, body: JSON.stringify(packet)
    }).then(response => {
      if (response.status === 401 || response.status === 403) { alive = false; clearInterval(timer); }
    }).catch(() => {});
  }
  function soon() { if (!alive || pending) return; pending = setTimeout(() => { pending = null; send(); }, 40); }
  function reportFrame(source, packet) {
    const frame = [...document.querySelectorAll('iframe')].find(frame => frame.contentWindow === source);
    if (!frame) return;
    const previous = frames.get(source);
    if (packet.ended) frames.delete(source); else frames.set(source, packet);
    if (usable(frame) && (document.activeElement === frame || frame.matches('#fullframe, .mitch-iab-frame, #player-frame'))) {
      if (previous?.interaction !== packet.interaction) interaction++;
      soon();
    }
  }
  window.MitchPresence = {
    delegated: !!owner, details, send, reportFrame,
    setTransport(sender) { transport = sender; send(); },
    setActivity(activity, page) { explicit = activity ? { activity: clean(activity), page: pathOf(page || location.pathname) } : null; interaction++; soon(); }
  };
  for (const name of ['pushState', 'replaceState']) {
    const original = history[name];
    history[name] = function(...args) { const result = original.apply(this, args); interaction++; soon(); return result; };
  }
  function watchFrames() {
    for (const frame of document.querySelectorAll('iframe')) {
      if (watched.has(frame)) continue;
      watched.add(frame); frame.addEventListener('load', soon);
      new MutationObserver(soon).observe(frame, { attributes: true, attributeFilter: ['src', 'hidden', 'style', 'class'] });
    }
    for (const source of frames.keys()) if (![...document.querySelectorAll('iframe')].some(frame => frame.contentWindow === source)) frames.delete(source);
  }
  function start() {
    if (started) return; started = true;
    watchFrames();
    new MutationObserver(watchFrames).observe(document.body, { childList: true, subtree: true });
    new MutationObserver(soon).observe(document.body, { attributes: true, attributeFilter: ['class', 'hidden'] });
    const title = document.querySelector('title');
    if (title) new MutationObserver(soon).observe(title, { childList: true, subtree: true, characterData: true });
    send(); timer = setInterval(send, 15000);
    window.dispatchEvent(new Event('mitch-presence-ready'));
  }
  let lastInteraction = 0;
  function interacted() {
    if (Date.now() - lastInteraction < 3000) return;
    lastInteraction = Date.now(); interaction++; soon();
  }
  document.addEventListener('pointerdown', interacted, true);
  document.addEventListener('keydown', interacted, true);
  document.addEventListener('visibilitychange', () => { interaction++; soon(); });
  window.addEventListener('focus', () => { interaction++; soon(); });
  window.addEventListener('blur', soon);
  window.addEventListener('hashchange', soon);
  window.addEventListener('popstate', () => { interaction++; soon(); });
  window.addEventListener('online', soon);
  window.addEventListener('storage', event => { if (event.key === '_prefPrivacy') { interaction++; soon(); } });
  window.addEventListener('pagehide', () => { alive = false; clearInterval(timer); clearTimeout(pending); pending = null; send(true); });
  window.addEventListener('pageshow', event => { if (event.persisted) { alive = true; interaction++; timer = setInterval(send, 15000); send(); } });
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', start, { once: true }); else start();
})();
