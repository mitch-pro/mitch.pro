(function (global) {
  function withCsrfHeaders(headers) {
    const h = new Headers(headers || {});
    if (!h.has('X-Mitch-Requested-With')) {
      h.set('X-Mitch-Requested-With', '1');
    }
    return h;
  }

  function apiFetch(url, options) {
    options = options || {};
    const headers = withCsrfHeaders(options.headers);
    if (options.body && typeof options.body === 'string' && !headers.has('Content-Type')) {
      headers.set('Content-Type', 'application/json');
    }
    return fetch(url, Object.assign({}, options, {
      credentials: options.credentials || 'include',
      headers: headers,
    }));
  }

  // Auto-attach CSRF header for same-origin mutating /api/* calls site-wide.
  const nativeFetch = global.fetch.bind(global);
  global.fetch = function (input, init) {
    init = init || {};
    let url = '';
    try {
      url = typeof input === 'string' ? input : (input && input.url) || '';
    } catch (_) {}
    const method = String((init.method || (input && input.method) || 'GET')).toUpperCase();
    const mutating = method !== 'GET' && method !== 'HEAD' && method !== 'OPTIONS';
    let path = url;
    try {
      path = new URL(url, global.location && global.location.origin).pathname;
    } catch (_) {}
    if (mutating && path.startsWith('/api/')) {
      const headers = withCsrfHeaders(init.headers || (input && input.headers));
      init = Object.assign({}, init, { headers: headers, credentials: init.credentials || 'include' });
    }
    return nativeFetch(input, init);
  };

  global.apiFetch = apiFetch;

  // Retries a request only on conditions that are actually transient
  // (network drop, 5xx) — never on a normal 4xx response, since that's a
  // real answer (wrong password, already claimed, out of stock, etc.),
  // not a fault. `onRetry(attemptNumber)` is called before each wait so
  // callers can update their status message.
  async function fetchWithRetry(url, options, onRetry) {
    var maxAttempts = 3, baseDelay = 500;
    for (var attempt = 1; attempt <= maxAttempts; attempt++) {
      try {
        var res = await fetch(url, options);
        if (res.status >= 500 && res.status <= 599 && attempt < maxAttempts) {
          if (onRetry) onRetry(attempt);
          await new Promise(function (resolve) {
            setTimeout(resolve, baseDelay * Math.pow(2, attempt - 1) + Math.random() * baseDelay * 0.5);
          });
          continue;
        }
        return res;
      } catch (err) {
        if (attempt >= maxAttempts) throw err;
        if (onRetry) onRetry(attempt);
        await new Promise(function (resolve) {
          setTimeout(resolve, baseDelay * Math.pow(2, attempt - 1) + Math.random() * baseDelay * 0.5);
        });
      }
    }
  }

  global.fetchWithRetry = fetchWithRetry;
})(typeof window !== 'undefined' ? window : globalThis);
