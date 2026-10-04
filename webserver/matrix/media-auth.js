(function () {
  const mediaCache = new Map();
  const objectUrls = new Set();
  const mediaPath = /^\/_matrix\/(?:media\/(?:r0|v3)|client\/v1\/media)\/(thumbnail|download)\/([^/?#]+)\/([^/?#]+)/;

  function authenticatedMediaUrl(value) {
    if (!value || value.startsWith('blob:') || value.startsWith('data:')) return null;
    let url;
    if (value.startsWith('mxc://')) {
      const parts = value.slice(6).split('/');
      if (parts.length !== 2 || !parts[0] || !parts[1]) return null;
      url = new URL(`/_matrix/client/v1/media/download/${encodeURIComponent(parts[0])}/${encodeURIComponent(parts[1])}`, location.origin);
    } else {
      try { url = new URL(value, location.href); } catch (_) { return null; }
      if (url.origin !== location.origin) return null;
      const match = url.pathname.match(mediaPath);
      if (!match) return null;
      url.pathname = `/_matrix/client/v1/media/${match[1]}/${match[2]}/${match[3]}`;
    }
    return url.href;
  }

  async function loadMedia(value) {
    const url = authenticatedMediaUrl(value);
    const token = localStorage.getItem('cinny_access_token');
    if (!url || !token) return null;
    const cacheKey = `${token}\n${url}`;
    if (!mediaCache.has(cacheKey)) {
      const request = fetch(url, {
        headers: { Authorization: `Bearer ${token}` },
        credentials: 'same-origin'
      }).then(async response => {
        if (!response.ok) return null;
        const blob = await response.blob();
        if (!blob.type.startsWith('image/')) return null;
        const objectUrl = URL.createObjectURL(blob);
        objectUrls.add(objectUrl);
        return objectUrl;
      }).catch(() => null);
      mediaCache.set(cacheKey, request);
    }
    return mediaCache.get(cacheKey);
  }

  async function repairImage(img) {
    const source = img.getAttribute('src');
    if (!authenticatedMediaUrl(source) || img.dataset.mitchMediaSource === source) return;
    img.dataset.mitchMediaSource = source;
    const replacement = await loadMedia(source);
    if (replacement && img.isConnected && img.dataset.mitchMediaSource === source) {
      img.removeAttribute('srcset');
      img.src = replacement;
    }
  }

  async function repairBackground(element) {
    const style = element.style.backgroundImage;
    const match = style.match(/^url\(["']?(.+?)["']?\)$/);
    if (!match || !authenticatedMediaUrl(match[1]) || element.dataset.mitchMediaSource === style) return;
    element.dataset.mitchMediaSource = style;
    const replacement = await loadMedia(match[1]);
    if (replacement && element.isConnected && element.dataset.mitchMediaSource === style) {
      element.style.backgroundImage = `url("${replacement}")`;
    }
  }

  function repair(node) {
    if (!(node instanceof Element)) return;
    if (node instanceof HTMLImageElement) repairImage(node);
    if (node.style.backgroundImage) repairBackground(node);
    node.querySelectorAll('img, [style*="background-image"]').forEach(child => {
      if (child instanceof HTMLImageElement) repairImage(child);
      if (child.style.backgroundImage) repairBackground(child);
    });
  }

  // Cinny sets img.src directly to the raw /_matrix/media endpoint, which
  // the browser's native <img> loader has no way to attach an Authorization
  // header to — that request always 401s. Without this, every piece of
  // authenticated media (including every GIF already in a room's timeline)
  // briefly shows broken/failed, and Cinny's own retry-button UI reacts to
  // that failed load, before the MutationObserver-based repair() below
  // swaps in the authenticated blob URL a moment later. This intercepts the
  // assignment before the browser ever issues that doomed request, instead
  // of racing it after the fact.
  const imgSrcDescriptor = Object.getOwnPropertyDescriptor(HTMLImageElement.prototype, 'src');
  if (imgSrcDescriptor && imgSrcDescriptor.set) {
    Object.defineProperty(HTMLImageElement.prototype, 'src', {
      configurable: true,
      enumerable: imgSrcDescriptor.enumerable,
      get: imgSrcDescriptor.get,
      set(value) {
        if (authenticatedMediaUrl(value)) {
          this.dataset.mitchMediaSource = value;
          loadMedia(value).then(replacement => {
            if (replacement && this.isConnected && this.dataset.mitchMediaSource === value) {
              imgSrcDescriptor.set.call(this, replacement);
            }
          });
          return;
        }
        imgSrcDescriptor.set.call(this, value);
      }
    });
  }

  new MutationObserver(mutations => {
    for (const mutation of mutations) {
      if (mutation.type === 'attributes') repair(mutation.target);
      else for (const node of mutation.addedNodes) repair(node);
    }
  }).observe(document.documentElement, {
    childList: true,
    subtree: true,
    attributes: true,
    attributeFilter: ['src', 'style']
  });
  document.addEventListener('DOMContentLoaded', () => repair(document.body), { once: true });
  window.addEventListener('pagehide', event => {
    if (event.persisted) return;
    for (const url of objectUrls) URL.revokeObjectURL(url);
  });
})();
