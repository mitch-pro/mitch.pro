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
  }

  function applyAccent(hex) {
    var root = document.documentElement;
    // --mh-* is the homepage's own token namespace (home.css) — it forces
    // its own --mh-accent into the shared --t-ac with !important, so a
    // custom accent has to win there too, not just on --t-ac.
    var props = ['--t-ac', '--t-ac2', '--t-ac3', '--t-gr', '--t-gls', '--mh-accent', '--mh-accent-hover', '--mh-accent-soft'];
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
    root.style.setProperty('--mh-accent', hex);
    root.style.setProperty('--mh-accent-hover', hover);
    root.style.setProperty('--mh-accent-soft', shade(hex, dark ? -0.82 : 0.9));
  }

  function apply() {
    applyMode(readCookie('theme'));
    applyAccent(readCookie('accent'));
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
    }
  };
})();
