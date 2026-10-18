(() => {
  const prefix = '/proxy/rivestream';
  function proxyUrl(value) {
    if (typeof value === 'string' && value.startsWith('#')) return value;
    let url;
    try { url = new URL(value, document.baseURI); } catch (_) { return value; }
    if (url.origin === 'https://www.rivestream.app') {
      return prefix + url.pathname + url.search + url.hash;
    }
    if (url.origin === location.origin && url.pathname !== prefix && !url.pathname.startsWith(prefix + '/')) {
      return prefix + url.pathname + url.search + url.hash;
    }
    return value;
  }
  const fetch = window.fetch;
  window.fetch = (input, options) => {
    if (input instanceof Request) {
      const url = proxyUrl(input.url);
      if (url !== input.url) input = new Request(url, input);
    } else {
      input = proxyUrl(input);
    }
    return fetch.call(window, input, options);
  };
  const open = XMLHttpRequest.prototype.open;
  XMLHttpRequest.prototype.open = function(method, url, ...options) {
    return open.call(this, method, proxyUrl(url), ...options);
  };
  const setAttribute = Element.prototype.setAttribute;
  Element.prototype.setAttribute = function(name, value) {
    if (['src', 'href', 'action', 'poster'].includes(String(name).toLowerCase())) value = proxyUrl(value);
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
  // The upstream PWA must not register a service worker on Mitch's origin.
  try { delete Object.getPrototypeOf(navigator).serviceWorker; } catch (_) {}
})();
