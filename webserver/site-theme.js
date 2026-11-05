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

  // Holiday themes by date (local calendar):
  //   Halloween    Oct 15 - Oct 31
  //   Thanksgiving Nov 1 - Thanksgiving Day (fourth Thursday of November)
  //   Christmas    the day after Thanksgiving - Dec 26
  //   New Year's   Dec 27 - Jan 3
  //   Valentine's  Feb 7 - Feb 14
  // The palettes live in theme-tokens.css (html[data-holiday]); a custom accent
  // from Preferences still wins. Keep seasonFor in mitch_lib::seasons in step.
  function thanksgivingDay(year) {
    var firstThursday = 1 + ((4 - new Date(year, 10, 1).getDay() + 7) % 7);
    return firstThursday + 21;
  }

  function holidayFor(now) {
    var month = now.getMonth(), day = now.getDate();
    if (month === 9) return day >= 15 ? 'halloween' : '';
    if (month === 10) return day <= thanksgivingDay(now.getFullYear()) ? 'thanksgiving' : 'christmas';
    if (month === 11) return day <= 26 ? 'christmas' : 'newyear';
    if (month === 0) return day <= 3 ? 'newyear' : '';
    if (month === 1) return day >= 7 && day <= 14 ? 'valentines' : '';
    return '';
  }

  // A short greeting for the day itself, or '' on other days. When a theme is
  // being previewed (?holiday= or a pinned server) its default greeting is used.
  var GREETINGS = {
    halloween: 'Happy Halloween', thanksgiving: 'Happy Thanksgiving', christmas: 'Merry Christmas',
    newyear: 'Happy New Year', valentines: "Happy Valentine's Day"
  };

  function greetingFor(now) {
    var month = now.getMonth(), day = now.getDate();
    if (month === 9 && day === 31) return GREETINGS.halloween;
    if (month === 10 && day === thanksgivingDay(now.getFullYear())) return GREETINGS.thanksgiving;
    if (month === 11 && (day === 24 || day === 25)) return GREETINGS.christmas;
    if (month === 11 && day === 31) return "Happy New Year's Eve";
    if (month === 0 && day === 1) return GREETINGS.newyear;
    if (month === 1 && day === 14) return GREETINGS.valentines;
    return '';
  }

  // A server run can pin the season (MITCH_HOLIDAY_THEME, see docs/holiday-themes.md):
  // the server puts window.__holidayForce in front of this script.
  var SEASONS = { halloween: 1, thanksgiving: 1, christmas: 1, newyear: 1, valentines: 1 };

  // The theme that is in season: the server's pinned one, else by date.
  function holidaySeason() {
    var pinned = window.__holidayForce;
    if (pinned === 'none') return '';
    if (SEASONS[pinned]) return pinned;
    return holidayFor(new Date());
  }

  function applyHoliday() {
    var root = document.documentElement;
    var forced = (location.search.match(/[?&]holiday=([a-z]+)/) || [])[1];
    var name;
    if (forced === 'none') name = '';
    else if (SEASONS[forced]) name = forced;
    else {
      var preference = '';
      try { preference = localStorage.getItem('theme_holiday') || ''; } catch (_) {}
      name = preference === 'off' ? '' : holidaySeason();
    }
    if (name) root.setAttribute('data-holiday', name);
    else root.removeAttribute('data-holiday');
    if (window.__holidayFx) window.__holidayFx.refresh();
    else if (name && !loadingFx) {
      loadingFx = true;
      var script = document.createElement('script');
      script.src = '/holiday-fx.js?v=4';
      script.defer = true;
      document.head.appendChild(script);
    }
  }
  var loadingFx = false;

  function apply() {
    applyHoliday();
    applyMode(readCookie('theme'));
    applyAccent(readCookie('accent'));
    applyContrast(readCookie('contrast'));
    applyLayout();
    // Repaint the effect once the light/dark mode is final.
    if (window.__holidayFx) window.__holidayFx.refresh();
  }

  apply();

  // The greeting to show now: nothing when the holiday theme is off, the
  // preview greeting when a theme is forced, else the day's own greeting.
  function holidayGreeting() {
    var name = document.documentElement.getAttribute('data-holiday') || '';
    if (!name) return '';
    var forced = (location.search.match(/[?&]holiday=([a-z]+)/) || [])[1];
    var pinned = window.__holidayForce;
    if (SEASONS[forced] || SEASONS[pinned]) return GREETINGS[name] || '';
    return greetingFor(new Date());
  }

  function holidayOff() {
    try { return localStorage.getItem('theme_holiday') === 'off'; } catch (_) { return false; }
  }

  // Turns the holiday theme off (or back on) for every page; Preferences shows
  // the same setting.
  function setHolidayOff(off) {
    try {
      if (off) localStorage.setItem('theme_holiday', 'off');
      else localStorage.removeItem('theme_holiday');
    } catch (_) {}
    apply();
  }

  // Holiday effects (falling leaves, snow, embers, confetti, hearts) can be
  // turned off on their own; the holiday colors stay. holiday-fx.js reads the
  // same key.
  function holidayFxOff() {
    try { return localStorage.getItem('theme_holidayfx') === 'off'; } catch (_) { return false; }
  }
  function setHolidayFxOff(off) {
    try {
      if (off) localStorage.setItem('theme_holidayfx', 'off');
      else localStorage.removeItem('theme_holidayfx');
    } catch (_) {}
    apply();
  }

  window.__siteTheme = {
    holidayFor: holidayFor,
    holidaySeason: holidaySeason,
    greetingFor: greetingFor,
    holidayGreeting: holidayGreeting,
    holidayOff: holidayOff,
    setHolidayOff: setHolidayOff,
    holidayFxOff: holidayFxOff,
    setHolidayFxOff: setHolidayFxOff,
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
