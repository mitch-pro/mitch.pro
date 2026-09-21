(function setupBroadcast() {
    var ws;
    var presenceTimer;
    var reconnectTimer;
    var fallbackTimer;
    var peopleRefreshTimer;
    var LAST_BROADCAST_KEY = 'mitch:last-admin-broadcast';
    var lastBroadcastId = '';
    var pageOpenedAt = Date.now();
    function stopPresencePing() {
      if (presenceTimer) clearInterval(presenceTimer);
      presenceTimer = null;
    }
    function presenceDetails() {
      var path = location.pathname || '/';
      var explicit = String(window.currentPlayingActivity || '').trim();
      var activity = explicit;
      if (/^\/matrix(?:\/|$)/.test(path)) activity = 'Chatting in Matrix';
      else if (/^\/encrypt(?:\/|$)/.test(path)) activity = 'Using secure chat';
      else if (/^\/public-chat(?:\/|$)/.test(path)) activity = 'In public chat';
      else if (/^\/vms\/desktop(?:\/|$)/.test(path)) activity = 'Using VM desktop';
      else if (/^\/vms(?:\/|$)/.test(path)) activity = 'Managing a VM';
      else if (/^\/(?:game-portal|games|casino|chess)(?:\/|$)/.test(path) && !activity) activity = 'Playing games';
      else if (path === '/' || path === '/index.html') activity = 'On the homepage';
      else if (!activity) activity = 'Viewing ' + (document.title || path).replace(/\s*[—|-]\s*mitch(?:\.pro)?\s*$/i, '').trim();
      return {
        type: 'presence_ping',
        activity: activity.slice(0, 120),
        playing: activity.slice(0, 120),
        page: path.slice(0, 180),
        title: String(document.title || '').slice(0, 100),
        visible: !document.hidden
      };
    }
    function sendPresencePing() {
      var details = presenceDetails();
      if (ws && ws.readyState === WebSocket.OPEN) {
        try { ws.send(JSON.stringify(details)); return; } catch(ex) {}
      }
      fetch('/api/presence/heartbeat', {
        method: 'POST', credentials: 'same-origin', keepalive: true,
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(details)
      }).catch(function() {});
    }
    function hasSeenBroadcast(data) {
      if (!data || !data.broadcastId) return false;
      if (lastBroadcastId === data.broadcastId) return true;
      try {
        if (sessionStorage.getItem(LAST_BROADCAST_KEY) === data.broadcastId) return true;
        sessionStorage.setItem(LAST_BROADCAST_KEY, data.broadcastId);
      } catch(ex) {}
      lastBroadcastId = data.broadcastId;
      return false;
    }
    function handleMessage(data) {
      if ((data.type === 'admin_broadcast' || data.type === 'admin_jumpscare') && data.createdAt && Number(data.createdAt) < pageOpenedAt) {
        return;
      }
      if (data.type === 'admin_broadcast') {
        if (!hasSeenBroadcast(data)) showBroadcast(data.message);
      } else if (data.type === 'admin_jumpscare') {
        if (!hasSeenBroadcast(data)) showJumpscare(data.message);
      } else if (data.type === 'refresh_notifications') {
        if (typeof window.__refreshNotifications === 'function') {
          window.__refreshNotifications();
        }
      } else if (data.type === 'new_dm') {
        if (typeof window.__handleIncomingDm === 'function') {
          window.__handleIncomingDm(data.message || null);
        }
      }
      if (data.type === 'presence_changed') schedulePeopleRefresh();
      window.dispatchEvent(new CustomEvent('ws-broadcast-message', { detail: data }));
    }
    function scheduleReconnect() {
      if (reconnectTimer) return;
      reconnectTimer = setTimeout(function() {
        reconnectTimer = null;
        connect();
      }, 1800);
    }
    function connect() {
      if (ws && (ws.readyState === WebSocket.OPEN || ws.readyState === WebSocket.CONNECTING)) return;
      var protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
      try {
        ws = new WebSocket(protocol + '//' + location.host + '/ws');
      } catch(ex) {
        scheduleReconnect();
        return;
      }
      ws.onmessage = function(e) {
        try {
          var data = JSON.parse(e.data);
          handleMessage(data);
        } catch(ex) {}
      };
      ws.onopen = function() {
        sendPresencePing();
        window.dispatchEvent(new CustomEvent('ws-broadcast-status', { detail: { connected: true } }));
      };
      ws.onclose = function() {
        window.dispatchEvent(new CustomEvent('ws-broadcast-status', { detail: { connected: false } }));
        scheduleReconnect();
      };
    }
    function formatPresenceAge(ts) {
      var minutes = Math.max(0, Math.floor((Date.now() - Number(ts || Date.now())) / 60000));
      if (minutes < 1) return 'just now';
      if (minutes < 60) return minutes + 'm';
      return Math.floor(minutes / 60) + 'h ' + (minutes % 60) + 'm';
    }
    function ensurePeoplePanel() {
      if (document.getElementById('site-presence')) return;
      var style = document.createElement('style');
      style.textContent = '#site-presence{position:fixed;right:16px;bottom:16px;z-index:2147482000;font:12px/1.35 system-ui,sans-serif;color:#f4f7fb}#site-presence button{font:inherit}.site-presence-toggle{display:flex;align-items:center;gap:8px;min-height:38px;padding:8px 12px;border:1px solid #ffffff28;border-radius:999px;background:#111827ed;color:#fff;box-shadow:0 10px 35px #0007;cursor:pointer}.site-presence-dot{width:9px;height:9px;border-radius:50%;background:#42d392;box-shadow:0 0 0 4px #42d39225}.site-presence-panel{display:none;position:absolute;right:0;bottom:46px;width:min(360px,calc(100vw - 24px));max-height:min(520px,70vh);overflow:auto;border:1px solid #ffffff24;border-radius:18px;background:#101722f7;box-shadow:0 22px 70px #000a;backdrop-filter:blur(18px)}#site-presence.open .site-presence-panel{display:block}.site-presence-head{position:sticky;top:0;display:flex;justify-content:space-between;gap:10px;padding:14px 15px;background:#101722fa;border-bottom:1px solid #ffffff18}.site-presence-head strong{font-size:14px}.site-presence-head a{color:#a8c7fa}.site-presence-list{display:grid;gap:7px;padding:10px}.site-presence-person{display:grid;grid-template-columns:34px minmax(0,1fr) auto;gap:9px;align-items:center;padding:9px;border-radius:12px;color:inherit;text-decoration:none;background:#ffffff08}.site-presence-person:hover{background:#ffffff10}.site-presence-person.is-chat{background:#7c3aed22;border:1px solid #a78bfa45}.site-presence-avatar{display:grid;place-items:center;width:34px;height:34px;border-radius:50%;background:#263348;font-weight:800;overflow:hidden}.site-presence-avatar img{width:100%;height:100%;object-fit:cover}.site-presence-copy{min-width:0}.site-presence-name,.site-presence-activity,.site-presence-page{display:block;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.site-presence-name{font-weight:750}.site-presence-activity{color:#cbd5e1}.site-presence-page{color:#8290a5;font:10px ui-monospace,monospace}.site-presence-age{color:#8290a5;font-size:10px}.site-presence-empty{padding:22px;text-align:center;color:#9aa6b8}@media(max-width:760px){#site-presence{right:10px;bottom:calc(72px + env(safe-area-inset-bottom))}.site-presence-panel{bottom:44px}}';
      document.head.appendChild(style);
      var root = document.createElement('aside');
      root.id = 'site-presence';
      root.setAttribute('aria-label', 'People online');
      root.innerHTML = '<button class="site-presence-toggle" type="button" aria-expanded="false"><span class="site-presence-dot"></span><span class="site-presence-count">Online</span></button><section class="site-presence-panel"><header class="site-presence-head"><strong>Who\'s online</strong><a href="/members/">All people</a></header><div class="site-presence-list"><div class="site-presence-empty">Checking activity…</div></div></section>';
      document.body.appendChild(root);
      root.querySelector('.site-presence-toggle').onclick = function(event) {
        event.stopPropagation();
        root.classList.toggle('open');
        this.setAttribute('aria-expanded', String(root.classList.contains('open')));
        if (root.classList.contains('open')) refreshPeoplePanel();
      };
      root.onclick = function(event) { event.stopPropagation(); };
      document.addEventListener('click', function() { root.classList.remove('open'); root.querySelector('.site-presence-toggle').setAttribute('aria-expanded', 'false'); });
    }
    function renderPeoplePanel(members) {
      ensurePeoplePanel();
      var root = document.getElementById('site-presence');
      var online = (members || []).filter(function(member) { return member.online; });
      online.sort(function(a, b) {
        var aChat = /chat|matrix|encrypt/i.test(String(a.activity || a.playing || '') + ' ' + String(a.page || ''));
        var bChat = /chat|matrix|encrypt/i.test(String(b.activity || b.playing || '') + ' ' + String(b.page || ''));
        return Number(bChat) - Number(aChat) || String(a.displayName || a.handle || '').localeCompare(String(b.displayName || b.handle || ''));
      });
      root.querySelector('.site-presence-count').textContent = online.length + ' online';
      var list = root.querySelector('.site-presence-list');
      list.replaceChildren();
      if (!online.length) { list.innerHTML = '<div class="site-presence-empty">Nobody else is active right now.</div>'; return; }
      online.forEach(function(member) {
        var activity = String(member.activity || member.playing || 'Online now');
        var page = String(member.page || '/');
        var chat = /chat|matrix|encrypt/i.test(activity + ' ' + page);
        var name = String(member.displayName || member.handle || member.email || 'Member');
        var card = document.createElement('a');
        card.className = 'site-presence-person' + (chat ? ' is-chat' : '');
        card.href = member.profileUrl || ('/profile/?u=' + encodeURIComponent(member.handle || member.email || ''));
        var avatar = document.createElement('span'); avatar.className = 'site-presence-avatar'; avatar.textContent = name.slice(0,1).toUpperCase();
        if (member.pfp) { var img = document.createElement('img'); img.src = member.pfp; img.alt = ''; img.onerror = function(){ img.remove(); }; avatar.appendChild(img); }
        var copy = document.createElement('span'); copy.className = 'site-presence-copy';
        var nameEl = document.createElement('span'); nameEl.className = 'site-presence-name'; nameEl.textContent = name + (chat ? ' · CHAT' : '');
        var activityEl = document.createElement('span'); activityEl.className = 'site-presence-activity'; activityEl.textContent = activity;
        var pageEl = document.createElement('code'); pageEl.className = 'site-presence-page'; pageEl.textContent = page;
        copy.append(nameEl, activityEl, pageEl);
        var age = document.createElement('span'); age.className = 'site-presence-age'; age.textContent = formatPresenceAge(member.onlineSince);
        card.append(avatar, copy, age); list.appendChild(card);
      });
    }
    function refreshPeoplePanel() {
      fetch('/api/members?t=' + Date.now(), { credentials: 'same-origin', cache: 'no-store' })
        .then(function(response) { return response.ok ? response.json() : null; })
        .then(function(data) { if (data) renderPeoplePanel(data.members || []); })
        .catch(function() {});
    }
    function schedulePeopleRefresh() {
      clearTimeout(peopleRefreshTimer);
      peopleRefreshTimer = setTimeout(refreshPeoplePanel, 150);
    }
    function pollLatestBroadcast() {
      fetch('/api/broadcast/latest', {
        credentials: 'same-origin',
        cache: 'no-store',
        headers: { 'Accept': 'application/json' }
      }).then(function(response) {
        if (!response.ok) return null;
        return response.json();
      }).then(function(result) {
        if (result && result.active && result.event) handleMessage(result.event);
      }).catch(function() {});
    }
    function start() {
      connect();
      stopPresencePing();
      sendPresencePing();
      presenceTimer = setInterval(sendPresencePing, 20000);
      ensurePeoplePanel();
      refreshPeoplePanel();
      pollLatestBroadcast();
      fallbackTimer = setInterval(pollLatestBroadcast, 3000);
      document.addEventListener('visibilitychange', function() {
        sendPresencePing();
        if (!document.hidden) { pollLatestBroadcast(); refreshPeoplePanel(); }
      });
      setInterval(refreshPeoplePanel, 30000);
    }
    function showJumpscare(msg) {
      showVideoJumpscare(msg);
    }
    function showVideoJumpscare(msg) {
      var previous = document.getElementById('admin-video-jumpscare');
      if (previous) {
        var previousVideo = previous.querySelector('video');
        if (previousVideo) previousVideo.pause();
        previous.remove();
      }
      var overlay = document.createElement('div');
      overlay.id = 'admin-video-jumpscare';
      overlay.setAttribute('role', 'dialog');
      overlay.setAttribute('aria-modal', 'true');
      overlay.style.cssText = 'position:fixed;inset:0;background:#000;z-index:2147483647;display:grid;place-items:center;overflow:hidden;';
      var video = document.createElement('video');
      video.src = '/media/admin-jumpscare-krupp-1935.webm';
      video.autoplay = true;
      video.playsInline = true;
      video.preload = 'auto';
      video.style.cssText = 'width:100%;height:100%;object-fit:contain;background:#000;';
      var close = document.createElement('button');
      close.type = 'button';
      close.setAttribute('aria-label', 'Close video');
      close.textContent = '×';
      close.style.cssText = 'position:absolute;top:max(12px,env(safe-area-inset-top));right:14px;width:42px;height:42px;border:1px solid #ffffff55;border-radius:50%;background:#000a;color:#fff;font:26px/1 system-ui;cursor:pointer;z-index:2;';
      var caption = document.createElement('div');
      caption.style.cssText = 'position:absolute;left:50%;bottom:max(18px,env(safe-area-inset-bottom));translate:-50% 0;max-width:min(760px,calc(100% - 40px));padding:9px 14px;border-radius:10px;background:#000b;color:#fff;font:700 clamp(14px,2vw,22px)/1.25 system-ui;text-align:center;';
      caption.textContent = String(msg || '').trim();
      caption.hidden = !caption.textContent;
      function remove() { video.pause(); video.removeAttribute('src'); overlay.remove(); }
      close.onclick = remove;
      video.addEventListener('ended', remove, { once: true });
      video.addEventListener('error', function() { caption.hidden = false; caption.textContent = 'The video could not be loaded.'; });
      overlay.append(video, close, caption);
      document.body.appendChild(overlay);
      video.play().catch(function() {
        video.muted = true;
        video.play().catch(function() {});
      });
    }
    function showBroadcast(msg) {
      var el = document.createElement('div');
      el.style.cssText = 'position:fixed;top:0;left:0;right:0;background:#ef4444;color:#fff;padding:1.5rem;text-align:center;z-index:1000000;font-weight:900;box-shadow:0 10px 40px rgba(0,0,0,0.5);font-family:system-ui,sans-serif;font-size:1.1rem;animation:slideDown .4s ease-out;';
      el.innerHTML = '<div style="margin-bottom:10px;font-size:.8rem;opacity:.8;letter-spacing:.1em;text-transform:uppercase;">Global Broadcast</div>' + msg + '<div style="margin-top:15px;"><button id="close-broadcast" style="background:#fff;color:#000;border:none;border-radius:6px;padding:6px 15px;font-weight:800;cursor:pointer;">Dismiss</button></div>';
      
      if (!document.getElementById('broadcast-style')) {
        var style = document.createElement('style');
        style.id = 'broadcast-style';
        style.textContent = '@keyframes slideDown { from { transform: translateY(-100%); } to { transform: translateY(0); } }';
        document.head.appendChild(style);
      }
      
      document.body.appendChild(el);
      el.querySelector('#close-broadcast').onclick = function() { el.remove(); };
    }
    if (document.body) start();
    else document.addEventListener('DOMContentLoaded', start, { once: true });
})();

