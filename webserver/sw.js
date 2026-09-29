const CACHE_NAME = 'mitch-pro-cache-v45';
const ASSETS = [
  '/favicon.ico',
  '/manifest.json',
  '/apple-touch-icon.png',
  '/icon-192.png',
  '/icon-512.png',
  '/cookie-consent.js',
  '/relaunch.css',
  '/portal-redesign.css?v=16',
  '/home-redesign.css?v=2',
  '/popup.js',
  '/pwa-install.js'
];

self.addEventListener('install', (e) => {
  e.waitUntil(
    caches.open(CACHE_NAME).then((cache) => {
      return cache.addAll(ASSETS);
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

  // Admin panel, Matrix media and sync, and APIs carry credentials and must never enter a shared cache.
  if (requestUrl.pathname.startsWith('/admin') || requestUrl.pathname.startsWith('/api/') || requestUrl.pathname.startsWith('/_matrix/') || e.request.url.startsWith('ws')) {
    return;
  }

// Let the admin broadcast video stream directly from the network. Service
// Worker cache.put can fail on large authenticated/range media responses.
if (requestUrl.pathname === '/media/admin-jumpscare-krupp-1935.webm') {
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
    // Network-first for HTML pages — always get fresh content
    e.respondWith(
      fetch(e.request).then((response) => {
        if (response && response.status === 200 && response.type === 'basic') {
          const clone = response.clone();
          caches.open(CACHE_NAME).then((cache) => cache.put(e.request, clone));
        }
        return response;
      }).catch(async () => {
        // Offline fallback: serve cached version if available
        const cached = await caches.match(e.request);
        return cached || new Response('<!DOCTYPE html><html><body>Offline</body></html>', {
          status: 503,
          headers: { 'Content-Type': 'text/html' }
        });
      })
    );
  } else {
    const pathname = requestUrl.pathname;
    const isCode = /\.(css|js|mjs|json)(\?|$)/.test(pathname) || pathname === '/readability.css';
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
    const cs = await clients.matchAll({ type: 'window', includeUncontrolled: true });
    const isMatrix = typeof targetUrl === 'string' && targetUrl.includes('/matrix');
    return cs.some(c => {
      try {
        if (!c.url || !c.url.startsWith(self.location.origin)) return false;
        if (c.visibilityState !== 'visible') return false;
        const p = new URL(c.url).pathname;
        if (isMatrix) return p.startsWith('/matrix');
        return p.startsWith('/encrypt');
      } catch { return false; }
    });
  } catch { return false; }
}

// The wolf logo on rjuhsd.school, the mitch mark everywhere else.
const notifyIcon = () => (self.location.hostname.endsWith('rjuhsd.school') ? '/rjuhsd-assets/icon-192.png' : '/icon-192.png');

self.addEventListener('push', e => {
  let data = { title: 'New message', body: '', url: '/matrix/' };
  try { data = Object.assign(data, JSON.parse(e.data.text())); } catch {}
  const isCall = (data.tag && data.tag.startsWith('matrix-call')) || data.type === 'call';

  e.waitUntil(isUserInActiveChat(data.url).then(inChat => {
    // Already inside active chat on this device — stay quiet, UNLESS it's an incoming call
    if (inChat && !isCall) return;

    return self.registration.showNotification(data.title, {
      body: data.body,
      icon: notifyIcon(),
      badge: notifyIcon(),
      tag: data.tag || undefined,
      renotify: Boolean(data.tag),
      vibrate: isCall ? [300, 100, 300, 100, 300, 100, 600] : [90, 45, 90],
      requireInteraction: isCall || Boolean(data.requireInteraction),
      actions: isCall ? [
        { action: 'answer', title: '📞 Join Call' },
        { action: 'decline', title: 'Dismiss' }
      ] : [
        { action: 'open', title: 'Open Chat' }
      ],
      data: { url: data.url, type: data.type || (isCall ? 'call' : 'message') }
    });
  }));
});

// Keep notification clicks on the origin the PWA was installed from: resolve
// the payload URL against this SW's own scope, and if an old/absolute payload
// points at a different host (mitch.pro, mitchdog.com, …), strip it down to
// its path so we never navigate the rjuhsd.school PWA to a foreign origin
// that would demand a fresh login.
function notificationTargetUrl(raw) {
  let u = String(raw || '/matrix/');
  try {
    const resolved = new URL(u, self.registration.scope);
    if (resolved.origin !== self.location.origin) {
      return resolved.pathname + resolved.search + resolved.hash || '/matrix/';
    }
    return resolved.href;
  } catch {
    return '/matrix/';
  }
}

self.addEventListener('notificationclick', e => {
  e.notification.close();
  if (e.action === 'decline') return;

  const url = notificationTargetUrl(e.notification.data?.url);
  e.waitUntil(clients.matchAll({ type: 'window', includeUncontrolled: true }).then(async cs => {
    // Hand the URL to an existing app window and let it present the target
    // in its in-app browser sheet (Apple-style), instead of navigating the
    // whole PWA window away from whatever the user had open.
    for (const c of cs) {
      if (!c.url.startsWith(self.location.origin) || !('focus' in c)) continue;
      try { await c.focus(); } catch {}
      if (url.includes('/matrix')) {
        if ('navigate' in c) {
          try { await c.navigate(url); return c; } catch {}
        }
      } else {
        c.postMessage({ type: 'open-in-app-browser', url });
        return c;
      }
    }
    return clients.openWindow(url);
  }));
});
