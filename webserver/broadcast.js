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
      if (data.type === 'account_banned') {
        if (window.MitchBanGuard) window.MitchBanGuard.showBan();
        else location.replace('/');
        return;
      }
      if (data.type === 'admin_broadcast' && data.createdAt && Number(data.createdAt) < pageOpenedAt) {
        return;
      }
      if (data.type === 'admin_broadcast') {
        if (!hasSeenBroadcast(data)) showBroadcast(data.message);
      } else if (data.type === 'refresh_notifications') {
        if (typeof window.__refreshNotifications === 'function') {
          window.__refreshNotifications();
        }
      } else if (data.type === 'new_dm') {
        if (typeof window.__handleIncomingDm === 'function') {
          window.__handleIncomingDm(data.message || null);
        }
      }
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
      pollLatestBroadcast();
      fallbackTimer = setInterval(pollLatestBroadcast, 3000);
      document.addEventListener('visibilitychange', function() {
        sendPresencePing();
        if (!document.hidden) { pollLatestBroadcast(); }
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
    return d.innerHTML.replace(/"/g, '&quot;').replace(/'/g, '&#39;');
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
    if (!registration) registration = await navigator.serviceWorker.register('/sw.js?v=48', { scope: '/', updateViaCache: 'none' });
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
    // Permissions are offered inside Notifications when the user opens it.

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
    var sheet = document.createElement('link');
    sheet.id = 'sw-notif-styles'; sheet.rel = 'stylesheet'; sheet.href = '/notifications/panel.css?v=1';
    document.head.appendChild(sheet);
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
      '<section id="sw-notif-panel" aria-label="Notifications">' +
      '  <div class="sw-notif-head">' +
      '    <span>Notifications</span>' +
      '    <button id="sw-notif-close" type="button" aria-label="Close notifications">×</button></div><div class="sw-notif-tools">' +
      '    <button id="sw-notif-manage" type="button" title="Turn message alerts on or off">Alerts: …</button>' +
      '    <a class="sw-notif-settings" href="/notifications/" title="Full notification settings">Settings</a>' +
      '    <button id="sw-notif-read-all" type="button">Read all</button>' +
      '  </div>' +
      '  <div id="sw-push-upsell"></div>' +
      '  <div id="sw-notif-list"><div class="sw-notif-empty">No unread notifications</div></div>' +
      '</section>';
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
        '  <span class="sw-upsell-text"><strong>Message alerts</strong><span>Get a browser notification for calls and messages.</span></span>' +
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
        '  <span class="sw-upsell-text"><strong>Browser alerts are off</strong><span>You can still read notifications here. Change your browser’s site permissions to enable alerts.</span></span>' +
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
        if (candidate.origin === location.origin || candidate.origin === 'https://mitchdog.com' || candidate.origin === 'https://mitch.pro') {
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