// Site-wide Notifications
// ── In-app browser sheet ─────────────────────────────────────────────────
// Push notification clicks land here (the SW postMessages the URL): the
// target opens inside an Apple-style sheet with its own address bar and a
// Done button, instead of navigating the whole PWA window.
(function setupInAppBrowser() {
  function hostOf(url) {
    try { return new URL(url, location.href).host; } catch (e) { return ''; }
  }
  function openInAppBrowser(rawUrl) {
    if (!rawUrl) return;
    var target;
    try { target = new URL(rawUrl, location.href); } catch (e) { return; }
    // Only same-origin pages can be framed; anything else opens normally.
    if (target.origin !== location.origin) { location.assign(target.href); return; }
    var existing = document.getElementById('mitch-iab');
    if (existing) existing.remove();
    var wrap = document.createElement('div');
    wrap.id = 'mitch-iab';
    wrap.setAttribute('role', 'dialog');
    wrap.setAttribute('aria-label', 'In-app browser');
    wrap.innerHTML =
      '<div class="mitch-iab-bar">' +
      '  <span class="mitch-iab-lock" aria-hidden="true">&#128274;</span>' +
      '  <b></b>' +
      '  <button id="mitch-iab-done" type="button">Done</button>' +
      '</div>' +
      '<iframe class="mitch-iab-frame" title="In-app browser" src="' + target.href + '"></iframe>';
    wrap.querySelector('.mitch-iab-bar b').textContent = hostOf(target.href);
    document.body.appendChild(wrap);
    requestAnimationFrame(function () { wrap.classList.add('show'); });
    wrap.querySelector('#mitch-iab-done').onclick = function () {
      wrap.classList.remove('show');
      var frame = wrap.querySelector('iframe');
      if (frame) frame.src = 'about:blank';
      setTimeout(function () { if (wrap.parentNode) wrap.remove(); }, 240);
    };
  }
  if ('serviceWorker' in navigator) {
    try {
      navigator.serviceWorker.addEventListener('message', function (ev) {
        if (ev.data && ev.data.type === 'open-in-app-browser' && ev.data.url) {
          openInAppBrowser(ev.data.url);
        }
      });
    } catch (e) {}
  }
  var css = document.createElement('style');
  css.id = 'mitch-iab-style';
  css.textContent =
    '#mitch-iab{position:fixed;inset:0;z-index:2147483640;display:flex;flex-direction:column;' +
    'background:var(--t-bg,#0a0817);opacity:0;transition:opacity .22s ease,transform .22s ease;transform:translateY(14px);' +
    'padding-top:env(safe-area-inset-top,0px)}' +
    '#mitch-iab.show{opacity:1;transform:none}' +
    '#mitch-iab .mitch-iab-bar{flex:0 0 auto;display:flex;align-items:center;gap:8px;padding:10px 12px;' +
    'padding-left:max(12px,env(safe-area-inset-left,0px));padding-right:max(12px,env(safe-area-inset-right,0px));' +
    'border-bottom:1px solid var(--t-bd,rgba(255,255,255,.14));background:var(--t-bg2,rgba(20,16,40,.96))}' +
    '#mitch-iab .mitch-iab-lock{font-size:12px;opacity:.7}' +
    '#mitch-iab .mitch-iab-bar b{flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;' +
    'font:800 .82rem/1.2 var(--t-font,system-ui,sans-serif);color:var(--t-fg,#fff)}' +
    '#mitch-iab .mitch-iab-bar button{flex:0 0 auto;padding:7px 14px;border-radius:10px;cursor:pointer;' +
    'border:1px solid var(--t-bd,rgba(255,255,255,.16));background:var(--t-bg3,rgba(255,255,255,.08));' +
    'color:var(--t-ac,#c9a5ff);font:800 .78rem/1 system-ui,sans-serif}' +
    '#mitch-iab .mitch-iab-frame{flex:1;width:100%;border:0;background:#fff}';
  document.head.appendChild(css);
  window.__openInAppBrowser = openInAppBrowser;
})();

