const CACHE_NAME = 'mitch-pro-cache-v55';
const ASSETS = [
  '/favicon.ico',
  '/manifest.json',
  '/apple-touch-icon.png',
  '/icon-192.png',
  '/icon-512.png',
  '/cookie-consent.js',
  '/app.css',
  '/home.css?v=12',
  '/popup.js',
  '/pwa-install.js',
  '/site-theme.js'
];

self.addEventListener('install', (e) => {
  e.waitUntil(
    caches.open(CACHE_NAME).then(async (cache) => {
      await Promise.allSettled(
        ASSETS.map((asset) => cache.add(asset).catch(() => {}))
      );
    }).then(() => self.skipWaiting())
  );
});

self.addEventListener('activate', (e) => {
  e.waitUntil(
    caches.keys().then((keys) => {
      return Promise.all(
        keys.map((key) => {
          if (key !== CACHE_NAME) {
            return caches.delete(key);
          }
        })
      );
    }).then(() => self.clients.claim())
  );
});

// Pages can push a version bump through immediately: postMessage({type:'SKIP_WAITING'})
// moves a waiting worker into control (activate's cleanup then purges old caches).
self.addEventListener('message', (e) => {
  if (e.data && e.data.type === 'SKIP_WAITING') self.skipWaiting();
  if (e.data && e.data.type === 'ACCOUNT_BANNED') {
    e.waitUntil(Promise.all([
      caches.keys().then(keys => Promise.all(keys.map(key => caches.delete(key)))),
      clients.matchAll({type:'window',includeUncontrolled:true}).then(windows => Promise.all(windows.filter(client => !/^\/appeal(?:\/|\.html|$)/.test(new URL(client.url).pathname)).map(client => client.navigate('/').catch(() => {}))))
    ]));
  }
});

function isHtmlRequest(request) {
  const url = request.url;
  const accept = request.headers.get('accept') || '';
  // Navigation requests or accept: text/html
  if (request.mode === 'navigate') return true;
  if (accept.includes('text/html')) return true;
  // URLs ending with / or .html
  const pathname = new URL(url).pathname;
  if (pathname.endsWith('/') || pathname.endsWith('.html')) return true;
  return false;
}

self.addEventListener('fetch', (e) => {
  // Cache Storage only supports GET. Let POST/PUT/etc. go directly to the
  // network so form submissions and API calls can never reach cache.put().
  if (e.request.method !== 'GET') return;

  const requestUrl = new URL(e.request.url);
  // A service worker controls its pages' cross-origin subrequests too. Never
  // proxy those through our cache: doing so can turn CORP rejections into
  // network-error responses returned by the worker.
  if (requestUrl.origin !== self.location.origin) return;

  // Admin panel, Matrix media and sync, game proxy, and APIs carry credentials and must never enter a shared cache.
  if (requestUrl.pathname.startsWith('/admin') || requestUrl.pathname.startsWith('/api/') || requestUrl.pathname.startsWith('/_matrix/') || requestUrl.pathname.startsWith('/proxy/') || e.request.url.startsWith('ws')) {
    return;
  }

const isThemeJs = requestUrl.pathname === '/theme.js';

  if (isThemeJs) {
    // Force a fresh fetch by using cache: 'no-store' and a timestamp parameter
    e.respondWith(
      fetch(e.request.url + '?t=' + Date.now(), { cache: 'no-store' }).then((response) => {
        if (response && response.status === 200) {
          // Store under the original request URL so caches.match(e.request) still resolves offline
          const clone = response.clone();
          caches.open(CACHE_NAME).then((cache) => cache.put(e.request, clone));
        }
        return response;
      }).catch(async () => {
        const cached = await caches.match(e.request);
        return cached || new Response('', { status: 504, statusText: 'Gateway Timeout' });
      })
    );
  } else if (isHtmlRequest(e.request)) {
    // Page access must be checked by the server, including when offline.
    e.respondWith(fetch(e.request, {cache:'no-store'}).catch(() => new Response(
      '<!doctype html><html lang="en"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Offline</title><body><p>Connect to the internet to load this page.</p></body></html>',
      {status:503,headers:{'Content-Type':'text/html','Cache-Control':'no-store'}}
    )));
  } else {
    const pathname = requestUrl.pathname;
    const isCode = /\.(css|js|mjs|json)(\?|$)/.test(pathname);
    if (isCode) {
      // Network-first for code assets: never serve stale CSS/JS when online,
      // fall back to the cache only when offline.
      e.respondWith(
        fetch(e.request).then((response) => {
          if (response && response.status === 200 && response.type === 'basic') {
            const clone = response.clone();
            caches.open(CACHE_NAME).then((cache) => cache.put(e.request, clone));
          }
          return response;
        }).catch(async () => {
          const cached = await caches.match(e.request);
          return cached || new Response('', { status: 504, statusText: 'Gateway Timeout' });
        })
      );
    } else {
      // Cache-first for slow-changing assets (images, fonts, media)
      e.respondWith(
        caches.match(e.request).then((cachedResponse) => {
          if (cachedResponse) {
            return cachedResponse;
          }
          return fetch(e.request).then((response) => {
            if (response && response.status === 200 && response.type === 'basic') {
              const responseToCache = response.clone();
              caches.open(CACHE_NAME).then((cache) => {
                cache.put(e.request, responseToCache);
              });
            }
            return response;
          }).catch(() => {
            return new Response('', { status: 504, statusText: 'Gateway Timeout' });
          });
        })
      );
    }
  }
});

