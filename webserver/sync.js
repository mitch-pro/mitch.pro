(function () {
  'use strict';

  var SYNC_TS_KEY  = '_sync_ts';
  var INTERVAL_MS  = 5 * 60 * 1000;
  var RESTORED_KEY = '_just_restored';
  var IDB_HASH_KEY = '_sync_idb_hash';   // hash of the last IndexedDB upload; device-local

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
      if (k === SYNC_TS_KEY || k === RESTORED_KEY || k === IDB_HASH_KEY || k.startsWith('_panic') || k.startsWith('_cloak') || isSensitiveKey(k)) continue;
      
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

  // True while a newer server copy is waiting on the user's answer; background
  // saves pause so an older device can't overwrite it before they choose.
  var pendingRestore = false;

  function prefSync() {
    try { return JSON.parse(localStorage.getItem('_prefSync') || '{}') || {}; } catch (e) { return {}; }
  }

  // IndexedDB (where many games keep their progress) lives in /idb-sync.js,
  // loaded on first use.
  var idbLoader = null;
  function loadIdb() {
    if (window.MitchIDB) return Promise.resolve(window.MitchIDB);
    if (!idbLoader) idbLoader = new Promise(function (resolve) {
      var el = document.createElement('script');
      el.src = '/idb-sync.js';
      el.onload = function () { resolve(window.MitchIDB || null); };
      el.onerror = function () { resolve(null); };
      document.head.appendChild(el);
    });
    return idbLoader;
  }

  // Uploads this browser's IndexedDB databases when they changed since the last
  // upload. Merged per database over the server copy, so a device that has only
  // played one game doesn't erase the others; never uploads an empty set.
  function saveIdb() {
    if (pendingRestore || prefSync().enabled === false) return Promise.resolve();
    return loadIdb().then(function (I) {
      if (!I || !I.supported) return;
      return I.dump().then(function (dbs) {
        if (!Object.keys(dbs).length) return;
        var h = I.hash(dbs);
        if (h === localStorage.getItem(IDB_HASH_KEY)) return;
        return fetch('/api/userdata', { credentials: 'include' })
          .then(function (r) { return r.ok ? r.json() : null; })
          .then(function (server) {
            if (!server) return;
            var merged = Object.assign({}, server._idb || {}, dbs);
            return fetch('/api/userdata', {
              method: 'POST', credentials: 'include', headers: { 'Content-Type': 'application/json' },
              body: JSON.stringify({ _idb: merged, _idb_ts: Date.now() }),
            }).then(function (r) { if (r.ok) localStorage.setItem(IDB_HASH_KEY, h); });
          });
      });
    }).catch(function () {});
  }

  function save() {
    if (pendingRestore || prefSync().enabled === false) return;

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
    saveIdb();
  }

  function saveBeacon() {
    if (pendingRestore || prefSync().enabled === false) return;

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
    if (_restoreChecked || pendingRestore || prefSync().enabled === false || prefSync().restoreNotice === false) return;
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
          pendingRestore = true;
          showRestorePrompt(data._snapshot, serverTs, data._idb);
        }
      })
      .catch(function () {});
  }

  // Sync never restores on its own. This corner notice only says a newer save
  // exists; restoring is the user's call (phone and computer saves can differ).
  function showRestorePrompt(snap, serverTs, idb) {
    var savedDate = new Date(serverTs).toLocaleString('en-US', {
      month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit', hour12: true,
    });
    var box = document.createElement('div');
    box.id = '_sync_restore_notice';
    box.setAttribute('role', 'status');
    box.style.cssText = 'position:fixed;right:16px;bottom:16px;z-index:999998;max-width:340px;width:calc(100% - 32px);' +
      'box-sizing:border-box;padding:14px 16px;background:var(--t-bg2,#1e1e1e);color:var(--t-fg,#e8e6e3);' +
      'border:1px solid var(--t-bd,#444);border-radius:12px;box-shadow:0 10px 30px rgba(0,0,0,.35);' +
      'font:14px/1.4 "DM Sans",system-ui,sans-serif;';
    var title = document.createElement('div');
    title.style.cssText = 'font-weight:700;margin-bottom:2px;';
    title.textContent = 'A newer save is on your account';
    var sub = document.createElement('div');
    sub.style.cssText = 'font-size:12px;color:var(--t-fg2,#999);margin-bottom:10px;';
    sub.textContent = 'Saved ' + savedDate + '. Auto-sync is paused until you choose.';
    var row = document.createElement('div');
    row.style.cssText = 'display:flex;flex-wrap:wrap;gap:6px;';
    var btnCss = 'padding:6px 10px;border-radius:8px;border:1px solid var(--t-bd,#555);background:var(--t-bg3,#333);' +
      'color:var(--t-fg,#e8e6e3);font:inherit;font-size:13px;font-weight:600;cursor:pointer;';
    function btn(label, fn, primary) {
      var b = document.createElement('button');
      b.type = 'button';
      b.textContent = label;
      b.style.cssText = btnCss + (primary ? 'background:var(--t-ac,#4f8cff);border-color:var(--t-ac,#4f8cff);color:var(--t-ac-ink,#fff);' : '');
      b.addEventListener('click', fn);
      row.appendChild(b);
    }
    function close() { pendingRestore = false; box.remove(); }
    btn('Restore', function () {
      applySnap(snap);
      localStorage.setItem(SYNC_TS_KEY, serverTs);
      var done = function () { close(); location.reload(); };
      if (!idb || !Object.keys(idb).length) return done();
      loadIdb().then(function (I) { return I && I.restore(idb); }).then(done, done);
    }, true);
    btn('Keep this device', function () { close(); save(); });
    btn("Don't show again", function () {
      var p = prefSync();
      p.restoreNotice = false;
      localStorage.setItem('_prefSync', JSON.stringify(p));
      close();
      save();
    });
    box.appendChild(title);
    box.appendChild(sub);
    box.appendChild(row);
    var a = document.createElement('a');
    a.href = '/saves/';
    a.textContent = 'Manage saves';
    a.style.cssText = 'display:inline-block;margin-top:8px;font-size:12px;color:var(--t-ac,#4f8cff);';
    box.appendChild(a);
    document.body.appendChild(box);
  }

  // Init
  setTimeout(checkRestore, 2000);
  setInterval(save, INTERVAL_MS);
  // Game progress is written while playing on pages that don't run sync.js, so
  // pick up changes shortly after any sync page opens and when it's hidden.
  setTimeout(saveIdb, 8000);
  document.addEventListener('visibilitychange', function () { if (document.visibilityState === 'hidden') saveIdb(); });
  window.addEventListener('beforeunload', saveBeacon);

  window._mitchSync = { save: save, saveIdb: saveIdb, check: checkRestore };
})();
