// Runs first inside every proxied movy.sx page (injected by routes/movies.rs).
// The site was built to live at the root of its own origin and talks to three
// API hosts straight from the browser. This script keeps all of that inside the
// /proxy/movies mount, so every request goes through Mitch (and out the VPN
// interface) instead of leaving the visitor's browser directly.
(() => {
  const prefix = '/proxy/movies';
  const siteOrigin = 'https://www.movy.sx';
  const apiHosts = ['backend.movy.sx', 'db.wecollege.net', 'api.wecollege.net', 'trailers.wecollege.net', 'ipwho.is'];
  // Hosts the server asked us to leave alone (MOVIES_DIRECT_API_HOSTS).
  const directHosts = new Set(window.MOVIES_DIRECT_HOSTS || []);
  // Mitch's own scripts (presence) live at the real root and must not be moved.
  const mitchOwned = [/^\/presence\.js$/, /^\/api\/presence\//];

  function proxyUrl(value) {
    if (value instanceof URL) value = value.href;
    if (typeof value !== 'string') return value;
    if (value === '' || value.startsWith('#') || /^(data|blob|javascript|mailto|tel):/i.test(value)) return value;
    let url;
    try { url = new URL(value, document.baseURI); } catch (_) { return value; }
    if (url.origin === siteOrigin) {
      return prefix + url.pathname + url.search + url.hash;
    }
    if (url.protocol === 'https:' && apiHosts.includes(url.hostname) && !directHosts.has(url.hostname)) {
      return prefix + '/__api/' + url.hostname + url.pathname + url.search + url.hash;
    }
    if (url.origin === location.origin) {
      const path = url.pathname;
      if (path === prefix || path.startsWith(prefix + '/')) return value;
      if (mitchOwned.some(re => re.test(path))) return value;
      return prefix + path + url.search + url.hash;
    }
    return value;
  }

  // srcset / imagesrcset hold comma-separated "url descriptor" candidates.
  function proxySrcset(value) {
    if (typeof value !== 'string') return value;
    return value.split(',').map(candidate => {
      const lead = candidate.length - candidate.trimStart().length;
      const trimmed = candidate.slice(lead);
      const space = trimmed.search(/\s/);
      const url = space < 0 ? trimmed : trimmed.slice(0, space);
      return candidate.slice(0, lead) + proxyUrl(url) + (space < 0 ? '' : trimmed.slice(space));
    }).join(',');
  }

  const nativeFetch = window.fetch;
  window.fetch = (input, options) => {
    if (input instanceof Request) {
      const url = proxyUrl(input.url);
      if (url !== input.url) input = new Request(url, input);
    } else {
      input = proxyUrl(input);
    }
    return nativeFetch.call(window, input, options);
  };

  const open = XMLHttpRequest.prototype.open;
  XMLHttpRequest.prototype.open = function(method, url, ...rest) {
    return open.call(this, method, proxyUrl(url), ...rest);
  };

  if (navigator.sendBeacon) {
    const beacon = navigator.sendBeacon.bind(navigator);
    navigator.sendBeacon = (url, data) => beacon(proxyUrl(url), data);
  }

  const setAttribute = Element.prototype.setAttribute;
  Element.prototype.setAttribute = function(name, value) {
    const key = String(name).toLowerCase();
    if (['src', 'href', 'action', 'poster'].includes(key)) value = proxyUrl(value);
    else if (key === 'srcset' || key === 'imagesrcset') value = proxySrcset(value);
    return setAttribute.call(this, name, value);
  };

  for (const [type, property] of [
    [HTMLImageElement, 'src'], [HTMLScriptElement, 'src'], [HTMLLinkElement, 'href'],
    [HTMLAnchorElement, 'href'], [HTMLIFrameElement, 'src'], [HTMLMediaElement, 'src'],
    [HTMLSourceElement, 'src'], [HTMLVideoElement, 'poster'], [HTMLFormElement, 'action']
  ]) {
    const descriptor = Object.getOwnPropertyDescriptor(type.prototype, property);
    if (descriptor?.set) Object.defineProperty(type.prototype, property, {
      ...descriptor,
      set(value) { descriptor.set.call(this, proxyUrl(value)); }
    });
  }

  for (const [type, property] of [
    [HTMLImageElement, 'srcset'], [HTMLSourceElement, 'srcset'], [HTMLLinkElement, 'imageSrcset']
  ]) {
    const descriptor = Object.getOwnPropertyDescriptor(type.prototype, property);
    if (descriptor?.set) Object.defineProperty(type.prototype, property, {
      ...descriptor,
      set(value) { descriptor.set.call(this, proxySrcset(value)); }
    });
  }

  // The upstream PWA must not register a service worker on Mitch's origin.
  try { delete Object.getPrototypeOf(navigator).serviceWorker; } catch (_) {}
})();
