(function () {
  /* ── Two-mode theme engine ────────────────────────────────────────────────
     Dark (default) and light. Everything on the site rides the --t-* tokens
     set here, so switching modes re-colors every page. A sun/moon toggle
     button (#theme-btn) flips the mode and remembers it in the `theme`
     cookie. Legacy multi-theme cookie values (void, daylight, github, …)
     normalize to dark or light. */

  var DARK = {
    name: 'Dark',
    bg: '#070510', bg2: 'rgba(22,13,44,0.7)', bg3: 'rgba(42,23,76,0.74)',
    fg: '#faf7ff', fg2: '#b4a9c9',
    ac: '#b86cff', ac2: '#e47cff', ac3: '#8257ff',
    bd: 'rgba(224,198,255,0.16)', bda: 'rgba(194,126,255,0.48)',
    gl: 'rgba(180,94,255,0.35)', gls: 'rgba(180,94,255,0.14)',
    gr: 'linear-gradient(135deg,#7957f1,#ca57f5 56%,#ff5fa7)',
    bgr: 'linear-gradient(180deg,rgba(5,4,17,0.85),rgba(5,4,17,0.95))',
    bgImg: '',
    sw: '#b86cff',
  };
  var LIGHT = {
    name: 'Light',
    light: true,
    bg: '#eef1f9', bg2: '#ffffff', bg3: '#e7ebf7',
    fg: '#101426', fg2: '#3f4560',
    ac: '#4f46e5', ac2: '#0891b2', ac3: '#db2777',
    bd: 'rgba(16,20,42,0.16)', bda: 'rgba(79,70,229,0.5)',
    gl: 'rgba(79,70,229,0.34)', gls: 'rgba(79,70,229,0.16)',
    // Light mode keeps gradients in one indigo family — the old indigo→cyan→pink
    // sweep read as confetti. Page backdrop is a plain neutral wash.
    gr: 'linear-gradient(135deg,#4f46e5,#7c3aed)',
    bgr: 'linear-gradient(160deg,#f3f5fa,#e9edf6)',
    bgImg: '',
    sw: '#4f46e5',
  };
  var T = { dark: DARK, light: LIGHT };
  var LEGACY_LIGHT = { daylight: 1, paper: 1, arctic: 1, blossom: 1 };
  var BACKGROUND_DEFAULTS_VERSION = 'mountain-2026-09-06';
  var VFX_DEFAULTS_VERSION = 'all-on-2026-09-06';
  var VFX_DEFAULTS = { snow: true, stars: true, rain: true, particles: true };
  function applyVFXDefaults() {
    if (usesSchoolDefaults() || getPref('vfxDefaults', '') === VFX_DEFAULTS_VERSION) return;
    try {
      localStorage.setItem('_prefVFX', JSON.stringify(VFX_DEFAULTS));
      setPref('vfxDefaults', VFX_DEFAULTS_VERSION);
    } catch (_) {}
  }
  var BACKGROUND_DEFAULTS = {
    bgimg: '/backgrounds/wallhaven-black-mountain.webp',
    bgblur: '8', accent: '', adapt: 'on', dim: '0.50', bgmode: 'cover', bgpos: 'center'
  };
  var SCHOOL_BACKGROUND_DEFAULT = '/backgrounds/wallhaven-black-mountain.webp';

  function usesSchoolDefaults() {
    return /(^|\.)rjuhsd\.school$/.test(location.hostname) || /^\/rjuhsd(?:\/|$)/.test(location.pathname);
  }
  function applyBackgroundDefaults() {
    if (usesSchoolDefaults()) {
      if (!getPref('bgimg', '')) setBgImgCookie(SCHOOL_BACKGROUND_DEFAULT);
      if (!getPref('bgblur', '')) setPref('bgblur', '8');
      return;
    }
    if (getPref('backgroundDefaults', '') === BACKGROUND_DEFAULTS_VERSION) return;
    Object.keys(BACKGROUND_DEFAULTS).forEach(function (key) { setPref(key, BACKGROUND_DEFAULTS[key]); });
    setBgImgCookie(BACKGROUND_DEFAULTS.bgimg);
    setCookie('dark');
    setPref('backgroundDefaults', BACKGROUND_DEFAULTS_VERSION);
  }
  function preparePreferenceSnapshot(snapshot) {
    if (usesSchoolDefaults()) {
      if (snapshot.theme_bgimg) return snapshot;
      return Object.assign({}, snapshot, { theme_bgimg: SCHOOL_BACKGROUND_DEFAULT });
    }
    var result = snapshot;
    if (snapshot.theme_vfxDefaults !== VFX_DEFAULTS_VERSION) {
      result = Object.assign({}, result, { _prefVFX: Object.assign({}, VFX_DEFAULTS), theme_vfxDefaults: VFX_DEFAULTS_VERSION });
    }
    if (snapshot.theme_backgroundDefaults === BACKGROUND_DEFAULTS_VERSION) return result;
    // Old account backups must not undo the site-wide default rollout.
    result = Object.assign({}, result);
    Object.keys(BACKGROUND_DEFAULTS).forEach(function (key) { result['theme_' + key] = BACKGROUND_DEFAULTS[key]; });
    result.theme_backgroundDefaults = BACKGROUND_DEFAULTS_VERSION;
    setCookie('dark');
    return result;
  }

  function normalize(name) {
    if (name === 'light' || LEGACY_LIGHT[name]) return 'light';
    return 'dark';
  }

  function getCookie() {
    var m = document.cookie.match(/(?:^|; )theme=([^;]+)/);
    return m ? normalize(decodeURIComponent(m[1])) : 'dark';
  }
  function setCookie(mode) {
    document.cookie = 'theme=' + encodeURIComponent(mode) + ';path=/;max-age=31536000';
  }

  function getBgImgCookie() {
    var m = document.cookie.match(/(?:^|; )bgimg=([^;]*)/);
    var v = m ? decodeURIComponent(m[1]) : '';
    var saved = getPref('bgimg', null);
    if (saved !== null) v = saved;
    setBgImgCookie(v);
    // The official wallpapers were SVGs once; anyone still pointing at one gets
    // silently moved to its .webp replacement (same art, rasterized).
    var migrated = v.replace(/^\/backgrounds\/(bg-[a-z0-9-]+)\.svg$/, '/backgrounds/$1.webp');
    if (migrated !== v) { setBgImgCookie(migrated); v = migrated; }
    return v;
  }
  function setBgImgCookie(url) {
    setPref('bgimg', url || '');
    document.cookie = 'bgimg=' + encodeURIComponent(url || '') + ';path=/;max-age=31536000';
  }

  function getPref(key, fallback) {
    try {
      var v = localStorage.getItem('theme_' + key);
      return v === null ? fallback : v;
    } catch (_) { return fallback; }
  }
  function setPref(key, value) {
    try { localStorage.setItem('theme_' + key, String(value)); } catch (_) {}
  }
  function clamp(n, min, max) {
    n = Number(n);
    return Number.isFinite(n) ? Math.max(min, Math.min(max, n)) : min;
  }
  function hexToRgba(hex, alpha) {
    var m = /^#?([0-9a-f]{6})$/i.exec(String(hex || ''));
    if (!m) return '';
    var n = parseInt(m[1], 16);
    return 'rgba(' + ((n >> 16) & 255) + ',' + ((n >> 8) & 255) + ',' + (n & 255) + ',' + alpha + ')';
  }
  function rgbToHsl(r, g, b) {
    r /= 255; g /= 255; b /= 255;
    var max = Math.max(r, g, b), min = Math.min(r, g, b), h = 0, s = 0;
    var l = (max + min) / 2;
    if (max !== min) {
      var d = max - min;
      s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
      if (max === r) h = ((g - b) / d + (g < b ? 6 : 0));
      else if (max === g) h = (b - r) / d + 2;
      else h = (r - g) / d + 4;
      h *= 60;
    }
    return [h, s, l];
  }
  function hslToHex(h, s, l) {
    h = ((h % 360) + 360) % 360;
    s = clamp(s, 0, 1); l = clamp(l, 0, 1);
    var c = (1 - Math.abs(2 * l - 1)) * s;
    var x = c * (1 - Math.abs(((h / 60) % 2) - 1));
    var m = l - c / 2;
    var rgb;
    if (h < 60) rgb = [c, x, 0];
    else if (h < 120) rgb = [x, c, 0];
    else if (h < 180) rgb = [0, c, x];
    else if (h < 240) rgb = [0, x, c];
    else if (h < 300) rgb = [x, 0, c];
    else rgb = [c, 0, x];
    var to = function (v) {
      var n = Math.round((v + m) * 255);
      return ('0' + clamp(n, 0, 255).toString(16)).slice(-2);
    };
    return '#' + to(rgb[0]) + to(rgb[1]) + to(rgb[2]);
  }
  function applyCustomizationPrefs() {
    var r = document.documentElement.style;
    var dim = clamp(getPref('dim', '0.50'), 0, 0.85);
    var bgMode = getPref('bgmode', 'cover');
    var bgSize = bgMode === 'contain' ? 'contain' : bgMode === 'tile' ? 'auto' : bgMode === 'stretch' ? '100% 100%' : 'cover';
    var bgRepeat = bgMode === 'tile' ? 'repeat' : 'no-repeat';
    var bgPos = getPref('bgpos', 'center');
    var defaultDensity = navigator.userAgent.includes('CrOS') ? 'compact' : 'normal';
    var density = getPref('density', defaultDensity);
    var radius = getPref('radius', 'soft');
    var font = getPref('font', 'system');
    var motion = getPref('motion', 'on');
    var fontMap = {
      system: '"Segoe UI",system-ui,-apple-system,sans-serif',
      mono: '"DM Mono","SFMono-Regular",Consolas,monospace',
      rounded: 'ui-rounded,"Nunito","Segoe UI",system-ui,sans-serif',
      serif: 'Georgia,"Times New Roman",serif',
      futuristic: '"Trebuchet MS","Segoe UI",system-ui,sans-serif'
    };
    var radiusMap = { sharp: '3px', soft: '8px', round: '14px', bubble: '22px' };
    var densityMap = { compact: '.9', normal: '1', comfy: '1.05', huge: '1.10' };
    r.setProperty('--t-bg-dim', dim.toFixed(2));
    r.setProperty('--t-bg-size', bgSize);
    r.setProperty('--t-bg-repeat', bgRepeat);
    r.setProperty('--t-bg-pos', bgPos);
    r.setProperty('--t-bg-blur', clamp(getPref('bgblur', '8'), 0, 40).toFixed(0) + 'px');
    r.setProperty('--t-font', fontMap[font] || fontMap.system);
    r.setProperty('--t-radius', radiusMap[radius] || radiusMap.soft);
    r.setProperty('--t-ui-scale', densityMap[density] || '1');
    r.setProperty('--t-motion', motion === 'off' ? '0s' : '.15s');
    var accent = getPref('accent', '');
    if (/^#[0-9a-f]{6}$/i.test(accent)) {
      r.setProperty('--t-ac', accent);
      r.setProperty('--t-ac2', accent);
      r.setProperty('--t-bda', hexToRgba(accent, 0.55));
      r.setProperty('--t-gl', hexToRgba(accent, 0.48));
      r.setProperty('--t-gls', hexToRgba(accent, 0.15));
      r.setProperty('--t-gr', 'linear-gradient(135deg,' + accent + ',var(--t-ac3,#60a5fa))');
    }
    document.documentElement.classList.toggle('theme-no-motion', motion === 'off');
    applyMaterialMode();
  }

  function canUseGlass() {
    try {
      return !!(window.CSS && CSS.supports && (
        CSS.supports('backdrop-filter', 'blur(8px)') ||
        CSS.supports('-webkit-backdrop-filter', 'blur(8px)')
      ));
    } catch (_) {
      return false;
    }
  }

  function applyMaterialMode() {
    var pref = getPref('material', 'auto');
    if (pref !== 'glass' && pref !== 'solid' && pref !== 'auto') pref = 'auto';
    // Auto always solid. Explicit glass works wherever backdrop-filter is supported (incl. Firefox).
    var useGlass = pref === 'glass' ? canUseGlass() : false;
    document.documentElement.classList.toggle('theme-glass', useGlass);
    document.documentElement.classList.toggle('theme-solid', !useGlass);
    document.documentElement.setAttribute('data-material', useGlass ? 'glass' : 'solid');
    document.documentElement.setAttribute('data-material-pref', pref);
  }

  // The wallpaper picker UI is gone (preferences no longer offers a
  // library), so this is just the one background rjuhsd.school/the old
  // mitch.pro default still actually use — every other entry used to
  // point at files that were deleted along with the picker.
  var THEME_BGS = [
    { id: 'wallhaven-black-mountain', name: 'Wallhaven Black Mountain', url: '/backgrounds/wallhaven-black-mountain.webp', thumbUrl: '/backgrounds/thumbs/wallhaven-black-mountain.webp' }
  ];

  function isHomePage() {
    var p = location.pathname || '/';
    return p === '/' || p === '/index.html';
  }

  // School hubs keep their own identity and use the school wallpaper when a
  // visitor has not selected another background.
  function isSchoolHub() {
    return !!(document.body && document.body.classList.contains('school-hub'));
  }

  function getEffectiveBgImg() {
    // rjuhsd.school only ever ships the one wallpaper now — unconditional,
    // so a stale "bgimg" cookie from before the wallpaper library was
    // trimmed down (pointing at a background that no longer exists) can't
    // leave the page with a broken/missing background.
    if (isSchoolHub()) return SCHOOL_BACKGROUND_DEFAULT;
    var custom = getBgImgCookie();
    if (custom) return custom;
    var isLight = document.documentElement.classList.contains('theme-light');
    if (isLight) return '';
    return '/backgrounds/wallhaven-black-mountain.webp';
  }

  // Body backgrounds are forced transparent (inline author-important beats
  // stylesheet author-important, e.g. site-galaxy's background-color rule) so
  // the fixed html::before wallpaper layer is visible underneath. Idempotent —
  // other code mutates body.style, so re-assert on every applyBgImg call.
  function setBodyTransparent() {
    var b = document.body;
    if (!b) return;
    b.style.setProperty('background-color', 'transparent', 'important');
    b.style.setProperty('background-image', 'none', 'important');
    b.style.setProperty('background-attachment', 'fixed', 'important');
    document.documentElement.style.setProperty('background', 'var(--t-bg)', 'important');
  }
  function clearBodyTransparent() {
    var b = document.body;
    if (!b) return;
    b.style.removeProperty('background-color');
    b.style.removeProperty('background-image');
    b.style.removeProperty('background-attachment');
    document.documentElement.style.removeProperty('background');
  }

  var effectInstance = null, effectKey = '', effectGeneration = 0, effectLoads = {};
  function getBackground(url) {
    return THEME_BGS.find(function (bg) { return bg.url === url; });
  }
  function applyEffect(config) {
    var key = config ? config.effect : '';
    if (key === effectKey) { if (effectInstance) effectInstance.update(); return; }
    effectKey = key;
    var generation = ++effectGeneration;
    if (effectInstance) effectInstance.destroy();
    effectInstance = null;
    if (!config) return;
    if (!effectLoads[key]) effectLoads[key] = new Promise(function (resolve, reject) {
      var script = document.createElement('script');
      script.src = config.script;
      script.onload = function () { script.remove(); resolve(); };
      script.onerror = function () { script.remove(); reject(new Error('Background unavailable')); };
      document.head.appendChild(script);
    });
    effectLoads[key].then(function () {
      if (generation !== effectGeneration) return;
      effectInstance = window.MitchBackgroundEffects[key](document.documentElement);
    }).catch(function () {
      if (generation === effectGeneration) effectKey = '';
      delete effectLoads[key];
    });
  }
  window.addEventListener('storage', function (event) {
    if (event.key === 'theme_bgimg') {
      setBgImgCookie(event.newValue || '');
      applyBgImg(getEffectiveBgImg());
    }
  });
  window.addEventListener('pagehide', function () { applyEffect(null); });
  window.addEventListener('pageshow', function (event) { if (event.persisted) applyBgImg(getEffectiveBgImg()); });

  function applyBgImg(url) {
    var h = document.documentElement, r = h.style;
    var isLight = h.classList.contains('theme-light');
    var videoEl = document.getElementById('mitch-bg-video');
    var config = getBackground(url);
    var effect = config && config.effect ? config : null;
    if (effect) h.setAttribute('data-bg-effect', effect.effect); else h.removeAttribute('data-bg-effect');
    applyEffect(effect);
    var isVideo = url && (/\.(webm|mp4)($|\?)/i.test(url) || /^data:video\/(webm|mp4)/i.test(url));

    if (isVideo) {
      if (!videoEl) {
        videoEl = document.createElement('video');
        videoEl.id = 'mitch-bg-video';
        videoEl.setAttribute('autoplay', '');
        videoEl.setAttribute('loop', '');
        videoEl.setAttribute('muted', '');
        videoEl.setAttribute('playsinline', '');
        videoEl.style.cssText = 'position:fixed;top:0;left:0;width:100vw;height:100vh;object-fit:cover;z-index:-9999;pointer-events:none;transition:opacity 0.5s ease;';
        (document.body || document.documentElement).appendChild(videoEl);
      }
      if (videoEl.getAttribute('src') !== url) {
        videoEl.src = url;
      }
      var isPrefs = (location.pathname || '').indexOf('/preferences') === 0;
      if (isPrefs) {
        videoEl.pause();
        try { videoEl.currentTime = 0.1; } catch (_) {}
      } else {
        videoEl.play().catch(function(){});
      }
      videoEl.style.display = 'block';
      videoEl.style.opacity = isLight ? '0.35' : '0.65';
      r.setProperty('--t-bg-img-layer', 'none');
    } else {
      if (videoEl) {
        videoEl.pause();
        videoEl.removeAttribute('src');
        videoEl.load();
        videoEl.remove();
      }
      if (effect) {
        r.setProperty('--t-bg-img-layer', 'none');
      } else if (url) {
        // User wallpaper: paint dim gradient + image, honoring the pos/size
        // preferences. Painted via the background shorthand (see baseStyle),
        // so position/size ride inside the value.
        var dimGradient = isLight
          ? 'linear-gradient(rgba(240,243,250,var(--t-bg-dim-light,0.85)),rgba(240,243,250,var(--t-bg-dim-light,0.85)))'
          : 'linear-gradient(rgba(0,0,0,var(--t-bg-dim,0.5)),rgba(0,0,0,var(--t-bg-dim,0.5)))';
        r.setProperty('--t-bg-img-layer',
          dimGradient + ',url(' + JSON.stringify(url) + ') var(--t-bg-pos,center) / var(--t-bg-size,cover) var(--t-bg-repeat,no-repeat)');
      } else {
        // Page-owned --t-bgr (stylesheets) wins over the theme-owned default.
        r.setProperty('--t-bg-img-layer', 'var(--t-bgr, var(--t-bgr-theme, none))');
      }
    }
    var resolved = '';
    try { resolved = getComputedStyle(h).getPropertyValue('--t-bg-img-layer'); } catch (_) {}
    // Pages declare --t-bgr on <body>, and custom properties never inherit
    // upward — so if the html-level var chain resolved to nothing, lift the
    // body's computed value inline onto <html> as a literal layer value.
    if (!url && !/url\(/.test(resolved) && document.body) {
      try {
        var fromBody = getComputedStyle(document.body).getPropertyValue('--t-bgr');
        if (/url\(/.test(fromBody)) { r.setProperty('--t-bg-img-layer', fromBody); resolved = fromBody; }
        // Pages also declare --t-bg-layer-opacity on <body> next to --t-bgr —
        // same inheritance problem (the html::before layer can't see it).
        // Lift it too when the page set a non-default value.
        var bodyOp = getComputedStyle(document.body).getPropertyValue('--t-bg-layer-opacity').trim();
        if (bodyOp && bodyOp !== '1' && !h.style.getPropertyValue('--t-bg-layer-opacity')) {
          r.setProperty('--t-bg-layer-opacity', bodyOp);
        }
      } catch (_) {}
    }
    var active = !!effect || isVideo || /url\(/.test(resolved);
    h.toggleAttribute('data-bglayer', active);
    if (active) setBodyTransparent(); else clearBodyTransparent();
    scheduleAdaptive();
  }

  /* ── Background-adaptive accent (dark mode) ────────────────────────────────
     Samples the active wallpaper (the url() inside --t-bg-img-layer / --t-bgr)
     on a tiny canvas, finds its dominant saturated hue, and tints the accent
     tokens to match — so the UI picks up the background's color. Skipped in
     light mode, when the user set a manual accent, or when theme_adapt=off. */

  var ADAPT_CACHE = {};
  var adaptTimer = null;
  // Tokens this feature owns — applyTheme only resets --t-*, so clearAdaptive
  // must remove ALL of them (including the bg2/bg3 nudges and --ui-* writes)
  // or they'd survive a dark → light → dark round trip.
  var ADAPT_TOKENS = ['--t-ac', '--t-ac2', '--t-ac3', '--t-bda', '--t-gl', '--t-gls', '--t-gr', '--t-bg2', '--t-bg3', '--ui-blue', '--ui-blue-2'];

  function scheduleAdaptive() {
    clearTimeout(adaptTimer);
    adaptTimer = setTimeout(applyAdaptiveTheme, 250);
  }

  function clearAdaptive() {
    var r = document.documentElement.style;
    for (var i = 0; i < ADAPT_TOKENS.length; i++) r.removeProperty(ADAPT_TOKENS[i]);
    // applyTheme set these inline just before we cleared them, and the :root
    // blocks in portal-redesign.css / site-galaxy.css are dark-only fallbacks —
    // without this restore every bail-out (light mode, manual accent, adapt
    // off, sampler failure) would land on dark purple tokens over the light
    // palette. Restore the active theme's values, then let a manual accent sit
    // back on top.
    var t = T[document.documentElement.classList.contains('theme-light') ? 'light' : 'dark'];
    if (t) {
      r.setProperty('--t-bg2', t.bg2);
      r.setProperty('--t-bg3', t.bg3);
      r.setProperty('--t-ac',  t.ac);
      r.setProperty('--t-ac2', t.ac2);
      r.setProperty('--t-ac3', t.ac3);
      r.setProperty('--t-bda', t.bda);
      r.setProperty('--t-gl',  t.gl);
      r.setProperty('--t-gls', t.gls);
      r.setProperty('--t-gr',  t.gr);
    }
    applyCustomizationPrefs();
    document.documentElement.removeAttribute('data-adapt');
  }

  function adaptiveImageUrl() {
    try {
      var style = getComputedStyle(document.documentElement);
      var cands = [style.getPropertyValue('--t-bg-img-layer'), style.getPropertyValue('--t-bgr')];
      for (var i = 0; i < cands.length; i++) {
        var m = /url\((['"]?)([^'")]+)\1\)/.exec(cands[i] || '');
        if (m && m[2]) return m[2].trim();
      }
    } catch (_) {}
    return '';
  }

  function applyAdaptiveTheme(force) {
    var root = document.documentElement;
    if (root.hasAttribute('data-bg-effect')) { clearAdaptive(); return; }
    if (getPref('adapt', 'on') === 'off' && !force) { clearAdaptive(); return; }
    if (root.classList.contains('theme-light')) { clearAdaptive(); return; }
    if (isSchoolHub()) { clearAdaptive(); return; }
    // A manual accent is the user's explicit choice — never override it.
    if (/^#[0-9a-f]{6}$/i.test(getPref('accent', ''))) { clearAdaptive(); return; }

    var url = adaptiveImageUrl();
    if (!url) { clearAdaptive(); return; }
    var mode = root.classList.contains('theme-light') ? 'light' : 'dark';
    var cacheKey = mode + '|' + url;

    var cached = ADAPT_CACHE[cacheKey];
    if (!cached) {
      try {
        var stored = sessionStorage.getItem('mitch_adapt_' + cacheKey);
        if (stored) {
          cached = JSON.parse(stored);
          if (cached && cached.tokens) ADAPT_CACHE[cacheKey] = cached;
        }
      } catch (_) {}
    }
    if (cached) {
      if (cached.failed) { clearAdaptive(); return; }
      writeAdaptiveTokens(cached.tokens);
      return;
    }

    // Cross-origin wallpapers taint the canvas — pre-reject instead of trying.
    var abs = null;
    try { abs = new URL(url, location.href); } catch (_) {}
    if (!abs || abs.origin !== location.origin) {
      ADAPT_CACHE[cacheKey] = { failed: true };
      clearAdaptive();
      return;
    }

    var img = new Image();
    img.crossOrigin = 'anonymous';
    img.onload = function () {
      try {
        var tokens = sampleImagePalette(img);
        ADAPT_CACHE[cacheKey] = { tokens: tokens };
        try { sessionStorage.setItem('mitch_adapt_' + cacheKey, JSON.stringify({ tokens: tokens })); } catch (_) {}
        writeAdaptiveTokens(tokens);
      } catch (_) {
        ADAPT_CACHE[cacheKey] = { failed: true };
        clearAdaptive();
      }
    };
    img.onerror = function () {
      ADAPT_CACHE[cacheKey] = { failed: true };
      clearAdaptive();
    };
    img.src = abs.href;
  }

  function sampleImagePalette(img) {
    var SIZE = 48;
    var canvas = document.createElement('canvas');
    canvas.width = SIZE; canvas.height = SIZE;
    var ctx = canvas.getContext('2d', { willReadFrequently: true });
    if (!ctx) throw Error('no 2d context');
    ctx.drawImage(img, 0, 0, SIZE, SIZE);
    var data = ctx.getImageData(0, 0, SIZE, SIZE).data;

    // Hue bins weighted toward saturated mid-luminance pixels — ignores the
    // dark overlay gradient that sits on top of the wallpaper. Weight also
    // falls off toward the edges so the cover/center crop the user actually
    // sees dominates.
    var BINS = 36, BIN = 360 / BINS;
    var bins = [];
    for (var i = 0; i < BINS; i++) bins.push({ w: 0, sinSum: 0, cosSum: 0, chroma: 0, lSum: 0 });
    for (var p = 0; p < data.length; p += 4) {
      var px = (p / 4) % SIZE;
      var py = Math.floor(p / 4 / SIZE);
      var dx = (px + 0.5) / SIZE - 0.5;
      var dy = (py + 0.5) / SIZE - 0.5;
      var centerW = 1 - 0.35 * Math.min(1, Math.sqrt(dx * dx + dy * dy) * 2);
      var hsl = rgbToHsl(data[p], data[p + 1], data[p + 2]);
      var h = hsl[0], s = hsl[1], l = hsl[2];
      if (s < 0.14 || l < 0.05 || l > 0.95) continue;
      var w = s * (1 - Math.abs(l - 0.42) * 1.6) * centerW;
      if (w <= 0) continue;
      var rad = h * Math.PI / 180;
      var bin = bins[Math.min(BINS - 1, Math.floor(h / BIN))];
      bin.w += w;
      bin.sinSum += Math.sin(rad) * w;
      bin.cosSum += Math.cos(rad) * w;
      bin.chroma += s * w;
      bin.lSum += l * w;
    }

    var best = null, bestIdx = -1;
    for (var b = 0; b < BINS; b++) {
      if (bins[b].w <= 0) continue;
      if (!best || bins[b].w > best.w) { best = bins[b]; bestIdx = b; }
    }
    if (!best) throw Error('no usable color');
    // Merge the winning bin with its neighbours when they carry comparable
    // weight — gradients smear a hue across adjacent bins and the circular
    // mean alone still flickers between them.
    var agg = { w: 0, sinSum: 0, cosSum: 0, chroma: 0, lSum: 0 };
    for (var n = -1; n <= 1; n++) {
      var nb = bins[(bestIdx + n + BINS) % BINS];
      if (n !== 0 && nb.w < best.w * 0.45) continue;
      agg.w += nb.w;
      agg.sinSum += nb.sinSum;
      agg.cosSum += nb.cosSum;
      agg.chroma += nb.chroma;
      agg.lSum += nb.lSum;
    }
    var H = Math.atan2(agg.sinSum, agg.cosSum) * 180 / Math.PI;
    // meanL is weighted over the same accepted pixels (not the whole image —
    // a mostly-dark wallpaper used to drag the accent lightness the wrong way).
    var meanL = agg.lSum / agg.w;
    var S = clamp(agg.chroma / agg.w * 1.05, 0.42, 0.75);
    var Lac = clamp(0.62 - (meanL - 0.34) * 0.22, 0.55, 0.68);

    // Contrast floor against the page base so the accent never turns to mud
    // on a wallpaper whose luminance sits close to --t-bg.
    var bgHex = document.documentElement.style.getPropertyValue('--t-bg') ||
      getComputedStyle(document.documentElement).getPropertyValue('--t-bg') || '';
    if (/^#[0-9a-f]{6}$/i.test(bgHex)) {
      var bgL = (0.2126 * parseInt(bgHex.slice(1, 3), 16) +
        0.7152 * parseInt(bgHex.slice(3, 5), 16) +
        0.0722 * parseInt(bgHex.slice(5, 7), 16)) / 255;
      var accHex = hslToHex(H, S, Lac);
      var acLum = (0.2126 * parseInt(accHex.slice(1, 3), 16) +
        0.7152 * parseInt(accHex.slice(3, 5), 16) +
        0.0722 * parseInt(accHex.slice(5, 7), 16)) / 255;
      if (Math.abs(bgL - acLum) < 0.30) {
        var mid = bgL > 0.5 ? bgL - 0.30 : bgL + 0.30;
        Lac = clamp(mid, 0.52, 0.70);
      }
    }

    var ac = hslToHex(H, S, Lac);
    var ac2 = hslToHex(H + 18, S * 0.9, Math.min(Lac + 0.10, 0.8));
    var ac3 = hslToHex(H - 30, S * 0.85, Math.max(Lac - 0.08, 0.3));
    return { ac: ac, ac2: ac2, ac3: ac3 };
  }

  function writeAdaptiveTokens(tokens) {
    var r = document.documentElement.style;
    // Same alphas as the manual-accent block in applyCustomizationPrefs(), so
    // adaptive and hand-picked accents are visually interchangeable.
    r.setProperty('--t-ac', tokens.ac);
    r.setProperty('--t-ac2', tokens.ac2);
    r.setProperty('--t-ac3', tokens.ac3);
    r.setProperty('--t-bda', hexToRgba(tokens.ac, 0.55));
    r.setProperty('--t-gl', hexToRgba(tokens.ac, 0.48));
    r.setProperty('--t-gls', hexToRgba(tokens.ac, 0.15));
    r.setProperty('--t-gr', 'linear-gradient(135deg,' + tokens.ac + ',' + tokens.ac3 + ')');

    // Nudge the translucent surfaces 12% toward the accent (never the opaque
    // page base or the wallpaper itself).
    blendSurface('--t-bg2', tokens.ac);
    blendSurface('--t-bg3', tokens.ac);

    // portal-redesign.css keeps its own --ui-* palette that never reads --t-*;
    // an inline style on <html> beats both its :root and .theme-light blocks.
    r.setProperty('--ui-blue', tokens.ac);
    r.setProperty('--ui-blue-2', tokens.ac2);
    document.documentElement.setAttribute('data-adapt', '1');
  }

  function blendSurface(prop, accentHex) {
    var cur = document.documentElement.style.getPropertyValue(prop) ||
      getComputedStyle(document.documentElement).getPropertyValue(prop) || '';
    var m = /rgba?\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*(?:,\s*([\d.]+)\s*)?\)/.exec(cur);
    if (!m) return;
    var a = parseInt(accentHex.slice(1, 3), 16);
    var g = parseInt(accentHex.slice(3, 5), 16);
    var b = parseInt(accentHex.slice(5, 7), 16);
    var mix = function (cStr, ac) { return Math.round(Number(cStr) * 0.88 + ac * 0.12); };
    var out = 'rgba(' + mix(m[1], a) + ',' + mix(m[2], g) + ',' + mix(m[3], b) +
      (m[4] !== undefined ? ',' + m[4] : '') + ')';
    document.documentElement.style.setProperty(prop, out);
  }

  var SUN_SVG =
    '<svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" ' +
    'stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">' +
    '<circle cx="12" cy="12" r="4.1"/>' +
    '<path d="M12 2.6v2.3M12 19.1v2.3M2.6 12h2.3M19.1 12h2.3M5.2 5.2l1.7 1.7M17.1 17.1l1.7 1.7M18.8 5.2l-1.7 1.7M6.9 17.1l-1.7 1.7"/>' +
    '</svg>';
  var MOON_SVG =
    '<svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" ' +
    'stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">' +
    '<path d="M20.6 14.6A8.6 8.6 0 0 1 9.4 3.4a8.6 8.6 0 1 0 11.2 11.2Z"/>' +
    '</svg>';

  function syncToggleBtn() {
    var btn = document.getElementById('theme-btn');
    if (!btn) return;
    var light = getCookie() === 'light';
    btn.innerHTML = light ? MOON_SVG : SUN_SVG;
    btn.title = light ? 'Switch to dark theme' : 'Switch to light theme';
    btn.setAttribute('aria-label', btn.title);
    if (light) {
      btn.style.background = 'rgba(15,17,35,0.06)';
      btn.style.borderColor = 'rgba(15,17,35,0.18)';
      btn.style.color = '#1e1b4b';
    } else {
      btn.style.background = 'rgba(255,255,255,0.06)';
      btn.style.borderColor = 'rgba(255,255,255,0.16)';
      btn.style.color = 'rgba(255,255,255,0.85)';
    }
  }

  function applyTheme(name) {
    var t = T[normalize(name)];
    var r = document.documentElement.style;
    document.documentElement.classList.toggle('theme-light', !!(t.light));
    document.documentElement.style.colorScheme = t.light ? 'light' : 'dark';
    // Announce every application — not just toggles from the button this
    // script builds — so pages that wire their own #theme-btn (rjuhsd hub)
    // can re-sync their body class and icon after __theme.apply().
    try { window.dispatchEvent(new CustomEvent('themechange', { detail: t.name })); } catch (_) {}
    applyCustomizationPrefs();
    r.setProperty('--t-bg',  t.bg);
    r.setProperty('--t-bg2', t.bg2);
    r.setProperty('--t-bg3', t.bg3);
    r.setProperty('--t-fg',  t.fg);
    r.setProperty('--t-fg2', t.fg2);
    r.setProperty('--t-ac',  t.ac);
    r.setProperty('--t-ac2', t.ac2);
    r.setProperty('--t-ac3', t.ac3);
    r.setProperty('--t-bd',  t.bd);
    r.setProperty('--t-bda', t.bda);
    r.setProperty('--t-gl',  t.gl);
    r.setProperty('--t-gls', t.gls);
    r.setProperty('--t-gr',  t.gr);
    r.setProperty('--t-bgr-theme', t.bgr);
    r.setProperty('--t-display', "'Figtree', system-ui, sans-serif");
    applyCustomizationPrefs();
    applyBgImg(getEffectiveBgImg());
    applyAdaptiveTheme();
    syncToggleBtn();
  }

  applyBackgroundDefaults();
  applyVFXDefaults();
  applyTheme(getCookie());
  // applyTheme ran while <body> didn't exist yet (script is in <head>) —
  // re-assert the wallpaper layer's body transparency once it does.
  if (document.body) { applyBgImg(getEffectiveBgImg()); }
  else { document.addEventListener('DOMContentLoaded', function () { applyBgImg(getEffectiveBgImg()); }); }

  var baseStyle = document.createElement('style');
  baseStyle.textContent =
    'html { font-size: calc(16px * var(--t-ui-scale, 1)); }' +
    'body{background-color:var(--t-bg);color:var(--t-fg);font-family:var(--t-font,"Segoe UI",system-ui,-apple-system,sans-serif);' +
      'background-image:var(--t-bg-img-layer,none)!important;' +
      'background-size:var(--t-bg-size,cover)!important;background-position:var(--t-bg-pos,center)!important;' +
      'background-repeat:var(--t-bg-repeat,no-repeat)!important;background-attachment:fixed!important;}' +
    // Unified wallpaper layer: a fixed, blurred pseudo-element below all
    // content. Engaged via [data-bglayer] when a url() resolves into
    // --t-bg-img-layer (custom bgimg cookie or a page-declared --t-bgr); body
    // is forced transparent by JS so the layer shows through. scale(1.12)
    // hides the blur's edge fringing.
    'html[data-bglayer]{background:var(--t-bg)!important}' +
    'html[data-bg-effect]{background:#050b1b!important}' +
    'html[data-bg-effect]::before{display:none!important}' +
    'html[data-bg-effect].theme-light body:not(.prefs-page) :is(.page-head,.page-header) :is(h1,p){color:#eef2ff!important}' +
    'html[data-bglayer]::before{content:"";position:fixed;inset:0;z-index:-1;pointer-events:none;' +
      'opacity:var(--t-bg-layer-opacity,1);' +
      // Shorthand, not background-image: page-owned --t-bgr values carry their
      // own position/size (e.g. "url(x) center top / cover"), which is only
      // valid as a full background value.
      'background:var(--t-bg-img-layer,none);' +
      'filter:blur(var(--t-bg-blur,18px));transform:scale(1.12)}' +
    'html[data-bglayer][data-noblur]::before{filter:none;transform:none}' +
    'html[data-bglayer][data-blur-drag]::before{transition:none}' +
    'input,textarea,select{background:var(--t-bg2);color:var(--t-fg);border:1px solid var(--t-bd);' +
      'padding:7px 11px;border-radius:var(--t-radius,8px);font-family:inherit;font-size:.9rem;transition:border-color var(--t-motion,.15s),box-shadow var(--t-motion,.15s)}' +
    'input:focus,textarea:focus,select:focus{outline:none;border-color:var(--t-ac);box-shadow:0 0 0 3px var(--t-gls)}' +
    'body:not(.mitch-design) button:not(#devtools-btn):not(#theme-btn):not(.tbg-btn):not(#sw-notif-btn):not(.msg-more):not(.msg-action){background:var(--t-bg2);color:var(--t-ac);' +
      'border:1px solid var(--t-bda);padding:7px 16px;border-radius:var(--t-radius,8px);cursor:pointer;' +
      'font-family:inherit;font-size:.88rem;font-weight:500;transition:all var(--t-motion,.15s)}' +
    'body:not(.mitch-design) button:not(#devtools-btn):not(#theme-btn):not(.tbg-btn):not(#sw-notif-btn):not(.msg-more):not(.msg-action):hover{background:var(--t-bg3);box-shadow:0 0 8px var(--t-gls)}' +
    '.theme-no-motion *{animation-duration:0s!important;transition-duration:0s!important;scroll-behavior:auto!important}' +
    'hr{border:none;border-top:1px solid var(--t-bd)}' +
    'a{color:var(--t-ac)}a:hover{color:var(--t-ac2)}' +
    'label{color:var(--t-fg2)}' +
    // Installed PWA (iOS Dynamic Island / home indicator): let the themed
    // background extend edge-to-edge, but keep content clear of the cutouts.
    '@supports (padding: env(safe-area-inset-top)) {' +
      '@media (display-mode: standalone) {' +
        'body:not(.encrypt-page):not(.cellar-page){padding-top:env(safe-area-inset-top)!important;padding-right:env(safe-area-inset-right)!important;' +
          'padding-bottom:env(safe-area-inset-bottom)!important;padding-left:env(safe-area-inset-left)!important}' +
        'body.encrypt-page,body.cellar-page{padding:0!important}' +
        'html{background:var(--t-bg)}' +
        'body.home-galaxy .hud-topbar{top:calc(14px + env(safe-area-inset-top))!important}' +
      '}' +
    '}';
  document.head.appendChild(baseStyle);

  var lightStyle = document.createElement('style');
  lightStyle.id = 'theme-light-overrides';
  lightStyle.textContent =
    '.theme-light .glass-card,.theme-light .card,' +
    '.theme-light .hud-topbar,.theme-light .hud-hero-card,.theme-light .category-group,.theme-light .hud-widget-card,.theme-light .hud-nudge-card,.theme-light #hud-terminal-overlay{' +
      'background:rgba(255,255,255,0.72)!important;backdrop-filter:blur(24px) saturate(180%)!important;-webkit-backdrop-filter:blur(24px) saturate(180%)!important;' +
      'border:1px solid rgba(0,0,0,0.08)!important;box-shadow:0 12px 32px rgba(0,0,0,0.04),inset 0 1px 0 rgba(255,255,255,0.8)!important;color:#0f1123!important;}' +
    '.theme-light #sw-notif-btn{background:rgba(255,255,255,0.75)!important;border:1px solid rgba(0,0,0,0.14)!important;color:var(--t-ac)!important;}' +
    '.theme-light #sw-notif-panel{background:rgba(250,250,255,0.97)!important;border:1px solid rgba(0,0,0,0.1)!important;box-shadow:0 18px 60px rgba(0,0,0,0.12)!important;}' +
    '.theme-light .sw-notif-head{border-bottom:1px solid rgba(0,0,0,0.08)!important;}' +
    '.theme-light .sw-notif-head span,.theme-light .sw-notif-title{color:#0f1123!important;}' +
    '.theme-light .sw-notif-body,.theme-light .sw-notif-detail{color:rgba(15,17,35,0.65)!important;}' +
    '.theme-light .sw-notif-item{background:rgba(0,0,0,0.025)!important;border:1px solid rgba(0,0,0,0.07)!important;}' +
    '.theme-light .sw-notif-empty{color:rgba(15,17,35,0.45)!important;}' +
    '.theme-light .sw-notif-head button,.theme-light .sw-notif-head a.sw-notif-settings,.theme-light .sw-notif-actions button,.theme-light .sw-notif-open{background:rgba(0,0,0,0.04)!important;border:1px solid rgba(0,0,0,0.1)!important;color:var(--t-ac)!important;}' +
    '.theme-light button:not(#devtools-btn):not(#theme-btn):not(.tbg-btn):not(#sw-notif-btn):not(.btn-primary):not(.auth-tab-btn):not(.msg-more):not(.msg-action):not(.bg-chip):not(.bg-chip-del):not(.chip):not(.soft-btn):not(.weather-summary):not(.primary-button):not(.secondary-button):not([data-theme]){background:rgba(255,255,255,0.7)!important;border:1px solid rgba(0,0,0,0.12)!important;color:var(--t-ac)!important;}' +
    '.theme-light button:not(#devtools-btn):not(#theme-btn):not(.tbg-btn):not(#sw-notif-btn):not(.btn-primary):not(.auth-tab-btn):not(.msg-more):not(.msg-action):not(.bg-chip):not(.bg-chip-del):not(.chip):not(.soft-btn):not(.weather-summary):not(.primary-button):not(.secondary-button):not([data-theme]):hover{background:rgba(255,255,255,0.9)!important;box-shadow:0 4px 16px rgba(0,0,0,0.08)!important;}' +
    '.theme-light input:not([type=range]):not([type=color]),.theme-light textarea,.theme-light select{background:rgba(255,255,255,0.7)!important;color:#0f1123!important;border:1px solid rgba(0,0,0,0.12)!important;}' +
    '.theme-light input::placeholder,.theme-light textarea::placeholder{color:rgba(15,17,35,0.4)!important;}' +
    '.theme-light .back-btn{color:var(--t-ac)!important;}' +
    '.theme-light #mitch-watermark{opacity:0.5!important;}' +
    '.theme-light #_ap{background:rgba(250,250,255,0.97)!important;border:1px solid rgba(0,0,0,0.1)!important;box-shadow:0 8px 40px rgba(0,0,0,0.12)!important;}' +
    '.theme-light #_ah{border-bottom:1px solid rgba(0,0,0,0.08)!important;color:#0f1123!important;}' +
    '.theme-light ._mu{background:rgba(0,0,0,0.06)!important;color:#0f1123!important;}' +
    '.theme-light ._ma{background:rgba(0,0,0,0.04)!important;color:rgba(15,17,35,0.85)!important;}' +
    '.theme-light #_at{background:rgba(0,0,0,0.05)!important;color:#0f1123!important;border:1px solid rgba(0,0,0,0.1)!important;}' +
    '.theme-light #_as{background:var(--t-ac)!important;color:#fff!important;}' +
    '.theme-light #home-games-mega,.theme-light #home-prox-mega,.theme-light .mega-game-copy strong{color:var(--t-fg)!important;}' +
    '.theme-light .mega-game-copy small{color:var(--t-fg2)!important;}' +
    '.theme-light #home-games-mega .mega-game-arrow,.theme-light #home-prox-mega .mega-game-arrow{color:var(--t-fg)!important;background:rgba(0,0,0,0.06)!important;}' +
    '.theme-light #happy-hour-nudge.inactive{background:rgba(0,0,0,0.03)!important;border-color:rgba(0,0,0,0.08)!important;color:var(--t-fg2)!important;}' +
    '.theme-light #happy-hour-text{color:var(--t-fg2)!important;}' +
    '.theme-light #happy-hour-nudge.active{background:rgba(34,211,238,0.12)!important;border:1px solid rgba(34,211,238,0.5)!important;color:#0f766e!important;box-shadow:0 0 12px rgba(34,211,238,0.1)!important;}' +
    '.theme-light #achievement-nudge{background:rgba(245,158,11,0.1)!important;border:1px solid rgba(245,158,11,0.4)!important;color:#b45309!important;}' +
    '.theme-light #admin-dashboard-card strong{color:var(--t-fg)!important;}' +
    '.theme-light #admin-dashboard-card small{color:var(--t-fg2)!important;opacity:0.8!important;}' +
    '.theme-light .wallet strong,.theme-light .fs-wallet strong,.theme-light .hist strong{color:var(--t-fg)!important;}' +
    '.theme-light .listing-title{color:var(--t-fg)!important;}' +
    '.theme-light .btn.sec{color:var(--t-fg)!important;background:rgba(0,0,0,0.05)!important;border-color:rgba(0,0,0,0.1)!important;}' +
    // School hubs design their own heading colors (white on maroon bands) —
    // don't force the light palette onto them.
    '.theme-light body:not(.school-hub) h1,.theme-light body:not(.school-hub) h2,.theme-light body:not(.school-hub) h3,.theme-light body:not(.school-hub) h4,.theme-light body:not(.school-hub) h5,.theme-light body:not(.school-hub) h6{color:var(--t-fg)!important;}' +
    '.theme-light .brand{color:var(--t-fg)!important;}' +
    '.theme-light .opt-btn:hover{color:var(--t-fg)!important;background:rgba(0,0,0,0.08)!important;}' +
    '.theme-light .choice.active{color:var(--t-fg)!important;background:rgba(56,189,248,0.18)!important;}' +
    '.theme-light .glass-card strong,.theme-light .card strong{color:var(--t-fg)!important;}' +
    '.theme-light .glass-card small,.theme-light .card small{color:var(--t-fg2)!important;}' +
    '.theme-light #greeting-email,.theme-light .hud-brand,.theme-light #greeting-clock,.theme-light .widget-title,.theme-light .links-heading h3,.theme-light .category-title,.theme-light .category-title small,.theme-light #preferences-card span,.theme-light #abToggleBar span{color:#0f1123!important;text-shadow:none!important;}' +
    '.theme-light .category-title{border-bottom:1px solid rgba(0,0,0,0.08)!important;}' +
    '.theme-light .hero-subtitle,.theme-light .links-heading p,.theme-light #greeting-text,.theme-light .hud-kicker,.theme-light .status-user-view{color:rgba(15,17,35,0.7)!important;}' +
    '.theme-light a.site-link{background:rgba(0,0,0,0.02)!important;border:1px solid rgba(0,0,0,0.05)!important;color:#0f1123!important;}' +
    '.theme-light a.site-link:hover{background:rgba(0,0,0,0.04)!important;border-color:var(--t-ac)!important;color:var(--t-ac)!important;}' +
    '.theme-light a.site-link .lbl{color:#0f1123!important;}' +
    '.theme-light a.site-link:hover .lbl{color:var(--t-ac)!important;}' +
    '.theme-light .hud-search-area .home-search-box{background:rgba(255,255,255,0.6)!important;border:1px solid rgba(0,0,0,0.08)!important;}' +
    '.theme-light .hud-search-area #home-search{color:#0f1123!important;}' +
    '.theme-light .progress-bar-bg{background:rgba(0,0,0,0.05)!important;}' +
    '.theme-light .status-pill{background:rgba(34,197,94,0.1)!important;border:1px solid rgba(34,197,94,0.2)!important;}' +
    '.theme-light .hud-terminal-toggle-btn{background:rgba(0,0,0,0.03)!important;border:1px solid rgba(0,0,0,0.08)!important;color:#0f1123!important;}' +
    '.theme-light .hud-terminal-toggle-btn:hover{background:rgba(0,0,0,0.06)!important;border-color:var(--t-ac)!important;}' +
    // Neon-gold name colors are unreadable on the light background — swap to
    // a dark amber and drop the glow so usernames stay legible.
    '.theme-light .name.gold_glow,.theme-light .entry-name.gold_glow,.theme-light .display-name.gold_glow,.theme-light .author.gold_glow{color:#b45309!important;text-shadow:none!important;}' +
    '.theme-light .badge-premium,.theme-light .badge-shop,.theme-light .premium-label{color:#b45309!important;}';
  lightStyle.textContent = lightStyle.textContent.replaceAll('.theme-light button:not', '.theme-light body:not(.mitch-design):not(.prefs-page):not(.school-hub) button:not');
  document.head.appendChild(lightStyle);

  function buildToggle() {
    if (document.getElementById('theme-btn')) return syncToggleBtn();
    var btn = document.createElement('button');
    btn.id = 'theme-btn';
    btn.type = 'button';
    btn.style.cssText =
      'width:30px;height:30px;border-radius:50%;padding:0;margin:0;' +
      'display:inline-flex;align-items:center;justify-content:center;flex-shrink:0;' +
      'cursor:pointer;line-height:1;font-family:inherit;font-size:0;' +
      'border:1px solid rgba(255,255,255,0.16);background:rgba(255,255,255,0.06);' +
      'color:rgba(255,255,255,0.85);opacity:0.85;' +
      'transition:transform var(--t-motion,.15s),opacity var(--t-motion,.15s),background var(--t-motion,.15s);';
    btn.onmouseenter = function () { btn.style.transform = 'scale(1.08)'; btn.style.opacity = '1'; };
    btn.onmouseleave = function () { btn.style.transform = ''; btn.style.opacity = '0.85'; };
    btn.onclick = function (e) {
      e.stopPropagation();
      var next = getCookie() === 'light' ? 'dark' : 'light';
      setCookie(next);
      applyTheme(next); // applyTheme dispatches the themechange event
    };

    var mounted = mountToggle(btn);
    if (!mounted) {
      btn.style.position = 'fixed';
      btn.style.right = '20px';
      btn.style.top = '20px';
      btn.style.zIndex = '999999';
      document.body.appendChild(btn);
      // The shared topbar may be injected after this script runs; re-mount when it shows up.
      var retry = function () { if (mountToggle(btn)) { obs.disconnect(); } };
      var obs = null;
      if (window.MutationObserver) {
        obs = new MutationObserver(retry);
        obs.observe(document.documentElement, { childList: true, subtree: true });
      }
      document.addEventListener('DOMContentLoaded', retry);
      window.addEventListener('load', retry);
    }
    syncToggleBtn();
  }

  function mountToggle(btn) {
    var topbar = document.getElementById('site-topbar') || document.getElementById('app-topbar');
    if (!topbar) return false;
    btn.style.position = '';
    btn.style.right = '';
    btn.style.top = '';
    btn.style.zIndex = '';
    if (btn.parentNode !== topbar) topbar.appendChild(btn);
    return true;
  }

  function injectThemeToggle() {
    if (document.body) { buildToggle(); }
    else { document.addEventListener('DOMContentLoaded', buildToggle); }
  }
  injectThemeToggle();

  function addWatermark() {
    if (location.pathname.endsWith('/encrypt.html')) return;
    if (location.pathname.startsWith('/encrypt')) return;
    // rjuhsd.school is a clean school-branded hub — no mitch watermark or Discord button there.
    if (location.hostname === 'rjuhsd.school' || location.hostname.endsWith('.rjuhsd.school')) return;
    if (location.pathname.startsWith('/rjuhsd')) return;
    if (!document.getElementById('mitch-watermark')) {
      var wm = document.createElement('img');
      wm.id = 'mitch-watermark';
      wm.src = '/icon-192.png';
      wm.style.cssText = 'position:fixed;right:15px;bottom:15px;width:32px;height:32px;opacity:0.7;pointer-events:none;z-index:999998;';
      document.body.appendChild(wm);
    }
  }
  if (document.body) { addWatermark(); }
  else { document.addEventListener('DOMContentLoaded', addWatermark); }

  window.__theme = {
    apply: function (name) { applyTheme(normalize(name)); },
    get: getCookie,
    themes: T,
    backgrounds: THEME_BGS,
    preparePreferenceSnapshot: preparePreferenceSnapshot,
    getBackground: getBackground,
    mergeBackgrounds: function (items) {
      return THEME_BGS.filter(function (bg) { return bg.effect; }).concat(items.filter(function (bg) { return !THEME_BGS.some(function (entry) { return entry.effect && entry.url === bg.url; }); }));
    },
    setBg: function(url) { setBgImgCookie(url); applyBgImg(url || getEffectiveBgImg()); },
    setBlur: function(px) {
      setPref('bgblur', clamp(Number(px) || 0, 0, 40));
      applyCustomizationPrefs();
    },
    applyMaterial: applyMaterialMode,
    adapt: function () { applyAdaptiveTheme(); },
    canUseGlass: canUseGlass
  };

  // Drag-to-blur: press an empty area of the page and drag vertically to
  // change the wallpaper blur live. Mouse/pen only — a vertical drag on
  // touch IS a scroll, so hijacking it would break every page. Disabled via
  // the theme_bgdrag=off pref.
  (function dragToBlur() {
    var HUD = null;
    function hud() {
      if (HUD) return HUD;
      HUD = document.createElement('div');
      HUD.id = 'bg-blur-hud';
      HUD.style.cssText = 'position:fixed;right:18px;bottom:18px;z-index:10000;' +
        'padding:7px 13px;border-radius:999px;border:1px solid var(--t-bda,rgba(255,255,255,.2));' +
        'background:rgba(10,10,16,.82);color:var(--t-fg,#fff);font:800 11px/1 system-ui,sans-serif;' +
        'letter-spacing:.04em;pointer-events:none;opacity:0;transition:opacity .25s;';
      (document.body || document.documentElement).appendChild(HUD);
      return HUD;
    }
    var active = false, startY = 0, startBlur = 8, pid = -1;
    function emptyTarget(el) {
      if (!el || el.nodeType !== 1) return false;
      if (el.closest && el.closest('a,button,input,select,textarea,label,summary,[contenteditable],img,canvas,video,' +
        '.msg,.card,.chip,.portal-shell,.prefs-page,#msgs,#sidebar,nav,header,footer,table,pre,code')) return false;
      return (el.textContent || '').trim() === '';
    }
    document.addEventListener('pointerdown', function(e) {
      if (e.pointerType !== 'mouse' && e.pointerType !== 'pen') return;
      if (e.button !== 0) return;
      if (getPref('bgdrag', 'on') === 'off') return;
      if (!document.documentElement.hasAttribute('data-bglayer')) return;
      if (!emptyTarget(e.target)) return;
      var cur = clamp(getPref('bgblur', '8'), 0, 40);
      if (cur === 0) return;
      active = true; startY = e.clientY; startBlur = cur; pid = e.pointerId;
    });
    document.addEventListener('pointermove', function(e) {
      if (!active || e.pointerId !== pid) return;
      var dy = e.clientY - startY;
      if (Math.abs(dy) < 8) return;
      try { e.target.setPointerCapture(pid); } catch (_) {}
      document.documentElement.setAttribute('data-blur-drag', '1');
      var px = clamp(Math.round(startBlur - dy / 6), 0, 40);
      document.documentElement.style.setProperty('--t-bg-blur', px + 'px');
      var h = hud();
      h.textContent = 'Blur ' + px + 'px';
      h.style.opacity = '1';
    });
    function end(e) {
      if (!active || (e.pointerId !== undefined && e.pointerId !== pid)) return;
      active = false;
      document.documentElement.removeAttribute('data-blur-drag');
      if (e.type === 'pointerup' || e.type === 'pointercancel') {
        var px = clamp(getPref('bgblur', '8'), 0, 40);
        try {
          var inline = document.documentElement.style.getPropertyValue('--t-bg-blur');
          if (inline) px = clamp(parseInt(inline, 10) || 0, 0, 40);
        } catch (_) {}
        setPref('bgblur', px);
        applyCustomizationPrefs();
        if (HUD) HUD.style.opacity = '0';
      }
      pid = -1;
    }
    document.addEventListener('pointerup', end);
    document.addEventListener('pointercancel', end);
  })();

  // Pointer-follow tilt on liquid-glass pages
  (function ensureLiquidGlassJs() {
    if (document.getElementById('liquid-glass-js')) return;
    var s = document.createElement('script');
    s.id = 'liquid-glass-js';
    s.src = '/liquid-glass.js';
    s.defer = true;
    (document.head || document.documentElement).appendChild(s);
  })();

  // ── Visual Effects ──────────────────────────────────────────────────────────
  var cleanupVFX = null;
  function applyVFX() {
    if (cleanupVFX) { cleanupVFX(); cleanupVFX = null; }
    var vfx = {};
    try { vfx = JSON.parse(localStorage.getItem('_prefVFX') || '{}') || {}; } catch (_) {}
    var existing = document.getElementById('mitch-vfx-canvas');
    if (existing) existing.remove();

    var any = vfx.snow || vfx.stars || vfx.rain || vfx.particles;
    if (!any) return;

    var canvas = document.createElement('canvas');
    canvas.id = 'mitch-vfx-canvas';
    canvas.style.cssText = 'position:fixed;top:0;left:0;width:100%;height:100%;pointer-events:none;z-index:-1;opacity:0.6;';
    document.body.appendChild(canvas);

    var ctx = canvas.getContext('2d');
    if (!ctx) { canvas.remove(); return; }
    var frame = 0;
    var motionQuery = matchMedia('(prefers-reduced-motion: reduce)');
    var w, h;
    function resize() {
      var dpr = Math.min(window.devicePixelRatio || 1, 2);
      w = canvas.width = Math.floor(window.innerWidth * dpr);
      h = canvas.height = Math.floor(window.innerHeight * dpr);
      canvas.style.width = window.innerWidth + 'px';
      canvas.style.height = window.innerHeight + 'px';
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      w = window.innerWidth;
      h = window.innerHeight;
    }
    window.addEventListener('resize', resize);
    resize();

    var snowList = [];
    var starList = [];
    var rainList = [];
    var partList = [];
    if (vfx.snow) {
      for (var i=0; i<80; i++) snowList.push({ x:Math.random()*w, y:Math.random()*h, r:Math.random()*2.5+1, v:Math.random()*0.8+0.4 });
    }
    if (vfx.stars) {
      for (var i=0; i<100; i++) starList.push({ x:Math.random()*w, y:Math.random()*h, r:Math.random()*1.5, o:Math.random(), ov:Math.random()*0.02 });
    }
    if (vfx.rain) {
      for (var i=0; i<50; i++) rainList.push({ x:Math.random()*w, y:Math.random()*h, l:Math.random()*18+8, v:Math.random()*8+8 });
    }
    if (vfx.particles) {
      for (var i=0; i<35; i++) partList.push({ x:Math.random()*w, y:Math.random()*h, r:Math.random()*3+2, vx:(Math.random()-0.5)*0.4, vy:(Math.random()-0.5)*0.4 });
    }

    var cachedAccent = '#7c3aed';
    function updateCachedAccent() {
      cachedAccent = getComputedStyle(document.documentElement).getPropertyValue('--t-ac').trim() || '#7c3aed';
    }
    updateCachedAccent();
    window.addEventListener('themecustomize', updateCachedAccent);

    var lastFrame = 0;
    var FRAME_MIN_MS = 1000 / 30;
    function animate(now) {
      if (!canvas.isConnected || document.hidden) return;
      if (!motionQuery.matches && !document.documentElement.classList.contains('theme-no-motion')) {
        frame = requestAnimationFrame(animate);
      }
      if (now && now - lastFrame < FRAME_MIN_MS) return;
      lastFrame = now || performance.now();

      ctx.clearRect(0, 0, w, h);

      // 1. Batch Snow
      if (snowList.length) {
        ctx.fillStyle = '#fff';
        ctx.beginPath();
        for (var i = 0; i < snowList.length; i++) {
          var p = snowList[i];
          ctx.moveTo(p.x + p.r, p.y);
          ctx.arc(p.x, p.y, p.r, 0, Math.PI * 2);
          p.y += p.v; p.x += Math.sin(p.y / 30) * 0.5;
          if (p.y > h) p.y = -10; if (p.x > w) p.x = 0; if (p.x < 0) p.x = w;
        }
        ctx.fill();
      }

      // 2. Stars
      if (starList.length) {
        for (var i = 0; i < starList.length; i++) {
          var p = starList[i];
          ctx.fillStyle = 'rgba(255,255,255,' + p.o + ')';
          ctx.fillRect(p.x - p.r, p.y - p.r, p.r * 2, p.r * 2);
          p.o += p.ov; if (p.o > 1 || p.o < 0) p.ov *= -1;
        }
      }

      // 3. Batch Rain
      if (rainList.length) {
        ctx.strokeStyle = 'rgba(255,255,255,0.3)';
        ctx.lineWidth = 1;
        ctx.beginPath();
        for (var i = 0; i < rainList.length; i++) {
          var p = rainList[i];
          ctx.moveTo(p.x, p.y);
          ctx.lineTo(p.x + p.v / 4, p.y + p.l);
          p.y += p.v; p.x += p.v / 4;
          if (p.y > h) { p.y = -20; p.x = Math.random() * w; }
        }
        ctx.stroke();
      }

      // 4. Batch Particles
      if (partList.length) {
        ctx.fillStyle = cachedAccent;
        ctx.globalAlpha = 0.2;
        ctx.beginPath();
        for (var i = 0; i < partList.length; i++) {
          var p = partList[i];
          ctx.moveTo(p.x + p.r, p.y);
          ctx.arc(p.x, p.y, p.r, 0, Math.PI * 2);
          p.x += p.vx; p.y += p.vy;
          if (p.x < 0 || p.x > w) p.vx *= -1;
          if (p.y < 0 || p.y > h) p.vy *= -1;
        }
        ctx.fill();
        ctx.globalAlpha = 1.0;
      }
    }
    function resumeVFX() {
      cancelAnimationFrame(frame);
      frame = 0;
      animate();
    }
    document.addEventListener('visibilitychange', resumeVFX);
    motionQuery.addEventListener('change', resumeVFX);
    cleanupVFX = function () {
      cancelAnimationFrame(frame);
      window.removeEventListener('resize', resize);
      window.removeEventListener('themecustomize', updateCachedAccent);
      document.removeEventListener('visibilitychange', resumeVFX);
      motionQuery.removeEventListener('change', resumeVFX);
      canvas.remove();
    };
    resumeVFX();
  }

  function applyCustomCSS() {
    var display = JSON.parse(localStorage.getItem('_prefDisplay') || '{}');
    var existing = document.getElementById('mitch-custom-css');
    if (existing) existing.remove();
    if (display.customCSS) {
      var style = document.createElement('style');
      style.id = 'mitch-custom-css';
      style.textContent = display.customCSS;
      document.head.appendChild(style);
    }
  }

  function applyQuickAccess() {
    var tc = JSON.parse(localStorage.getItem('_prefTools') || '{}');
    var existing = document.getElementById('mitch-quick-access');
    if (existing) existing.remove();
    if (!tc.quickAccess) return;

    var bar = document.createElement('div');
    bar.id = 'mitch-quick-access';
    bar.style.cssText = 'position:fixed;right:10px;top:50%;transform:translateY(-50%);z-index:10000;display:flex;flex-direction:column;gap:8px;padding:8px;background:rgba(10,10,10,0.4);border:1px solid rgba(255,255,255,0.1);border-radius:12px;box-shadow:0 8px 32px rgba(0,0,0,0.5);transition:opacity 0.2s;';

    var links = [
      { h:'/', i:'🏠', t:'Home' },
      { h:'/games/', i:'🎮', t:'Games' },
      { h:'/preferences/#account', i:'👤', t:'Account' },
      { h:'/encrypt.html', i:'💬', t:'Chat' },
      { h:'/canvas/', i:'🎨', t:'Canvas' },
      { h:'/shop/', i:'🛒', t:'Market' },
      { h:'/inventory/', i:'🎒', t:'Inventory' },
      { h:'/preferences/', i:'⚙️', t:'Settings' }
    ];

    links.forEach(function(l) {
      var a = document.createElement('a');
      a.href = l.h; a.title = l.t;
      a.style.cssText = 'width:34px;height:34px;display:flex;align-items:center;justify-content:center;background:rgba(255,255,255,0.05);border-radius:8px;text-decoration:none;font-size:18px;transition:all 0.2s;';
      a.innerHTML = l.i;
      a.onmouseover = function() { this.style.background = 'rgba(255,255,255,0.1)'; this.style.transform = 'scale(1.1)'; };
      a.onmouseout = function() { this.style.background = 'rgba(255,255,255,0.05)'; this.style.transform = 'scale(1)'; };
      bar.appendChild(a);
    });

    document.body.appendChild(bar);
  }

  if (document.body) { applyVFX(); applyCustomCSS(); applyQuickAccess(); }
  else { document.addEventListener('DOMContentLoaded', function(){ applyVFX(); applyCustomCSS(); applyQuickAccess(); }); }
  window.addEventListener('themecustomize', function() { applyAdaptiveTheme(); applyVFX(); applyCustomCSS(); applyQuickAccess(); });
})();
