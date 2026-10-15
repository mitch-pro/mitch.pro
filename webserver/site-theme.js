// Sitewide theme override: manual light/dark mode + a custom accent color,
// stored in a cookie so the server-rendered page can be colored correctly
// before first paint (no flash). Preferences reads/writes the same cookie
// through the small API below.
(function () {
  function readCookie(name) {
    var m = document.cookie.match(new RegExp('(?:^|; )' + name + '=([^;]*)'));
    return m ? decodeURIComponent(m[1]) : '';
  }
  function writeCookie(name, value) {
    if (value) {
      document.cookie = name + '=' + encodeURIComponent(value) + '; path=/; max-age=31536000; SameSite=Lax';
    } else {
      document.cookie = name + '=; path=/; max-age=0; SameSite=Lax';
    }
  }

  // Mixes a hex color toward black/white in plain RGB space — good enough
  // for a hover/soft shade without pulling in a color library.
  function shade(hex, amount) {
    var m = /^#?([0-9a-f]{6})$/i.exec(hex);
    if (!m) return hex;
    var n = parseInt(m[1], 16);
    var r = (n >> 16) & 255, g = (n >> 8) & 255, b = n & 255;
    var mix = amount < 0 ? 0 : 255;
    var t = Math.abs(amount);
    r = Math.round(r + (mix - r) * t);
    g = Math.round(g + (mix - g) * t);
    b = Math.round(b + (mix - b) * t);
    return '#' + [r, g, b].map(function (v) { return v.toString(16).padStart(2, '0'); }).join('');
  }

  function isDarkMode(mode) {
    if (mode === 'dark') return true;
    if (mode === 'light') return false;
    return window.matchMedia && window.matchMedia('(prefers-color-scheme: dark)').matches;
  }

  function applyMode(mode) {
    var root = document.documentElement;
    if (mode === 'light' || mode === 'dark') {
      root.setAttribute('data-theme', mode);
    } else {
      root.removeAttribute('data-theme');
    }
    root.style.colorScheme = isDarkMode(mode) ? 'dark' : 'light';
    var themeColor = document.querySelector('meta[name="theme-color"]');
    if (themeColor) {
      var background = getComputedStyle(root).getPropertyValue('--t-bg').trim();
      if (background) themeColor.setAttribute('content', background);
    }
    // Legacy event name a couple of pages (Matrix) still listen for to
    // react live instead of only reading the mode on load.
    window.dispatchEvent(new CustomEvent('themechange', { detail: isDarkMode(mode) ? 'dark' : 'light' }));
  }

  function applyAccent(hex) {
    var root = document.documentElement;
    var props = ['--t-ac', '--t-ac2', '--t-ac3', '--t-gr', '--t-gls', '--t-ac-ink'];
    if (!hex || !/^#[0-9a-f]{6}$/i.test(hex)) {
      props.forEach(function (p) { root.style.removeProperty(p); });
      return;
    }
    var dark = isDarkMode(readCookie('theme') || null);
    var hover = shade(hex, dark ? 0.18 : -0.18);
    root.style.setProperty('--t-ac', hex);
    root.style.setProperty('--t-ac2', hover);
    root.style.setProperty('--t-ac3', hover);
    root.style.setProperty('--t-gr', hex);
    root.style.setProperty('--t-gls', shade(hex, dark ? -0.82 : 0.9));
    var rgb = hex.slice(1).match(/.{2}/g).map(function (channel) {
      var value = parseInt(channel, 16) / 255;
      return value <= 0.04045 ? value / 12.92 : Math.pow((value + 0.055) / 1.055, 2.4);
    });
    var luminance = rgb[0] * 0.2126 + rgb[1] * 0.7152 + rgb[2] * 0.0722;
    root.style.setProperty('--t-ac-ink', luminance > 0.179 ? '#000000' : '#ffffff');
  }

  function applyContrast(level) {
    var root = document.documentElement;
    if (level === 'high') {
      root.setAttribute('data-contrast', 'high');
    } else {
      root.removeAttribute('data-contrast');
    }
  }

  function applyLayout() {
    function preference(name, fallback) {
      try { return localStorage.getItem('theme_' + name) || fallback; }
      catch (_) { return fallback; }
    }
    var root = document.documentElement;
    var sizes = { compact: 0.9, normal: 1, comfy: 1.05, huge: 1.1 };
    var corners = { sharp: '0px', soft: '10px', round: '14px', bubble: '22px' };
    var fonts = {
      system: "'DM Sans', system-ui, sans-serif",
      mono: 'ui-monospace, monospace',
      rounded: "'Arial Rounded MT Bold', 'Trebuchet MS', sans-serif",
      serif: 'Georgia, serif',
      futuristic: "'Trebuchet MS', system-ui, sans-serif"
    };
    var size = sizes[preference('density', 'normal')] || 1;
    var radius = preference('radius', 'soft');
    var font = fonts[preference('font', 'system')] || fonts.system;
    var reducedMotion = preference('motion', 'on') === 'off';
    root.style.fontSize = (16 * size) + 'px';
    root.style.setProperty('--t-ui-scale', String(size));
    root.style.setProperty('--t-radius', corners[radius] || corners.soft);
    root.style.setProperty('--t-radius-sm', radius === 'sharp' ? '0px' : '7px');
    root.style.setProperty('--t-font', font);
    root.style.setProperty('--t-display', font);
    root.style.setProperty('--t-motion', reducedMotion ? '0s' : '.15s');
    root.dataset.radius = Object.hasOwn(corners, radius) ? radius : 'soft';
    root.dataset.material = preference('material', 'auto') === 'glass' ? 'glass' : 'solid';
    root.classList.toggle('theme-no-motion', reducedMotion);
  }

  function apply() {
    applyMode(readCookie('theme'));
    applyAccent(readCookie('accent'));
    applyContrast(readCookie('contrast'));
    applyLayout();
  }

  apply();

  window.__siteTheme = {
    getMode: function () { return readCookie('theme'); },
    setMode: function (mode) {
      writeCookie('theme', mode === 'light' || mode === 'dark' ? mode : '');
      apply();
    },
    getAccent: function () { return readCookie('accent'); },
    setAccent: function (hex) {
      writeCookie('accent', hex && /^#[0-9a-f]{6}$/i.test(hex) ? hex.toLowerCase() : '');
      apply();
    },
    getContrast: function () { return readCookie('contrast'); },
    setContrast: function (level) {
      writeCookie('contrast', level === 'high' ? 'high' : '');
      apply();
    }
  };
})();
