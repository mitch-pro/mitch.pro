/*
 * IndexedDB snapshots for MitchSync and /saves/.
 *
 * Many browser games (Emscripten/Unity builds like Drive Mad) keep their
 * progress in IndexedDB, not localStorage, so a localStorage-only snapshot
 * misses exactly the data people care about. This dumps every IndexedDB
 * database of this origin to a JSON-safe object and writes one back.
 *
 *   MitchIDB.dump()            -> { <dbName>: { version, stores: [...] } }
 *   MitchIDB.restore(dbs, names?) -> { restored: [...], failed: [{name, error}] }
 *   MitchIDB.hash(dbs)         -> short string, to skip uploads when unchanged
 *
 * Skipped on purpose: databases whose names look sensitive (Matrix/Cinny
 * sessions and encryption keys, the site's E2E keys; mirrors
 * is_sensitive_idb_name in routes/members.rs), service-worker caches, and any
 * database holding values that can't be stored as JSON (Blobs/Files), since
 * restoring a partial copy would be worse than none.
 */
(function () {
  'use strict';

  var MAX_BYTES = 150 * 1024 * 1024;   // per-database cap on the encoded size
  var BYTE_KINDS = ['Uint8Array', 'Int8Array', 'Uint8ClampedArray', 'Int16Array', 'Uint16Array', 'Int32Array', 'Uint32Array',
    'Float32Array', 'Float64Array', 'BigInt64Array', 'BigUint64Array', 'DataView', 'ArrayBuffer'];

  function isSensitiveName(name) {
    var k = String(name).toLowerCase();
    return k.indexOf('cinny') !== -1 || k.indexOf('mx_') === 0 || k.indexOf('matrix') !== -1 || k.indexOf('e2e') !== -1 ||
      k.indexOf('passphrase') !== -1 || k.indexOf('access_token') !== -1 || k.indexOf('privatekey') !== -1 ||
      k.indexOf('private_key') !== -1 || k.indexOf('olm') !== -1 || k.indexOf('crypto') !== -1;
  }
  function skipName(name) {
    var k = String(name).toLowerCase();
    return isSensitiveName(name) || k.indexOf('workbox') === 0 || k.indexOf('firebase') === 0 || k.indexOf('sw-') === 0;
  }

  function toB64(bytes) {
    var s = '';
    for (var i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
    return btoa(s);
  }
  function fromB64(b64) {
    var s = atob(b64), out = new Uint8Array(s.length);
    for (var i = 0; i < s.length; i++) out[i] = s.charCodeAt(i);
    return out;
  }

  function Unsupported() {}

  // Structured-clone values -> JSON. Plain objects whose own keys include "$t"
  // are wrapped so they can't be mistaken for a tagged value.
  function encode(v, budget) {
    if (v === null || typeof v === 'boolean' || typeof v === 'string') { budget.n += 8 + (typeof v === 'string' ? v.length : 0); return v; }
    if (v === undefined) return { $t: 'undef' };
    if (typeof v === 'number') {
      budget.n += 8;
      return isFinite(v) ? v : { $t: 'num', v: String(v) };
    }
    if (typeof v === 'bigint') return { $t: 'bigint', v: v.toString() };
    if (v instanceof Date) return { $t: 'date', v: v.getTime() };
    if (v instanceof ArrayBuffer || ArrayBuffer.isView(v)) {
      var kind = v instanceof ArrayBuffer ? 'ArrayBuffer' : v.constructor.name;
      if (BYTE_KINDS.indexOf(kind) < 0) throw new Unsupported();
      var bytes = v instanceof ArrayBuffer ? new Uint8Array(v) : new Uint8Array(v.buffer, v.byteOffset, v.byteLength);
      budget.n += Math.ceil(bytes.length * 4 / 3);
      if (budget.n > MAX_BYTES) throw new Unsupported();
      return { $t: 'bytes', k: kind, b: toB64(bytes) };
    }
    if (Array.isArray(v)) return v.map(function (x) { return encode(x, budget); });
    if (typeof v === 'object') {
      var proto = Object.getPrototypeOf(v);
      if (proto !== Object.prototype && proto !== null) throw new Unsupported();   // Blob, File, Map, ...
      var o = {};
      Object.keys(v).forEach(function (k) { o[k] = encode(v[k], budget); });
      return Object.prototype.hasOwnProperty.call(o, '$t') ? { $t: 'obj', v: o } : o;
    }
    throw new Unsupported();
  }

  function decode(v) {
    if (v === null || typeof v !== 'object') return v;
    if (Array.isArray(v)) return v.map(decode);
    switch (v.$t) {
      case 'undef': return undefined;
      case 'num': return Number(v.v);
      case 'bigint': return BigInt(v.v);
      case 'date': return new Date(v.v);
      case 'bytes': {
        var bytes = fromB64(v.b);
        if (v.k === 'ArrayBuffer') return bytes.buffer;
        if (BYTE_KINDS.indexOf(v.k) < 0 || typeof window[v.k] !== 'function') return bytes;
        return new window[v.k](bytes.buffer);
      }
      case 'obj': v = v.v; break;
    }
    var o = {};
    Object.keys(v).forEach(function (k) { o[k] = decode(v[k]); });
    return o;
  }

  function req(r) {
    return new Promise(function (resolve, reject) {
      r.onsuccess = function () { resolve(r.result); };
      r.onerror = function () { reject(r.error); };
    });
  }
  function txDone(tx) {
    return new Promise(function (resolve, reject) {
      tx.oncomplete = function () { resolve(); };
      tx.onerror = function () { reject(tx.error); };
      tx.onabort = function () { reject(tx.error || new Error('aborted')); };
    });
  }
  function openDb(name, version, upgrade) {
    return new Promise(function (resolve, reject) {
      var r = version ? indexedDB.open(name, version) : indexedDB.open(name);
      if (upgrade) r.onupgradeneeded = function () { upgrade(r.result); };
      r.onsuccess = function () { resolve(r.result); };
      r.onerror = function () { reject(r.error); };
      r.onblocked = function () { reject(new Error('database is in use in another tab')); };
    });
  }
  function deleteDb(name) {
    return new Promise(function (resolve, reject) {
      var r = indexedDB.deleteDatabase(name);
      var t = setTimeout(function () { reject(new Error('database is in use in another tab; close open games and retry')); }, 4000);
      r.onsuccess = function () { clearTimeout(t); resolve(); };
      r.onerror = function () { clearTimeout(t); reject(r.error); };
      r.onblocked = function () { /* wait for the timeout */ };
    });
  }
  async function existingNames() {
    if (!indexedDB || !indexedDB.databases) return [];
    return (await indexedDB.databases()).map(function (d) { return d.name; });
  }

  async function dumpOne(name) {
    var db = await openDb(name);
    try {
      var budget = { n: 0 };
      var stores = [];
      var names = Array.prototype.slice.call(db.objectStoreNames);
      for (var i = 0; i < names.length; i++) {
        var tx = db.transaction(names[i], 'readonly');
        var store = tx.objectStore(names[i]);
        var meta = {
          name: names[i], keyPath: store.keyPath, autoIncrement: store.autoIncrement,
          indexes: Array.prototype.slice.call(store.indexNames).map(function (n) {
            var ix = store.index(n);
            return { name: n, keyPath: ix.keyPath, unique: ix.unique, multiEntry: ix.multiEntry };
          }),
          records: []
        };
        await new Promise(function (resolve, reject) {
          var c = store.openCursor();
          c.onerror = function () { reject(c.error); };
          c.onsuccess = function () {
            var cur = c.result;
            if (!cur) return resolve();
            try { meta.records.push([encode(cur.primaryKey, budget), encode(cur.value, budget)]); }
            catch (e) { return reject(e); }
            cur.continue();
          };
        });
        stores.push(meta);
      }
      return { version: db.version, stores: stores };
    } finally { db.close(); }
  }

  async function dump() {
    var out = {};
    var names = await existingNames();
    for (var i = 0; i < names.length; i++) {
      if (!names[i] || skipName(names[i])) continue;
      try { out[names[i]] = await dumpOne(names[i]); }
      catch (e) { /* unsupported or unreadable: leave it out */ }
    }
    return out;
  }

  function schemaMatches(db, snap) {
    return snap.stores.every(function (s) {
      if (!db.objectStoreNames.contains(s.name)) return false;
      var tx = db.transaction(s.name, 'readonly');
      var store = tx.objectStore(s.name);
      return JSON.stringify(store.keyPath) === JSON.stringify(s.keyPath) && store.autoIncrement === s.autoIncrement;
    });
  }

  function createSchema(db, snap) {
    snap.stores.forEach(function (s) {
      var opts = { autoIncrement: !!s.autoIncrement };
      if (s.keyPath !== null && s.keyPath !== undefined) opts.keyPath = s.keyPath;
      var store = db.createObjectStore(s.name, opts);
      (s.indexes || []).forEach(function (ix) {
        store.createIndex(ix.name, ix.keyPath, { unique: !!ix.unique, multiEntry: !!ix.multiEntry });
      });
    });
  }

  async function restoreOne(name, snap) {
    var present = (await existingNames()).indexOf(name) >= 0;
    var db = null;
    if (present) {
      db = await openDb(name);
      if (!schemaMatches(db, snap)) { db.close(); db = null; await deleteDb(name); }
    }
    // Recreate at the snapshot's own version: games open with a fixed version
    // and fail if the stored one is higher.
    if (!db) db = await openDb(name, snap.version, function (d) { createSchema(d, snap); });
    try {
      var names = snap.stores.map(function (s) { return s.name; });
      var tx = db.transaction(names, 'readwrite');
      snap.stores.forEach(function (s) {
        var store = tx.objectStore(s.name);
        store.clear();
        var inline = store.keyPath !== null;
        s.records.forEach(function (rec) {
          if (inline) store.put(decode(rec[1])); else store.put(decode(rec[1]), decode(rec[0]));
        });
      });
      await txDone(tx);
    } finally { db.close(); }
  }

  // Replaces the named databases (default: all in `dbs`) on this browser.
  async function restore(dbs, only) {
    var res = { restored: [], failed: [] };
    var names = Object.keys(dbs || {}).filter(function (n) { return !only || only.indexOf(n) >= 0; });
    for (var i = 0; i < names.length; i++) {
      if (skipName(names[i])) continue;
      try { await restoreOne(names[i], dbs[names[i]]); res.restored.push(names[i]); }
      catch (e) { res.failed.push({ name: names[i], error: (e && e.message) || String(e) }); }
    }
    return res;
  }

  // FNV-1a over the JSON text: enough to tell "changed since last upload".
  function hash(dbs) {
    var s = JSON.stringify(dbs), h = 0x811c9dc5;
    for (var i = 0; i < s.length; i++) { h ^= s.charCodeAt(i); h = Math.imul(h, 0x01000193) >>> 0; }
    return h.toString(16) + ':' + s.length;
  }

  window.MitchIDB = { dump: dump, restore: restore, hash: hash, isSensitiveName: isSensitiveName, supported: !!(window.indexedDB && indexedDB.databases) };
})();