// Push notification listeners
// True when a window client on the active chat surface (/encrypt/ or /matrix/) is actually
// on screen right now — in that case the page shows its own in-app view and a system
// notification would be redundant (and annoying mid-conversation).
async function isUserInActiveChat(targetUrl) {
  try {
    const target = new URL(notificationTargetUrl(targetUrl), self.location.origin);
    if (!/^\/(matrix|encrypt)(?:\/|$)/.test(target.pathname)) return false;
    function conversationKey(url) {
      if (url.pathname.startsWith('/encrypt')) return 'encrypt:' + (url.searchParams.get('group') ? 'group:' + url.searchParams.get('group') : 'dm:' + (url.searchParams.get('to') || '').toLowerCase());
      const roomPath = url.pathname.match(/^\/matrix\/(?:home|direct)\/([^/]+)/);
      if (roomPath) return 'matrix:' + roomPath[1];
      const room = url.hash.match(/^#\/room\/([^/]+)/);
      return room ? 'matrix:' + room[1] : url.pathname + url.search + url.hash;
    }
    const windows = await clients.matchAll({ type:'window', includeUncontrolled:true });
    return windows.some(client => {
      if (client.visibilityState !== 'visible' || !client.focused) return false;
      const current = new URL(client.url);
      return current.origin === self.location.origin && conversationKey(current) === conversationKey(target);
    });
  } catch { return false; }
}

// The wolf logo on rjuhsd.school, the mitch mark everywhere else.
const notifyIcon = () => (self.location.hostname.endsWith('rjuhsd.school') ? '/rjuhsd-assets/icon-192.png' : '/icon-192.png');

self.addEventListener('push', e => {
  let data = { title: 'New message', body: '', url: '/matrix/' };
  try { data = Object.assign(data, JSON.parse(e.data.text())); } catch {}
  const isCall = (data.tag && data.tag.startsWith('matrix-call')) || data.type === 'call';

  e.waitUntil(isUserInActiveChat(data.url).then(async inChat => {
    // Already inside active chat on this device — stay quiet, UNLESS it's an incoming call
    if (inChat && !isCall) return;

    const target = notificationTargetUrl(data.url);
    const tag = data.tag || data.id || 'mitch:' + target;
    const existing = await self.registration.getNotifications({ tag });
    const eventId = String(data.id || data.eventId || '');
    if (eventId && existing.some(notice => notice.data?.eventId === eventId)) return;
    return self.registration.showNotification(data.title, {
      body: data.body,
      icon: notifyIcon(),
      badge: notifyIcon(),
      tag,
      renotify: false,
      vibrate: isCall ? [300, 100, 300, 100, 300, 100, 600] : [90, 45, 90],
      requireInteraction: isCall || Boolean(data.requireInteraction),
      actions: isCall ? [
        { action: 'answer', title: '📞 Join Call' },
        { action: 'decline', title: 'Dismiss' }
      ] : [
        { action: 'open', title: 'Open' }
      ],
      data: { url: target, eventId, type: data.type || (isCall ? 'call' : 'message') }
    });
  }));
});

// Keep notification clicks on the origin the PWA was installed from: resolve
// the payload URL against this SW's own scope, and if an old/absolute payload
// points at a different host (mitch.pro, mitchdog.com, …), strip it down to
// its path so we never navigate the rjuhsd.school PWA to a foreign origin
// that would demand a fresh login.
function notificationTargetUrl(raw) {
  try {
    const resolved = new URL(String(raw || '/notifications/'), self.registration.scope);
    if (!['https:','http:'].includes(resolved.protocol) || resolved.username || resolved.password) return '/notifications/';
    if (resolved.origin !== self.location.origin && !['mitchdog.com','mitch.pro','rjuhsd.school'].includes(resolved.hostname)) return '/notifications/';
    return resolved.pathname + resolved.search + resolved.hash;
  } catch { return '/notifications/'; }
}

self.addEventListener('notificationclick', e => {
  e.notification.close();
  if (e.action === 'decline') return;

  const url = notificationTargetUrl(e.notification.data?.url);
  e.waitUntil(clients.matchAll({ type: 'window', includeUncontrolled: true }).then(async cs => {
    const local = cs.filter(c => new URL(c.url).origin === self.location.origin);
    local.sort((a,b) => Number(b.focused) - Number(a.focused));
    for (const client of local) {
      if (!('navigate' in client)) continue;
      try { const next = await client.navigate(new URL(url,self.location.origin).href); if (next) await next.focus(); return; } catch {}
    }
    return clients.openWindow(url);
  }));
});
