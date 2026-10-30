(function () {
  'use strict';

  var SYNC_TS_KEY  = '_sync_ts';
  var INTERVAL_MS  = 5 * 60 * 1000;
  var RESTORED_KEY = '_just_restored';

  // Preference keys to sync
  var PREF_KEYS = ['_prefAI', '_prefHomepage', '_prefSync', '_prefDisplay', '_prefTools', '_prefVFX', '_prefPrivacy'];

  // Never sync Matrix login/keys or the site's E2E key material: they would
  // put a live chat token and the keys that decrypt messages in the server
  // copy. Mirrors is_sensitive_sync_key in routes/members.rs and /saves/.
  function isSensitiveKey(key) {
    var k = String(key).toLowerCase();
    return k.indexOf('cinny_') === 0 || k.indexOf('mx_') === 0 || k.indexOf('matrix') !== -1 ||
      k.indexOf('e2e') !== -1 || k.indexOf('passphrase') !== -1 || k.indexOf('access_token') !== -1 ||
      k.indexOf('privatekey') !== -1 || k.indexOf('private_key') !== -1;
  }

  function getSnap() {
    var obj = {};
    for (var i = 0; i < localStorage.length; i++) {
      var k = localStorage.key(i);
      if (k === SYNC_TS_KEY || k === RESTORED_KEY || k.startsWith('_panic') || k.startsWith('_cloak') || isSensitiveKey(k)) continue;
      
      var raw = localStorage.getItem(k);
      try { obj[k] = JSON.parse(raw); }
      catch (e) { obj[k] = raw; }
    }
    return obj;
  }

  function applySnap(snap) {
    if (window.__theme && window.__theme.preparePreferenceSnapshot) snap = window.__theme.preparePreferenceSnapshot(snap);
    for (var k in snap) {
      if (!snap.hasOwnProperty(k) || isSensitiveKey(k)) continue;
      try {
        var v = snap[k];
        localStorage.setItem(k, typeof v === 'string' ? v : JSON.stringify(v));
      } catch (e) {}
    }
  }

  function save() {
    var prefSync = JSON.parse(localStorage.getItem('_prefSync') || '{}');
    if (prefSync.enabled === false) return;

    var snap = getSnap();
    var ts   = Date.now();
    localStorage.setItem(SYNC_TS_KEY, ts);
    fetch('/api/userdata', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      credentials: 'include',
      body: JSON.stringify({ _snapshot: snap, _snapshot_ts: ts }),
    }).then(function(r) {
      if (r.status === 413) {
        r.json().then(function(d) {
          if (window._syncOnQuotaExceeded) window._syncOnQuotaExceeded(d);
        }).catch(function(){});
      } else {
        r.text().catch(function(){});
      }
    }).catch(function () {});
  }

  function saveBeacon() {
    var prefSync = JSON.parse(localStorage.getItem('_prefSync') || '{}');
    if (prefSync.enabled === false) return;

    var snap = getSnap();
    var ts   = Date.now();
    localStorage.setItem(SYNC_TS_KEY, ts);
    var body = JSON.stringify({ _snapshot: snap, _snapshot_ts: ts });
    if (navigator.sendBeacon) {
      navigator.sendBeacon('/api/userdata', new Blob([body], { type: 'application/json' }));
    } else {
      fetch('/api/userdata', {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        credentials: 'include', body: body,
        keepalive: true,
      }).catch(function () {});
    }
  }

  var _restoreChecked = false;
  function checkRestore() {
    if (_restoreChecked) return;
    _restoreChecked = true;

    fetch('/api/userdata', { credentials: 'include' })
      .then(function (r) {
        if (!r.ok) { r.text().catch(function(){}); return null; }
        return r.json();
      })
      .then(function (data) {
        if (!data || !data._snapshot || !data._snapshot_ts) return;
        var localTs  = parseInt(localStorage.getItem(SYNC_TS_KEY) || '0', 10);
        var serverTs = data._snapshot_ts;
        if (serverTs > localTs + 10000) {
          showRestorePrompt(data._snapshot, serverTs);
        }
      })
      .catch(function () {});
  }

  function showRestorePrompt(snap, serverTs) {
    var lines = [];
    try {
      var chess = typeof snap.chess_elo === 'string'
        ? JSON.parse(snap.chess_elo) : snap.chess_elo;
      if (chess && chess.elo) {
        lines.push('Chess ELO: ' + chess.elo +
          ' (' + (chess.wins||0) + 'W/' + (chess.losses||0) + 'L/' + (chess.draws||0) + 'D)');
      }
    } catch (e) {}
    var detail = lines.length ? lines.join(' • ') : 'Saved progress found on MitchSync.';

    var savedDate = new Date(serverTs).toLocaleString('en-US', {
      month: 'short', day: 'numeric',
      hour: 'numeric', minute: '2-digit', hour12: true,
    });

    var style = document.createElement('style');
    style.textContent = '@keyframes _sfadein{from{opacity:0}to{opacity:1}}';
    document.head.appendChild(style);

    // Colors ride the --t-* theme tokens (set by theme.js) with dark fallbacks,
    // so this dialog follows the light theme like the rest of the site.
    var overlay = document.createElement('div');
    overlay.id = '_sync_restored_overlay';
    overlay.innerHTML =
      '<div style="position:fixed;inset:0;background:rgba(0,0,0,.55);z-index:999998;' +
        'display:flex;align-items:center;justify-content:center;' +
        'font-family:system-ui,sans-serif;animation:_sfadein .25s ease;">' +
        '<div style="background:var(--t-bg2,#1e1e1e);border:1px solid var(--t-bd,#444);border-radius:18px;' +
          'padding:2rem 2.5rem;max-width:400px;width:90%;text-align:center;' +
          'color:var(--t-fg,#e8e6e3);' +
          'box-shadow:0 20px 60px rgba(0,0,0,.6);">' +
          '<div style="font-size:2rem;margin-bottom:.5rem;">☁</div>' +
          '<div style="font-size:1.15rem;font-weight:700;color:var(--t-fg,#e8e6e3);margin-bottom:.4rem;">' +
            'Restore from MitchSync?' +
          '</div>' +
          '<div style="font-size:.8rem;color:var(--t-fg2,#888);margin-bottom:.35rem;">Saved ' + savedDate + '</div>' +
          '<div style="font-size:.82rem;color:var(--t-fg2,#aaa);margin-bottom:1.6rem;">' + detail + '</div>' +
          '<div style="font-size:.75rem;color:var(--t-fg2,#666);margin-bottom:1.2rem;">' +
            'If you choose No, the MitchSync save will be overwritten with your current data.' +
          '</div>' +
          '<div style="display:flex;gap:10px;justify-content:center;">' +
            '<button id="_sync_no" style="flex:1;padding:.6rem 1rem;background:var(--t-bg3,#333);color:var(--t-fg2,#aaa);' +
              'border:1px solid var(--t-bd,#555);border-radius:9px;font-size:.88rem;font-weight:600;cursor:pointer;">' +
              'No, keep current' +
            '</button>' +
            '<button id="_sync_yes" style="flex:1;padding:.6rem 1rem;background:var(--t-gr,#81b64c);color:#fff;' +
              'border:none;border-radius:9px;font-size:.88rem;font-weight:700;cursor:pointer;">' +
              'Yes, restore' +
            '</button>' +
          '</div>' +
        '</div>' +
      '</div>';

    document.body.appendChild(overlay);

    document.getElementById('_sync_yes').addEventListener('click', function () {
      applySnap(snap);
      localStorage.setItem(SYNC_TS_KEY, serverTs);
      overlay.remove();
      location.reload();
    });

    document.getElementById('_sync_no').addEventListener('click', function () {
      overlay.remove();
      save();
    });
  }

  // Init
  setTimeout(checkRestore, 2000);
  setInterval(save, INTERVAL_MS);
  window.addEventListener('beforeunload', saveBeacon);

  window._mitchSync = { save: save, check: checkRestore };
})();
