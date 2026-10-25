// PWA installation controls for the school hub.
// Captures the browser's beforeinstallprompt and offers a dismissible banner;
// Browsers without an install prompt receive platform-specific instructions.

(function () {
  'use strict';

  var DISMISS_KEY = 'mitch_pwa_install_dismissed';
  var deferredPrompt = null;
  var bannerShown = false;

  function isStandalone() {
    try {
      return window.matchMedia('(display-mode: standalone)').matches ||
             window.navigator.standalone === true;
    } catch (e) { return false; }
  }

  function isIos() {
    return /iphone|ipad|ipod/i.test(navigator.userAgent) ||
           (navigator.platform === 'MacIntel' && navigator.maxTouchPoints > 1);
  }

  function dismissedBefore() {
    try { return localStorage.getItem(DISMISS_KEY) === '1'; } catch (e) { return false; }
  }

  function rememberDismissal() {
    try { localStorage.setItem(DISMISS_KEY, '1'); } catch (e) {}
  }

  // Only surface the banner on real pages, not inside embedded game frames.
  // Install promotion is rjuhsd.school-only (phones only): the mitch.pro PWA
  // stays quiet — the school app is the one people actually want installed.
  function allowedHere() {
    try {
      if (window.top !== window.self) return false;
      if (window.location.pathname.startsWith('/games/')) return false;
      var h = String(window.location.hostname).toLowerCase();
      var isRj = h === 'rjuhsd.school' || h.endsWith('.rjuhsd.school') || window.location.pathname.indexOf('/rjuhsd') === 0;
      if (!isRj) return false;
      return true;
    } catch (e) { return false; }
  }

  function isPhone() {
    var ua = navigator.userAgent || '';
    if (/Mobi|Android|iPhone|iPad|iPod/i.test(ua)) return true;
    // iPads report as Macintosh with touch points.
    return /MacIntel/i.test(navigator.platform || '') && navigator.maxTouchPoints > 1;
  }

  function injectStyles() {
    if (document.getElementById('mitch-pwa-styles')) return;
    var s = document.createElement('style');
    s.id = 'mitch-pwa-styles';
    s.textContent = `
      #mitchPwaBanner {
        position: fixed; left: 50%; bottom: 18px; z-index: 99990;
        display: grid; grid-template-columns: minmax(0, 1fr) auto auto;
        align-items: center; gap: 12px; width: min(440px, calc(100vw - 32px));
        padding: 16px; border: 1px solid var(--line, #ffffff1a); border-radius: 14px;
        background: var(--card, #1a1b20); color: var(--ink, #f4f4f6);
        box-shadow: 0 8px 30px #0003; font-family: var(--sans, system-ui, sans-serif);
        opacity: 0; transform: translate(-50%, 10px);
        transition: opacity .2s, transform .2s;
      }
      #mitchPwaBanner.show { opacity: 1; transform: translate(-50%, 0); }
      #mitchPwaBanner .mitch-pwa-copy { min-width: 0; }
      #mitchPwaBanner .mitch-pwa-copy b { display: block; font-size: 13px; font-weight: 600; }
      #mitchPwaBanner .mitch-pwa-copy span {
        display: block; margin-top: 4px; font-size: 11px; line-height: 1.5;
        color: var(--muted, #a0a1ac);
      }
      #mitchPwaBanner button {
        min-height: 36px; padding: 8px 12px; border: 1px solid var(--line, #ffffff1a);
        border-radius: 8px; background: var(--raised, #222329); color: var(--ink, #f4f4f6);
        cursor: pointer; font: 500 11px var(--sans, system-ui, sans-serif); white-space: nowrap;
      }
      #mitchPwaBanner button:focus-visible { outline: 2px solid var(--wine, #d5a2ad); outline-offset: 3px; }
      #mitchPwaBanner #mitchPwaInstall {
        background: var(--wine, #d5a2ad); color: var(--paper, #111215); border-color: transparent;
      }
      @media (max-width: 560px) {
        #mitchPwaBanner { bottom: calc(86px + env(safe-area-inset-bottom)); gap: 12px 8px; }
        #mitchPwaBanner .mitch-pwa-copy { grid-column: 1 / -1; }
        #mitchPwaLater { grid-column: 2; }
      }
      @media (max-width: 850px) and (min-width: 561px) { #mitchPwaBanner { bottom: 86px; } }
      @media (prefers-reduced-motion: reduce) { #mitchPwaBanner { transition: none; } }
    `;
    document.head.appendChild(s);
  }

  function showBanner(canAutoInstall) {
    if (bannerShown || isStandalone() || dismissedBefore() || !allowedHere()) return;
    if (!document.body) { document.addEventListener('DOMContentLoaded', function () { showBanner(canAutoInstall); }); return; }
    bannerShown = true;
    injectStyles();

    var copy = canAutoInstall
      ? 'Add bell schedules to your home screen.'
      : isIos()
        ? 'Tap Share, then Add to Home Screen.'
        : 'Open your browser menu and choose Install app or Add to Home Screen.';

    var banner = document.createElement('div');
    banner.id = 'mitchPwaBanner';
    banner.setAttribute('role', 'region');
    banner.setAttribute('aria-label', 'Install rjuhsd.school app');
    banner.innerHTML =
      '<span class="mitch-pwa-copy"><b>Install rjuhsd.school</b><span></span></span>' +
      '<button id="mitchPwaLater" type="button">Not now</button>' +
      '<button id="mitchPwaInstall" type="button">Install</button>';
    banner.querySelector('.mitch-pwa-copy span').textContent = copy;
    if (!canAutoInstall) banner.querySelector('#mitchPwaInstall').textContent = 'Got it';
    document.body.appendChild(banner);
    requestAnimationFrame(function () { banner.classList.add('show'); });

    function close(remember) {
      if (remember) rememberDismissal();
      banner.classList.remove('show');
      setTimeout(function () { if (banner.parentNode) banner.remove(); bannerShown = false; }, 260);
    }

    banner.querySelector('#mitchPwaLater').onclick = function () { close(true); };
    banner.querySelector('#mitchPwaInstall').onclick = function () {
      if (deferredPrompt) {
        var prompt = deferredPrompt;
        Promise.resolve(prompt.prompt()).then(function () { return prompt.userChoice; })
          .catch(function () {})
          .finally(function () { deferredPrompt = null; close(false); });
      } else {
        // Manual installation stays on the school page while the visitor follows the browser instructions.
        close(true);
      }
    };
  }

  function notifyInstallable() {
    window.dispatchEvent(new CustomEvent('pwa-installable'));
    document.querySelectorAll('#pwa-install-btn, .pwa-install-btn').forEach(function(b) {
      b.style.display = 'inline-flex';
    });
  }

  window.__promptPwaInstall = function() {
    if (deferredPrompt) {
      deferredPrompt.prompt();
      return deferredPrompt.userChoice.finally(function () { deferredPrompt = null; });
    } else {
      showBanner(false);
    }
  };

  document.addEventListener('click', function(e) {
    var btn = e.target.closest('#pwa-install-btn, .pwa-install-btn');
    if (btn) {
      e.preventDefault();
      window.__promptPwaInstall();
    }
  });

  window.addEventListener('beforeinstallprompt', function (e) {
    e.preventDefault();
    deferredPrompt = e;
    notifyInstallable();
    if (!isStandalone() && !dismissedBefore() && allowedHere()) {
      // Give the page a moment to settle before offering.
      setTimeout(function () { showBanner(true); }, 2500);
    }
  });

  window.addEventListener('appinstalled', function () {
    rememberDismissal();
    document.querySelectorAll('#pwa-install-btn, .pwa-install-btn').forEach(function(b) {
      b.style.display = 'none';
    });
  });

  // iOS never fires beforeinstallprompt — offer the manual flow instead,
  // but only once and never inside the installed app.
  if (isIos() && !isStandalone() && !dismissedBefore() && allowedHere()) {
    notifyInstallable();
    setTimeout(function () { showBanner(false); }, 4000);
  }
})();