(function setupNotifications() {
  function escText(t) {
    var d = document.createElement('div');
    d.textContent = t == null ? '' : String(t);
    return d.innerHTML;
  }

  var _notifications = [];
  var _identityPromise = null;

  function loadIdentity() {
    if (!_identityPromise) {
      _identityPromise = fetch('/api/me', { credentials: 'include', cache: 'no-store' })
        .then(function(r) { return r.ok ? r.json() : null; })
        .catch(function() { return null; });
    }
    return _identityPromise;
  }

  function b64ToUint8(b64) {
    var pad = '='.repeat((4 - b64.length % 4) % 4);
    var raw = atob((b64 + pad).replace(/-/g, '+').replace(/_/g, '/'));
    return Uint8Array.from(Array.prototype.map.call(raw, function(c) { return c.charCodeAt(0); }));
  }

  async function ensurePushSubscription(askPermission) {
    if (!window.isSecureContext || !('Notification' in window) ||
        !('serviceWorker' in navigator) || !('PushManager' in window)) return false;
    var permission = Notification.permission;
    if (permission === 'default' && askPermission) permission = await Notification.requestPermission();
    if (permission !== 'granted') return false;

    var keyResponse = await fetch('/api/push/vapid-key', { credentials: 'include', cache: 'no-store' });
    if (!keyResponse.ok) throw new Error('Notification service unavailable');
    var keyData = await keyResponse.json();
    if (!keyData.publicKey) throw new Error('Notification service is not configured');

    var registration = await navigator.serviceWorker.getRegistration('/');
    if (!registration) registration = await navigator.serviceWorker.register('/sw.js?v=42', { scope: '/', updateViaCache: 'none' });
    await navigator.serviceWorker.ready;
    var subscription = await registration.pushManager.getSubscription();
    if (!subscription) {
      subscription = await registration.pushManager.subscribe({
        userVisibleOnly: true,
        applicationServerKey: b64ToUint8(keyData.publicKey)
      });
    }
    var saveResponse = await fetch('/api/push/subscribe', {
      method: 'POST',
      credentials: 'include',
      headers: { 'Content-Type': 'application/json', 'X-Mitch-Requested-With': '1' },
      body: JSON.stringify(subscription)
    });
    if (!saveResponse.ok) throw new Error('Could not save notification settings');
    var bell = document.getElementById('sw-notif-btn');
    if (bell) {
      bell.classList.add('push-enabled');
      bell.title = 'Notifications enabled';
      bell.setAttribute('aria-label', 'Notifications enabled');
    }
    return true;
  }

  function removePushPrompt() {
    var prompt = document.getElementById('sw-push-prompt');
    if (!prompt) return;
    prompt.classList.remove('show');
    setTimeout(function() { if (prompt.parentNode) prompt.remove(); }, 230);
  }

  function showPushPrompt() {
    if (document.getElementById('sw-push-prompt')) return;
    if (window.Notification && Notification.permission !== 'default') return;
    var prompt = document.createElement('div');
    prompt.id = 'sw-push-prompt';
    prompt.setAttribute('role', 'dialog');
    prompt.setAttribute('aria-label', 'Enable Mitch.pro notifications');
    prompt.innerHTML =
      '<span class="sw-push-mark" aria-hidden="true">&#128276;</span>' +
      '<span class="sw-push-copy"><b>Never miss a call or message</b><span>Enable push alerts for incoming voice/video calls, Matrix chat, and important updates.</span></span>' +
      '<span class="sw-push-actions"><button id="sw-push-later" type="button">Remind later</button><button id="sw-push-enable" type="button">Enable alerts</button></span>';
    document.body.appendChild(prompt);
    requestAnimationFrame(function() { prompt.classList.add('show'); });
    document.getElementById('sw-push-later').onclick = function() {
      try { sessionStorage.setItem('_mitchPushPromptSnooze', String(Date.now() + 180000)); } catch(e) {}
      removePushPrompt();
    };
    document.getElementById('sw-push-enable').onclick = async function() {
      var button = this;
      button.disabled = true;
      button.textContent = 'Enabling...';
      try {
        var enabled = await ensurePushSubscription(true);
        if (enabled) {
          removePushPrompt();
          refreshAlertsButton();
          if (typeof refreshPushUpsell === 'function') refreshPushUpsell();
        } else {
          button.textContent = Notification.permission === 'denied' ? 'Blocked in browser' : 'Try again';
          button.disabled = Notification.permission === 'denied';
        }
      } catch(e) {
        button.disabled = false;
        button.textContent = 'Try again';
        var copy = prompt.querySelector('.sw-push-copy span');
        if (copy) copy.textContent = e.message || 'Could not enable alerts. Please try again.';
      }
    };
  }

  async function setupPushEnrollment() {
    if (!window.isSecureContext || !('Notification' in window) ||
        !('serviceWorker' in navigator) || !('PushManager' in window)) return;
    var identity = await loadIdentity();
    if (!identity || !identity.email) return;
    if (Notification.permission === 'granted') {
      ensurePushSubscription(false).catch(function(){});
      return;
    }
    if (Notification.permission !== 'default') return;
    try {
      var snooze = parseInt(sessionStorage.getItem('_mitchPushPromptSnooze') || '0', 10);
      if (Date.now() < snooze) {
        setTimeout(setupPushEnrollment, Math.max(1000, snooze - Date.now()));
        return;
      }
    } catch(e) {}
    setTimeout(showPushPrompt, 800);
  }

  function showMessageToast(message) {
    if (!message || location.pathname.startsWith('/encrypt') || location.pathname.startsWith('/matrix')) return;
    removePushPrompt();
    var stack = document.getElementById('sw-message-toasts');
    if (!stack) {
      stack = document.createElement('div');
      stack.id = 'sw-message-toasts';
      stack.setAttribute('aria-live', 'polite');
      document.body.appendChild(stack);
    }
    var sender = message.from || 'Someone';
    var title = message.kind === 'group' && message.groupName
      ? sender + ' in ' + message.groupName
      : 'Message from ' + sender;
    var toast = document.createElement('div');
    toast.className = 'sw-message-toast';
    toast.innerHTML = '<span class="sw-message-toast-icon" aria-hidden="true"><img src="/icon-192.png" alt=""></span>' +
      '<span class="sw-message-toast-copy"><b>' + escText(title) + '</b><span>New encrypted message</span></span>' +
      '<a href="/matrix/">Open</a>';
    stack.prepend(toast);
    while (stack.children.length > 3) stack.lastElementChild.remove();
    setTimeout(function() { if (toast.parentNode) toast.remove(); }, 9000);
    var originalTitle = document.title.replace(/^\u2022\s*/, '');
    document.title = '\u2022 ' + originalTitle;
    setTimeout(function() { if (document.title === '\u2022 ' + originalTitle) document.title = originalTitle; }, 9000);
  }

  window.__handleIncomingDm = async function(message) {
    loadNotifications();
    if (!message) return;
    var identity = await loadIdentity();
    var mine = String(identity && identity.email || '').toLowerCase();
    var sender = String(message.from || '').toLowerCase();
    if (!mine || !sender || sender === mine) return;
    showMessageToast(message);
  };

  window.__enableSiteNotifications = function() {
    return ensurePushSubscription(true);
  };

  function injectNotifCSS() {
    if (document.getElementById('sw-notif-styles')) return;
    var s = document.createElement('style');
    s.id = 'sw-notif-styles';
    s.textContent = 
      /* shared top-right toolbar — only when we built the toolbar ourselves.
         A #site-topbar we didn't create (e.g. the home masthead renamed by
         app-shell.js) is a real nav bar and must keep its own layout. */
      '#site-topbar.sw-standalone { position: fixed; top: 10px; right: 12px; z-index: 1000001; display: flex; align-items: center; gap: 6px; overflow: visible; }' +
      '#sw-notif-wrap { position: relative; display: inline-flex; align-items: center; overflow: visible; isolation: isolate; }' +
      '#sw-notif-btn {' +
      '  width: 36px !important; height: 36px !important; min-width: 36px !important; min-height: 36px !important;' +
      '  padding: 0 !important; margin: 0; border-radius: 13px !important;' +
      '  display: flex !important; align-items: center; justify-content: center;' +
      '  background: linear-gradient(145deg,rgba(157,82,246,.28),rgba(22,12,47,.9)); color: #e2b4ff; border: 1px solid rgba(205,143,255,.38);' +
      '  cursor: pointer; box-shadow: inset 0 1px rgba(255,255,255,.13),0 8px 28px rgba(0,0,0,0.35);' +
      '  font-size: 15px; line-height: 1; position: relative; overflow: visible !important;' +
      '  backdrop-filter: blur(10px); flex-shrink: 0; box-sizing: border-box;' +
      '}' +
      '#sw-notif-count {' +
      '  display: none !important; position: absolute !important; top: -2px !important; right: -2px !important;' +
      '  z-index: 2; min-width: 16px !important; width: auto; height: 16px !important; padding: 0 4px !important;' +
      '  margin: 0 !important; border: 2px solid var(--t-bg, #10140c); border-radius: 99px !important;' +
      '  background: #ef4444 !important; color: #fff !important;' +
      '  align-items: center; justify-content: center;' +
      '  font-size: 9px !important; font-weight: 800 !important; line-height: 1 !important;' +
      '  pointer-events: none; box-sizing: border-box; overflow: visible;' +
      '}' +
      '#sw-notif-count.is-visible { display: inline-flex !important; }' +
      '#sw-notif-panel {' +
      '  display: none; position: absolute; top: 44px; right: 0;' +
      '  width: min(370px, calc(100vw - 24px)); max-height: min(470px, calc(100vh - 70px));' +
      '  background: radial-gradient(circle at 90% 0,rgba(183,83,255,.2),transparent 36%),rgba(10,7,25,.97); border: 1px solid rgba(214,173,255,.2);' +
      '  border-radius: 18px; box-shadow: 0 22px 70px rgba(0,0,0,0.65),inset 0 1px rgba(255,255,255,.09);' +
      '  overflow: hidden; backdrop-filter: blur(16px); -webkit-backdrop-filter: blur(16px);' +
      '}' +
      '#sw-notif-panel.show { display: block; }' +
      '.sw-notif-head {' +
      '  display: flex; align-items: center; gap: 8px;' +
      '  padding: 10px 12px; border-bottom: 1px solid rgba(255,255,255,0.08);' +
      '  font-size: .78rem; font-weight: 800; color: #fff;' +
      '}' +
      '.sw-notif-head span { flex: 1; }' +
      '.sw-notif-head button {' +
      '  background: transparent; border: 1px solid rgba(255,255,255,0.15);' +
      '  color: rgba(255,255,255,0.7); border-radius: 6px; padding: 3px 7px;' +
      '  font-size: .7rem; cursor: pointer;' +
      '}' +
      '.sw-notif-head a.sw-notif-settings {' +
      '  background: transparent; border: 1px solid rgba(255,255,255,0.15);' +
      '  color: rgba(255,255,255,0.7); border-radius: 6px; padding: 3px 7px;' +
      '  font-size: .7rem; font-weight: 800; text-decoration: none; cursor: pointer;' +
      '}' +
      '#sw-notif-list { max-height: 335px; overflow-y: auto; padding: 8px; }' +
      '.sw-notif-empty { padding: 18px 10px; text-align: center; color: rgba(255,255,255,0.5); font-size: .8rem; opacity: .65; }' +
      '.sw-notif-item {' +
      '  padding: 9px 10px; border: 1px solid rgba(255,255,255,0.08);' +
      '  border-radius: 13px; background: linear-gradient(145deg,rgba(255,255,255,.055),rgba(255,255,255,.02)); margin-bottom: 7px;' +
      '}' +
      '.sw-notif-title { color: #fff; font-size: .82rem; font-weight: 800; margin-bottom: 3px; }' +
      '.sw-notif-body { color: rgba(255,255,255,0.7); font-size: .78rem; line-height: 1.35; }' +
      '.sw-notif-detail { color: rgba(255,255,255,0.5); opacity: .7; font-size: .72rem; line-height: 1.35; margin-top: 3px; }' +
      '.sw-notif-actions { display: flex; gap: 7px; margin-top: 8px; }' +
      '.sw-notif-actions button, .sw-notif-open {' +
      '  flex: 1; text-align: center; text-decoration: none;' +
      '  background: rgba(255,255,255,0.05); border: 1px solid rgba(255,255,255,0.1);' +
      '  color: #dda4ff; border-radius: 9px; padding: 6px 8px;' +
      '  font-size: .72rem; cursor: pointer;' +
      '}' +
      '#sw-push-prompt {' +
      '  position: fixed; left: 50%; bottom: max(18px, env(safe-area-inset-bottom)); z-index: 2147483000;' +
      '  width: min(520px, calc(100vw - 24px)); box-sizing: border-box; padding: 14px;' +
      '  display: grid; grid-template-columns: 42px minmax(0,1fr) auto; align-items: center; gap: 12px;' +
      '  border: 1px solid rgba(218,178,255,.26); border-radius: 18px;' +
      '  color: #fff; background: linear-gradient(145deg,rgba(34,18,66,.94),rgba(10,7,27,.96));' +
      '  box-shadow: 0 24px 70px rgba(0,0,0,.55), inset 0 1px rgba(255,255,255,.12);' +
      '  backdrop-filter: blur(24px) saturate(145%); -webkit-backdrop-filter: blur(24px) saturate(145%);' +
      '  transform: translate(-50%, 18px); opacity: 0; transition: opacity .22s ease, transform .22s ease;' +
      '}' +
      '#sw-push-prompt.show { opacity: 1; transform: translate(-50%, 0); }' +
      '.sw-push-mark { width:42px; height:42px; display:grid; place-items:center; border-radius:14px;' +
      '  background:linear-gradient(145deg,#985cff,#ef58bd); box-shadow:0 9px 25px rgba(184,75,241,.35); font-size:19px; }' +
      '.sw-push-copy { min-width:0; }' +
      '.sw-push-copy b { display:block; margin-bottom:3px; font:800 .84rem/1.2 system-ui,sans-serif; }' +
      '.sw-push-copy span { display:block; color:rgba(240,230,255,.68); font:500 .73rem/1.35 system-ui,sans-serif; }' +
      '.sw-push-actions { display:flex; gap:7px; }' +
      '.sw-push-actions button { min-height:34px; padding:0 11px !important; border-radius:10px !important;' +
      '  border:1px solid rgba(255,255,255,.15) !important; color:#fff !important; font:700 .7rem/1 system-ui,sans-serif !important; }' +
      '#sw-push-enable { background:linear-gradient(135deg,#8b5cf6,#d946ef) !important; }' +
      '#sw-push-later { background:rgba(255,255,255,.055) !important; }' +
      '#sw-message-toasts { position:fixed; right:14px; bottom:14px; z-index:2147482999; display:grid; gap:8px;' +
      '  width:min(360px,calc(100vw - 28px)); pointer-events:none; }' +
      '.sw-message-toast { pointer-events:auto; display:grid; grid-template-columns:38px minmax(0,1fr) auto; gap:10px; align-items:center;' +
      '  padding:11px; border:1px solid rgba(219,181,255,.24); border-radius:16px; color:#fff;' +
      '  background:linear-gradient(145deg,rgba(35,18,69,.95),rgba(9,6,25,.96)); box-shadow:0 18px 52px rgba(0,0,0,.5);' +
      '  backdrop-filter:blur(22px); -webkit-backdrop-filter:blur(22px); animation:swToastIn .22s ease both; }' +
      '.sw-message-toast-icon { width:38px;height:38px;display:grid;place-items:center;border-radius:13px;background:linear-gradient(145deg,#d946ef,#7657ff);font-size:17px; }' +
      '.sw-message-toast-copy { min-width:0; } .sw-message-toast-copy b,.sw-message-toast-copy span { display:block;overflow:hidden;text-overflow:ellipsis;white-space:nowrap; }' +
      '.sw-message-toast-copy b { font:800 .8rem/1.2 system-ui,sans-serif; } .sw-message-toast-copy span { margin-top:3px;color:rgba(237,225,255,.68);font:500 .71rem/1.2 system-ui,sans-serif; }' +
      '.sw-message-toast a { padding:8px 10px;border-radius:10px;color:#fff;background:rgba(170,91,255,.18);border:1px solid rgba(210,157,255,.22);text-decoration:none;font:750 .68rem/1 system-ui,sans-serif; }' +
      '#sw-notif-btn { background:color-mix(in srgb,var(--t-ac) 15%,var(--t-bg2)) !important;color:var(--t-ac) !important;border-color:color-mix(in srgb,var(--t-ac) 38%,var(--t-bd)) !important;box-shadow:inset 0 1px #ffffff12 !important; }' +
      '#sw-notif-btn[aria-expanded="true"] { background:var(--t-ac) !important;color:var(--t-bg) !important; }' +
      '#sw-notif-panel { width:min(410px,calc(100vw - 20px));background:color-mix(in srgb,var(--t-bg) 96%,transparent) !important;border-color:var(--t-bd) !important;border-radius:16px !important;box-shadow:0 24px 70px #0008,inset 0 1px #ffffff12 !important;color:var(--t-fg) !important; }' +
      '.sw-notif-head { min-height:56px;padding:9px 10px !important;border-color:var(--t-bd) !important;color:var(--t-fg) !important; }' +
      '.sw-notif-head>span { font-size:.92rem; }' +
      '.sw-notif-head button,.sw-notif-head a.sw-notif-settings { min-height:30px;padding:5px 8px !important;border-color:var(--t-bd) !important;border-radius:8px !important;color:var(--t-fg2) !important;background:color-mix(in srgb,var(--t-bg3) 55%,transparent) !important; }' +
      '#sw-notif-list { max-height:min(430px,calc(100dvh - 140px));padding:8px !important; }' +
      '.sw-notif-empty { min-height:150px;display:grid;place-items:center;color:var(--t-fg2) !important;font-size:.85rem !important; }' +
      '.sw-notif-item { display:grid;grid-template-columns:38px minmax(0,1fr);column-gap:10px;padding:10px !important;border-color:var(--t-bd) !important;border-radius:13px !important;background:color-mix(in srgb,var(--t-bg2) 85%,transparent) !important; }' +
      '.sw-notif-type { grid-row:1/5;width:38px;height:38px;display:grid;place-items:center;border-radius:11px;background:color-mix(in srgb,var(--t-ac) 16%,var(--t-bg3));color:var(--t-ac);font-size:16px; }' +
      '.sw-notif-title { color:var(--t-fg) !important;font-size:.84rem !important; }' +
      '.sw-notif-body { color:var(--t-fg2) !important;font-size:.76rem !important; }' +
      '.sw-notif-detail { color:var(--t-fg2) !important;font-size:.72rem !important; }' +
      '.sw-notif-age { margin-left:6px;color:var(--t-fg2);font-size:.66rem;font-weight:600;white-space:nowrap; }' +
      '.sw-notif-actions { grid-column:2; }' +
      '.sw-notif-actions button,.sw-notif-open { min-height:31px;border-color:var(--t-bd) !important;border-radius:8px !important;background:color-mix(in srgb,var(--t-bg3) 62%,transparent) !important;color:var(--t-fg) !important;font-weight:700; }' +
      '.sw-notif-open { background:var(--t-ac) !important;color:var(--t-bg) !important;border-color:var(--t-ac) !important; }' +
      '.sw-message-toast { border-color:var(--t-bd) !important;background:color-mix(in srgb,var(--t-bg) 95%,transparent) !important;color:var(--t-fg) !important; }' +
      '.sw-message-toast-icon { background:color-mix(in srgb,var(--t-ac) 20%,var(--t-bg3)) !important; }' +
      '.sw-push-upsell { margin:8px 8px 4px 8px; }' +
      '.sw-upsell-box { display:flex; align-items:center; gap:10px; padding:10px 12px; border-radius:12px; background:linear-gradient(135deg,rgba(168,85,247,.22),rgba(99,102,241,.18)); border:1px solid rgba(192,132,252,.35); font-size:.78rem; color:var(--t-fg,#fff); }' +
      '.sw-upsell-box.is-denied { background:rgba(239,68,68,.12); border-color:rgba(239,68,68,.3); }' +
      '.sw-upsell-text { flex:1; min-width:0; }' +
      '.sw-upsell-text strong { display:block; font-size:.82rem; margin-bottom:2px; }' +
      '.sw-upsell-text span { display:block; font-size:.72rem; opacity:.85; line-height:1.3; }' +
      '#sw-upsell-btn { background:linear-gradient(135deg,#a855f7,#6366f1) !important; color:#fff !important; border:none !important; border-radius:8px !important; padding:6px 12px !important; font-weight:700 !important; font-size:.75rem !important; cursor:pointer; white-space:nowrap; box-shadow:0 4px 12px rgba(168,85,247,.35); }' +
      '@keyframes swToastIn { from { opacity:0; transform:translateY(10px); } }' +
      '@media(max-width:620px){#sw-notif-panel{position:fixed!important;top:64px!important;left:10px!important;right:10px!important;width:auto!important;max-height:calc(100dvh - 84px)!important}.sw-notif-head{flex-wrap:wrap}.sw-notif-head>span{flex-basis:100%}#sw-push-prompt{grid-template-columns:38px minmax(0,1fr);padding:12px;gap:9px}.sw-push-mark{width:38px;height:38px}.sw-push-actions{grid-column:1/-1}.sw-push-actions button{flex:1}#sw-message-toasts{left:10px;right:10px;bottom:10px;width:auto}.sw-message-toast{grid-template-columns:36px minmax(0,1fr) auto}}' +
      '@media(prefers-reduced-motion:reduce){#sw-push-prompt,.sw-message-toast{transition:none;animation:none}}';
    document.head.appendChild(s);
  }

  function injectNotifHTML() {
    if (document.getElementById('sw-notif-wrap')) return;
    // Mount into the shared topbar when one exists. Pages with the full
    // app topbar get the bell inline (a floating toolbar would cover the
    // account link); only fall back to our own floating toolbar otherwise.
    var topbar = document.getElementById('site-topbar');
    if (!topbar && document.getElementById('app-topbar')) {
      topbar = document.getElementById('app-topbar');
    }
    if (!topbar) {
      topbar = document.createElement('div');
      topbar.id = 'site-topbar';
      topbar.className = 'sw-standalone';
      document.body.appendChild(topbar);
    }
    var wrap = document.createElement('div');
    wrap.id = 'sw-notif-wrap';
    wrap.innerHTML = 
      '<button id="sw-notif-btn" type="button" title="Notifications" aria-label="Notifications" aria-expanded="false" aria-controls="sw-notif-panel"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true"><path d="M18 8a6 6 0 0 0-12 0c0 7-3 7-3 9h18c0-2-3-2-3-9Z"/><path d="M10 21h4M12 1v1"/></svg><span id="sw-notif-count">0</span></button>' +
      '<div id="sw-notif-panel">' +
      '  <div class="sw-notif-head">' +
      '    <span>Notifications</span>' +
      '    <button id="sw-notif-manage" type="button" title="Turn message alerts on or off">Alerts: …</button>' +
      '    <a class="sw-notif-settings" href="/notifications/" title="Full notification settings">Settings</a>' +
      '    <button id="sw-notif-read-all" type="button">Read all</button>' +
      '    <button id="sw-notif-close" type="button">Close</button>' +
      '  </div>' +
      '  <div id="sw-push-upsell"></div>' +
      '  <div id="sw-notif-list"><div class="sw-notif-empty">No unread notifications</div></div>' +
      '</div>';
    topbar.appendChild(wrap);
  }

  function refreshPushUpsell() {
    var upsell = document.getElementById('sw-push-upsell');
    if (!upsell) return;
    if (!('Notification' in window) || !window.isSecureContext) {
      upsell.innerHTML = '';
      return;
    }
    if (Notification.permission === 'default') {
      upsell.innerHTML =
        '<div class="sw-upsell-box">' +
        '  <span class="sw-upsell-text"><strong>🔔 Turn on push alerts</strong><span>Never miss incoming calls or messages when away.</span></span>' +
        '  <button id="sw-upsell-btn" type="button">Turn on</button>' +
        '</div>';
      var btn = document.getElementById('sw-upsell-btn');
      if (btn) {
        btn.onclick = async function() {
          btn.disabled = true;
          btn.textContent = 'Enabling...';
          await window.__enableSiteNotifications();
          refreshPushUpsell();
          refreshAlertsButton();
        };
      }
    } else if (Notification.permission === 'denied') {
      upsell.innerHTML =
        '<div class="sw-upsell-box is-denied">' +
        '  <span class="sw-upsell-text"><strong style="color:#f87171;">⚠️ Alerts Blocked</strong><span>Allow notifications in site settings to receive call rings.</span></span>' +
        '</div>';
    } else {
      upsell.innerHTML = '';
    }
  }

  async function loadNotifications() {
    try {
      var r = await fetch('/api/me/notifications', { credentials: 'include' });
      if (!r.ok) return;
      var d = await r.json();
      _notifications = d.notifications || [];
      renderNotifications();
    } catch(e) {}
  }

  function renderNotifications() {
    var count = _notifications.length;
    var badge = document.getElementById('sw-notif-count');
    var list = document.getElementById('sw-notif-list');
    if (!badge || !list) return;
    badge.textContent = count > 99 ? '99+' : String(count);
    badge.classList.toggle('is-visible', count > 0);
    var readAll = document.getElementById('sw-notif-read-all');
    if (readAll) readAll.disabled = count === 0;
    refreshPushUpsell();
    if (!count) {
      list.innerHTML = '<div class="sw-notif-empty">No unread notifications</div>';
      return;
    }
    function age(ts) {
      var seconds = Math.max(0, Math.floor((Date.now() - Number(ts || 0)) / 1000));
      if (seconds < 60) return 'now';
      if (seconds < 3600) return Math.floor(seconds / 60) + 'm';
      if (seconds < 86400) return Math.floor(seconds / 3600) + 'h';
      return Math.floor(seconds / 86400) + 'd';
    }
    var icons = {
      dm: '&#9993;',
      group_dm: '&#9783;',
      coin_gift: '&#9733;',
      admin_notice: '&#9888;',
      vm_admin_access: '&#128187;',
      matrix: '&#128172;',
      matrix_call: '&#128222;',
      matrix_invite: '&#128233;'
    };
    list.innerHTML = _notifications.map(function(n, i) {
      var url = '';
      try {
        if (!String(n.url || '').trim()) throw new Error('No destination');
        var candidate = new URL(String(n.url), location.origin);
        if (candidate.origin === location.origin || candidate.origin === 'https://mitch.pro' || candidate.origin === 'https://mitchdog.com') {
          url = candidate.pathname + candidate.search + candidate.hash;
        }
      } catch (_) {}
      var open = url ? '<a class="sw-notif-open" data-open="' + i + '" href="' + escText(url) + '">Open</a>' : '';
      return '<div class="sw-notif-item">' +
        '<span class="sw-notif-type" aria-hidden="true">' + (icons[n.type] || '&#128276;') + '</span>' +
        '<div class="sw-notif-title">' + escText(n.title) + '<span class="sw-notif-age">' + age(n.ts) + '</span></div>' +
        '<div class="sw-notif-body">' + escText(n.body) + '</div>' +
        (n.detail ? '<div class="sw-notif-detail">' + escText(n.detail) + '</div>' : '') +
        '<div class="sw-notif-actions">' + open + '<button type="button" data-i="' + i + '">Mark read</button></div>' +
      '</div>';
    }).join('');
    list.querySelectorAll('button[data-i]').forEach(function(btn) {
      btn.onclick = function() { markNotificationRead(_notifications[Number(btn.dataset.i)]); };
    });
    list.querySelectorAll('a[data-open]').forEach(function(link) {
      link.onclick = function() { markNotificationRead(_notifications[Number(link.dataset.open)], true); };
    });
  }

  async function markNotificationRead(n, navigating) {
    if (!n) return;
    var body = (n.type === 'coin_gift' || n.type === 'admin_notice' || n.type === 'vm_admin_access')
      ? { coinGiftIds: [n.id] }
      : n.type === 'group_dm'
        ? { groupIds: [n.groupId] }
      : n.type === 'dm'
        ? { dmFroms: [n.from] }
      : (n.type === 'matrix' || n.type === 'matrix_call' || n.type === 'matrix_invite')
        ? { matrixIds: [n.id], matrixRoomIds: [n.matrixRoomId] }
        : {};
    _notifications = _notifications.filter(function(item) { return item !== n; });
    renderNotifications();
    await fetch('/api/me/notifications/read', {
      method: 'POST',
      credentials: 'include',
      keepalive: !!navigating,
      headers: { 'Content-Type': 'application/json', 'X-Mitch-Requested-With': '1' },
      body: JSON.stringify(body),
    }).catch(function(){});
    loadNotifications();
  }

  async function markAllNotificationsRead() {
    if (!_notifications.length) return;
    _notifications = [];
    renderNotifications();
    await fetch('/api/me/notifications/read', {
      method: 'POST',
      credentials: 'include',
      headers: { 'Content-Type': 'application/json', 'X-Mitch-Requested-With': '1' },
      body: JSON.stringify({ all: true }),
    }).catch(function(){});
  }

  // ── Alerts manager: one button to turn push alerts on/off ───────────────
  async function refreshAlertsButton() {
    var btn = document.getElementById('sw-notif-manage');
    if (!btn) return;
    var on = false;
    try {
      var reg = await navigator.serviceWorker.getRegistration('/');
      var sub = reg && await reg.pushManager.getSubscription();
      on = !!sub;
    } catch (e) {}
    btn.textContent = on ? 'Alerts: on' : 'Alerts: off';
    btn.dataset.state = on ? 'on' : 'off';
    if (!('Notification' in window) || !window.isSecureContext) {
      btn.disabled = true;
      btn.textContent = 'Alerts: n/a';
    }
  }
  async function toggleAlerts() {
    var btn = document.getElementById('sw-notif-manage');
    if (!btn) return;
    btn.disabled = true;
    try {
      var reg = await navigator.serviceWorker.getRegistration('/');
      var sub = reg && await reg.pushManager.getSubscription();
      if (sub) {
        // Off: drop the browser subscription and the server copy of it.
        await fetch('/api/push/unsubscribe', {
          method: 'POST', credentials: 'include',
          headers: { 'Content-Type': 'application/json', 'X-Mitch-Requested-With': '1' },
        }).catch(function () {});
        try { await sub.unsubscribe(); } catch (e) {}
      } else {
        var enabled = await window.__enableSiteNotifications();
        if (!enabled) { btn.textContent = 'Alerts: blocked'; btn.dataset.state = 'off'; btn.disabled = false; return; }
      }
      await refreshAlertsButton();
    } catch (e) {}
    btn.disabled = false;
  }

  function init() {
    // Don't show on appeal page
    if (location.pathname.endsWith('/appeal.html')) return;
    
    injectNotifCSS();
    injectNotifHTML();

    // On encrypt page: move topbar into the sidebar-top bar to avoid covering chat tools
    if (window.location.pathname.startsWith('/encrypt')) {
      function relocateToSidebar() {
        var sidebarTop = document.getElementById('sidebar-top');
        var topbar = document.getElementById('site-topbar');
        if (sidebarTop && topbar) {
          topbar.style.cssText = 'position:relative;top:auto;right:auto;z-index:100;display:inline-flex;align-items:center;gap:6px;';
          sidebarTop.appendChild(topbar);
          // Keep the notification panel fixed so it opens without clipping
          var panel = document.getElementById('sw-notif-panel');
          if (panel) {
            panel.style.position = 'fixed';
            panel.style.top = '50px';
            panel.style.left = '8px';
            panel.style.right = 'auto';
          }
        }
      }
      // sidebar-top may not exist yet if app hasn't rendered; try now and after short delay
      relocateToSidebar();
      setTimeout(relocateToSidebar, 500);
    }

    var btn = document.getElementById('sw-notif-btn');
    var panel = document.getElementById('sw-notif-panel');
    if (!btn || !panel) return;

    document.getElementById('sw-notif-close').onclick = function() {
      panel.classList.remove('show');
      btn.setAttribute('aria-expanded', 'false');
      btn.focus();
    };
    document.getElementById('sw-notif-manage').onclick = toggleAlerts;
    refreshAlertsButton();
    document.getElementById('sw-notif-read-all').onclick = markAllNotificationsRead;
    btn.onclick = function(e) {
      e.stopPropagation();
      panel.classList.toggle('show');
      btn.setAttribute('aria-expanded', String(panel.classList.contains('show')));
      if (panel.classList.contains('show')) { loadNotifications(); refreshAlertsButton(); }
    };
    panel.onclick = function(e) { e.stopPropagation(); };
    document.addEventListener('click', function() { panel.classList.remove('show'); btn.setAttribute('aria-expanded', 'false'); });
    document.addEventListener('keydown', function(event) {
      if (event.key === 'Escape' && panel.classList.contains('show')) {
        panel.classList.remove('show');
        btn.setAttribute('aria-expanded', 'false');
        btn.focus();
      }
    });
    loadNotifications();
    setupPushEnrollment();
    window.__refreshNotifications = loadNotifications;
    setInterval(loadNotifications, 30000);
  }

  if (document.body) init();
  else document.addEventListener('DOMContentLoaded', init);
})();
