/*
 * Encrypted-message backup and device transfer for the chat client (Cinny).
 *
 * Cinny's bundle exposes its MatrixClient as `window.__mxClient`. This script
 * adds, on top of that client:
 *
 *   1. Backup setup. A user with no secret storage picks a passphrase; we
 *      create the recovery key, cross-signing keys and server-side key backup,
 *      and make them save the recovery key.
 *   2. New-device transfer. A signed-in device that isn't verified sends a
 *      `com.mitch.e2ee.transfer_request` to the account's other devices. A
 *      device that is already set up shows who is asking; on approval it
 *      cross-signs the new device and sends the room keys (and the backup key)
 *      as Olm-encrypted to-device messages.
 *   3. Restore by passphrase or recovery key, for when no other device is
 *      around to approve.
 *
 * The pure helpers at the top are exported for tests (tests/e2ee-backup.test.js).
 * Behaviour and security notes: docs/matrix-e2ee.md.
 */
(function (root) {
  'use strict';

  const TYPE_REQUEST = 'com.mitch.e2ee.transfer_request';
  const TYPE_RESPONSE = 'com.mitch.e2ee.transfer_response';
  const TYPE_KEYS = 'com.mitch.e2ee.transfer_keys';
  const REQUEST_TTL_MS = 14 * 24 * 60 * 60 * 1000;
  const RESEND_AFTER_MS = 6 * 60 * 60 * 1000;
  const SETUP_SNOOZE_MS = 24 * 60 * 60 * 1000;
  const DISMISS_MS = 7 * 24 * 60 * 60 * 1000;
  const CHUNK_BYTES = 48 * 1024;
  const MIN_PASSPHRASE = 8;

  // ── Pure helpers ────────────────────────────────────────────────

  const B58 = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';

  function base58Decode(text) {
    let n = 0n;
    for (const ch of text) {
      const i = B58.indexOf(ch);
      if (i < 0) return null;
      n = n * 58n + BigInt(i);
    }
    const bytes = [];
    while (n > 0n) { bytes.unshift(Number(n & 0xffn)); n >>= 8n; }
    for (const ch of text) { if (ch === B58[0]) bytes.unshift(0); else break; }
    return Uint8Array.from(bytes);
  }

  function base58Encode(bytes) {
    let n = 0n;
    for (const b of bytes) n = (n << 8n) | BigInt(b);
    let out = '';
    while (n > 0n) { out = B58[Number(n % 58n)] + out; n /= 58n; }
    for (const b of bytes) { if (b === 0) out = B58[0] + out; else break; }
    return out;
  }

  /** Matrix recovery key ("EsTc ...") to its 32 key bytes, or null if invalid. */
  function decodeRecoveryKey(text) {
    const raw = base58Decode(String(text || '').replace(/\s+/g, ''));
    if (!raw || raw.length !== 35 || raw[0] !== 0x8b || raw[1] !== 0x01) return null;
    let parity = 0;
    for (const b of raw) parity ^= b;
    if (parity !== 0) return null;
    return raw.slice(2, 34);
  }

  function encodeRecoveryKey(key32) {
    const buf = new Uint8Array(35);
    buf[0] = 0x8b; buf[1] = 0x01;
    buf.set(key32, 2);
    let parity = 0;
    for (let i = 0; i < 34; i++) parity ^= buf[i];
    buf[34] = parity;
    return (base58Encode(buf).match(/.{1,4}/g) || []).join(' ');
  }

  /** PBKDF2-SHA512 passphrase derivation used by Matrix secret storage (m.pbkdf2). */
  async function derivePassphraseKey(passphrase, salt, iterations, bits) {
    const enc = new TextEncoder();
    const material = await crypto.subtle.importKey('raw', enc.encode(passphrase), 'PBKDF2', false, ['deriveBits']);
    const out = await crypto.subtle.deriveBits(
      { name: 'PBKDF2', salt: enc.encode(salt), iterations, hash: 'SHA-512' },
      material,
      bits || 256
    );
    return new Uint8Array(out);
  }

  function describeBrowser(ua) {
    ua = String(ua || '');
    let browser = 'Browser';
    if (/Edg\//.test(ua)) browser = 'Edge';
    else if (/OPR\/|Opera/.test(ua)) browser = 'Opera';
    else if (/Firefox\/|FxiOS/.test(ua)) browser = 'Firefox';
    else if (/Chrome\/|CriOS/.test(ua)) browser = 'Chrome';
    else if (/Safari\//.test(ua)) browser = 'Safari';
    let os = '';
    if (/Windows/.test(ua)) os = 'Windows';
    else if (/Android/.test(ua)) os = 'Android';
    else if (/iPhone|iPad|iPod/.test(ua)) os = 'iOS';
    else if (/Mac OS X|Macintosh/.test(ua)) os = 'macOS';
    else if (/CrOS/.test(ua)) os = 'ChromeOS';
    else if (/Linux/.test(ua)) os = 'Linux';
    return os ? `${browser} on ${os}` : browser;
  }

  function cleanText(value, max) {
    return String(value == null ? '' : value).replace(/[\u0000-\u001f\u007f]/g, ' ').trim().slice(0, max);
  }

  const DEVICE_ID_RE = /^[A-Za-z0-9_.~-]{1,64}$/;

  /**
   * Validates a transfer request's content (it is sent by another device on
   * the same account, but is still untrusted text). Returns a normalized
   * request or null.
   */
  function validateRequest(content, ownDeviceId, now) {
    if (!content || typeof content !== 'object') return null;
    const id = cleanText(content.request_id, 64);
    const deviceId = typeof content.device_id === 'string' ? content.device_id : '';
    if (!id || !DEVICE_ID_RE.test(deviceId) || deviceId === ownDeviceId) return null;
    const ts = Number(content.client_ts);
    if (!Number.isFinite(ts) || ts > now + 5 * 60 * 1000 || now - ts > REQUEST_TTL_MS) return null;
    return { request_id: id, device_id: deviceId, summary: cleanText(content.summary, 80) || 'Unknown browser', client_ts: ts };
  }

  /** Splits exported room-key sessions into batches whose JSON stays under maxBytes. */
  function chunkSessions(sessions, maxBytes) {
    const chunks = [];
    let current = [];
    let size = 0;
    for (const s of sessions) {
      const len = JSON.stringify(s).length + 1;
      if (current.length && size + len > maxBytes) { chunks.push(current); current = []; size = 0; }
      current.push(s);
      size += len;
    }
    if (current.length) chunks.push(current);
    return chunks;
  }

  function timeAgo(ts, now) {
    const s = Math.max(0, Math.round(((now || Date.now()) - ts) / 1000));
    if (s < 60) return 'just now';
    if (s < 3600) return `${Math.round(s / 60)} min ago`;
    if (s < 86400) return `${Math.round(s / 3600)} h ago`;
    return `${Math.round(s / 86400)} d ago`;
  }

  function bytesToBase64(bytes) {
    let bin = '';
    for (let i = 0; i < bytes.length; i += 0x8000) bin += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
    return btoa(bin);
  }

  function base64ToBytes(text) {
    const bin = atob(text);
    const out = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
    return out;
  }

  const helpers = {
    TYPE_REQUEST, TYPE_RESPONSE, TYPE_KEYS,
    base58Decode, base58Encode, decodeRecoveryKey, encodeRecoveryKey,
    describeBrowser, validateRequest, chunkSessions, timeAgo, cleanText,
    bytesToBase64, base64ToBytes,
  };
  if (typeof module !== 'undefined' && module.exports) { module.exports = helpers; return; }
  if (!root || !root.document) return;

  // ── Browser-only part ───────────────────────────────────────────

  const store = {
    get(key) { try { return localStorage.getItem(key); } catch (_) { return null; } },
    set(key, value) { try { localStorage.setItem(key, value); } catch (_) {} },
    del(key) { try { localStorage.removeItem(key); } catch (_) {} },
    getJson(key, fallback) { try { const v = JSON.parse(localStorage.getItem(key)); return v == null ? fallback : v; } catch (_) { return fallback; } },
    setJson(key, value) { this.set(key, JSON.stringify(value)); },
  };

  function el(tag, attrs, children) {
    const node = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs || {})) {
      if (k === 'class') node.className = v;
      else if (k === 'text') node.textContent = v;
      else if (k.startsWith('on')) node.addEventListener(k.slice(2), v);
      else if (v !== false && v != null) node.setAttribute(k, v === true ? '' : v);
    }
    for (const c of [].concat(children || [])) if (c != null) node.append(c.nodeType ? c : document.createTextNode(String(c)));
    return node;
  }

  function whenBody(fn) {
    if (document.body) fn();
    else document.addEventListener('DOMContentLoaded', fn, { once: true });
  }

  let stackEl = null;
  function stack() {
    if (!stackEl) {
      stackEl = el('div', { id: 'mitch-e2ee-stack', 'aria-live': 'polite' });
      whenBody(() => document.body.append(stackEl));
    }
    return stackEl;
  }

  /** A notification card in the top-right stack. Returns { el, set, close }. */
  function card(id) {
    let node = id && stack().querySelector(`[data-card="${id}"]`);
    if (!node) {
      node = el('div', { class: 'mitch-e2ee-card', role: 'status' });
      if (id) node.dataset.card = id;
      stack().append(node);
    }
    return {
      el: node,
      set(children) { node.replaceChildren(...[].concat(children).filter(Boolean)); },
      close() { node.remove(); },
    };
  }

  function button(label, onclick, kind) {
    return el('button', { type: 'button', class: `mitch-e2ee-btn${kind ? ' ' + kind : ''}`, onclick, text: label });
  }

  /** Opens the replace-backup flow for people who lost every way in. */
  function resetButton() {
    return button('Reset backup', () => openSetup({ reset: true }));
  }

  /** A centered modal. Returns { el, body, close }. */
  function modal(title, dismissible) {
    const body = el('div', { class: 'mitch-e2ee-modal-body' });
    const panel = el('div', { class: 'mitch-e2ee-modal', role: 'dialog', 'aria-modal': 'true', 'aria-label': title }, [
      el('h2', { text: title }),
      body,
    ]);
    const overlay = el('div', { class: 'mitch-e2ee-overlay' }, [panel]);
    const close = () => { overlay.remove(); document.removeEventListener('keydown', onKey); };
    const onKey = e => { if (e.key === 'Escape' && dismissible) close(); };
    document.addEventListener('keydown', onKey);
    whenBody(() => document.body.append(overlay));
    return { el: overlay, body, close };
  }

  function errorLine() {
    return el('p', { class: 'mitch-e2ee-error', role: 'alert' });
  }

  // ── Client access ───────────────────────────────────────────────

  let client = null;
  const heldKeys = new Map(); // secret-storage key id -> Uint8Array, filled when the user unlocks
  const k = name => `mitch_e2ee_${name}_${client ? client.getUserId() : ''}`;

  function ownIds() { return { userId: client.getUserId(), deviceId: client.getDeviceId() }; }
  function cryptoApi() { return client && client.getCrypto && client.getCrypto(); }

  /**
   * Cinny only answers secret-storage key requests from keys it cached in its
   * own UI flows. Consult the keys this script holds first, then fall back.
   */
  function installKeyCallback() {
    const cb = client.cryptoCallbacks;
    if (!cb || cb.__mitchWrapped) return;
    const original = cb.getSecretStorageKey;
    cb.getSecretStorageKey = async function (opts, name) {
      for (const id of Object.keys((opts && opts.keys) || {})) if (heldKeys.has(id)) return [id, heldKeys.get(id)];
      return original ? original.call(this, opts, name) : null;
    };
    cb.__mitchWrapped = true;
  }

  /** The server completes the upload's password check (see routes/matrix.rs), so no prompt. */
  async function authUploadDeviceSigningKeys(makeRequest) {
    await makeRequest(null);
  }

  // ── State ───────────────────────────────────────────────────────

  async function assess() {
    const api = cryptoApi();
    if (!api) return null;
    const { userId, deviceId } = ownIds();
    const secret = await api.getSecretStorageStatus();
    const verification = await api.getDeviceVerificationStatus(userId, deviceId);
    let backupVersion = await api.getActiveSessionBackupVersion();
    return {
      hasSecretStorage: !!(secret && secret.defaultKeyId),
      verified: !!(verification && verification.crossSigningVerified),
      backupVersion,
    };
  }

  async function otherDevices() {
    const api = cryptoApi();
    const { userId, deviceId } = ownIds();
    const map = await api.getUserDeviceInfo([userId], true);
    const devices = map.get(userId) || new Map();
    return Array.from(devices.keys()).filter(id => id !== deviceId);
  }

  // ── Banner (persistent status line on this device) ──────────────

  const banner = {
    /**
     * Shows a status card with an x. `state` names what it is about; closing
     * it hides that state on this device for a week. `slot` lets two cards
     * (status, passphrase) be on screen at once.
     */
    show(text, actions, state, slot) {
      const dismissKey = k('dismiss_' + state);
      if (Date.now() - (Number(store.get(dismissKey)) || 0) < DISMISS_MS) return;
      const c = card(slot || 'banner');
      const close = el('button', {
        type: 'button', class: 'mitch-e2ee-x', 'aria-label': 'Dismiss',
        onclick: () => { store.set(dismissKey, String(Date.now())); c.close(); },
        text: '\u00d7',
      });
      c.set([close, el('p', { text }), actions && actions.length ? el('div', { class: 'mitch-e2ee-actions' }, actions) : null]);
      c.el.dataset.state = state || '';
    },
    hide(slot) { const n = stack().querySelector(`[data-card="${slot || 'banner'}"]`); if (n) n.remove(); },
  };

  // ── Setup ───────────────────────────────────────────────────────

  /** `reset` replaces an existing backup whose recovery key was lost. Old backed-up history is not recoverable. */
  function openSetup(opts) {
    const reset = !!(opts && opts.reset);
    const m = modal(reset ? 'Start a new backup' : 'Back up your encrypted messages', true);
    const err = errorLine();
    const pass = el('input', { type: 'password', class: 'mitch-e2ee-input', autocomplete: 'new-password', 'aria-label': 'Passphrase', minlength: String(MIN_PASSPHRASE), required: true });
    const confirm = el('input', { type: 'password', class: 'mitch-e2ee-input', autocomplete: 'new-password', 'aria-label': 'Confirm passphrase', required: true });
    const submit = el('button', { type: 'submit', class: 'mitch-e2ee-btn primary', text: 'Create backup' });
    const form = el('form', { class: 'mitch-e2ee-form', onsubmit: async e => {
      e.preventDefault();
      err.textContent = '';
      if (pass.value.length < MIN_PASSPHRASE) { err.textContent = `Use at least ${MIN_PASSPHRASE} characters.`; return; }
      if (pass.value !== confirm.value) { err.textContent = 'The passphrases don\'t match.'; return; }
      submit.disabled = true;
      submit.textContent = 'Creating…';
      try {
        const encoded = await createBackup(pass.value, reset);
        showRecoveryKey(m, encoded);
      } catch (ex) {
        console.error('[e2ee] setup failed', ex);
        err.textContent = 'Couldn\'t create the backup. ' + (ex && ex.message ? ex.message : 'Try again.');
        submit.disabled = false;
        submit.textContent = 'Create backup';
      }
    } }, [
      el('p', { text: reset
        ? 'This replaces your current backup. Messages that are only in the old backup can\'t be restored, and your other devices will need to be verified again. Choose a new passphrase.'
        : 'Choose a passphrase. You\'ll use it, or a recovery key we\'ll show you next, to read your encrypted messages on a new device when no other device is signed in.' }),
      el('label', {}, ['Passphrase', pass]),
      el('label', {}, ['Confirm passphrase', confirm]),
      err,
      el('div', { class: 'mitch-e2ee-actions' }, [
        button('Not now', () => { store.set(k('setup_snooze'), String(Date.now())); m.close(); }),
        submit,
      ]),
    ]);
    m.body.append(form);
    pass.focus();
  }

  function showRecoveryKey(m, encoded) {
    const saved = el('input', { type: 'checkbox', id: 'mitch-e2ee-saved' });
    const done = el('button', { type: 'button', class: 'mitch-e2ee-btn primary', disabled: true, text: 'Done', onclick: () => { m.close(); banner.hide(); } });
    saved.addEventListener('change', () => { done.disabled = !saved.checked; });
    m.body.replaceChildren(
      el('p', { text: 'Save this recovery key somewhere safe, such as a password manager. If you forget your passphrase it is the only other way to read your encrypted messages on a new device.' }),
      el('code', { class: 'mitch-e2ee-key', text: encoded }),
      el('div', { class: 'mitch-e2ee-actions start' }, [
        button('Copy', async ev => { try { await navigator.clipboard.writeText(encoded); ev.target.textContent = 'Copied'; } catch (_) { ev.target.textContent = 'Copy failed'; } }),
        button('Download', () => {
          const url = URL.createObjectURL(new Blob([`mitch.pro chat recovery key\n\n${encoded}\n`], { type: 'text/plain' }));
          const a = el('a', { href: url, download: 'mitch-pro-recovery-key.txt' });
          document.body.append(a); a.click(); a.remove();
          setTimeout(() => URL.revokeObjectURL(url), 1000);
        }),
      ]),
      el('label', { class: 'mitch-e2ee-check', for: 'mitch-e2ee-saved' }, [saved, ' I saved my recovery key']),
      el('div', { class: 'mitch-e2ee-actions' }, [done])
    );
  }

  /** Creates secret storage, cross-signing keys and server-side backup. Returns the encoded recovery key. */
  async function createBackup(passphrase, reset) {
    const api = cryptoApi();
    const recovery = await api.createRecoveryKeyFromPassphrase(passphrase);
    if (!recovery || !recovery.encodedPrivateKey) throw new Error('Couldn\'t create a recovery key.');
    await api.bootstrapCrossSigning({ authUploadDeviceSigningKeys, setupNewCrossSigning: !!reset });
    await api.bootstrapSecretStorage({
      createSecretStorageKey: async () => recovery,
      setupNewSecretStorage: true,
      setupNewKeyBackup: true,
    });
    await api.checkKeyBackupAndEnable();
    store.del(k('setup_snooze'));
    banner.hide();
    return recovery.encodedPrivateKey;
  }

  // ── Add a passphrase to a backup that only has a recovery key ──

  /** Whether this device already holds every key needed to re-wrap secret storage. */
  async function hasKeysLocally() {
    const api = cryptoApi();
    const st = await api.getCrossSigningStatus();
    const c = st.privateKeysCachedLocally;
    let backupKey = null;
    try { backupKey = await api.getSessionBackupPrivateKey(); } catch (_) {}
    return !!(c.masterKey && c.selfSigningKey && c.userSigningKey && backupKey);
  }

  /** Re-encrypts secret storage under a key derived from a new passphrase. Returns the new recovery key. */
  async function addPassphrase(passphrase, currentKey) {
    const api = cryptoApi();
    if (currentKey) {
      const { keyId, key } = await secretKeyFromInput(currentKey.trim());
      heldKeys.set(keyId, key);
      await api.bootstrapCrossSigning({});
      try { await api.crossSignDevice(client.getDeviceId()); } catch (_) { /* already signed */ }
      await api.loadSessionBackupPrivateKeyFromSecretStorage();
    }
    const recovery = await api.createRecoveryKeyFromPassphrase(passphrase);
    await api.bootstrapSecretStorage({ createSecretStorageKey: async () => recovery, setupNewSecretStorage: true });
    await api.checkKeyBackupAndEnable();
    banner.hide('passphrase');
    return recovery.encodedPrivateKey;
  }

  async function openAddPassphrase() {
    const m = modal('Add a passphrase to your backup', true);
    const err = errorLine();
    const needsKey = !(await hasKeysLocally());
    const current = el('input', { type: 'password', class: 'mitch-e2ee-input', autocomplete: 'off', spellcheck: 'false', 'aria-label': 'Current recovery key', required: needsKey });
    const pass = el('input', { type: 'password', class: 'mitch-e2ee-input', autocomplete: 'new-password', 'aria-label': 'New passphrase', minlength: String(MIN_PASSPHRASE), required: true });
    const confirm = el('input', { type: 'password', class: 'mitch-e2ee-input', autocomplete: 'new-password', 'aria-label': 'Confirm passphrase', required: true });
    const submit = el('button', { type: 'submit', class: 'mitch-e2ee-btn primary', text: 'Add passphrase' });
    const reset = el('button', { type: 'button', class: 'mitch-e2ee-link', text: 'I don\'t have my recovery key', onclick: () => { m.close(); openSetup({ reset: true }); } });
    m.body.append(el('form', { class: 'mitch-e2ee-form', onsubmit: async e => {
      e.preventDefault();
      err.textContent = '';
      if (pass.value.length < MIN_PASSPHRASE) { err.textContent = `Use at least ${MIN_PASSPHRASE} characters.`; return; }
      if (pass.value !== confirm.value) { err.textContent = 'The passphrases don\'t match.'; return; }
      submit.disabled = true;
      submit.textContent = 'Working…';
      try {
        showRecoveryKey(m, await addPassphrase(pass.value, needsKey ? current.value : ''));
      } catch (ex) {
        console.error('[e2ee] add passphrase failed', ex);
        err.textContent = ex && ex.message ? ex.message : 'Couldn\'t add a passphrase.';
        submit.disabled = false;
        submit.textContent = 'Add passphrase';
      }
    } }, [
      el('p', { text: 'Your backup only has a recovery key. Add a passphrase so you can restore your messages without it. You\'ll get a new recovery key to save.' }),
      needsKey ? el('label', {}, ['Current recovery key', current]) : null,
      el('label', {}, ['New passphrase', pass]),
      el('label', {}, ['Confirm passphrase', confirm]),
      err,
      el('div', { class: 'mitch-e2ee-actions' }, [
        needsKey ? reset : null,
        button('Not now', () => { store.set(k('passphrase_snooze'), String(Date.now())); m.close(); }),
        submit,
      ]),
    ]));
    (needsKey ? current : pass).focus();
  }

  // ── Unlock with passphrase or recovery key ──────────────────────

  async function secretKeyFromInput(text) {
    const tuple = await client.secretStorage.getKey(null);
    if (!tuple) throw new Error('No backup found on your account.');
    const [keyId, info] = tuple;
    const candidates = [];
    const asRecovery = decodeRecoveryKey(text);
    if (asRecovery) candidates.push(asRecovery);
    if (info.passphrase && info.passphrase.salt && info.passphrase.iterations) {
      candidates.push(await derivePassphraseKey(text, info.passphrase.salt, info.passphrase.iterations, info.passphrase.bits));
    }
    for (const key of candidates) {
      if (await client.secretStorage.checkKey(key, info)) return { keyId, key };
    }
    throw new Error('That passphrase or recovery key is wrong.');
  }

  /** Verifies this device from secret storage, then restores the backed-up room keys. */
  async function restoreWithSecret(text, onProgress) {
    const api = cryptoApi();
    const { keyId, key } = await secretKeyFromInput(text.trim());
    // Held for this page load only so later key requests work; never stored.
    heldKeys.set(keyId, key);
    await api.bootstrapCrossSigning({});
    try { await api.crossSignDevice(client.getDeviceId()); } catch (_) { /* already signed */ }
    await api.loadSessionBackupPrivateKeyFromSecretStorage();
    await api.checkKeyBackupAndEnable();
    await api.restoreKeyBackup(onProgress ? { progressCallback: onProgress } : undefined);
  }

  function openUnlock(intro) {
    const m = modal('Restore your encrypted messages', true);
    const err = errorLine();
    const input = el('input', { type: 'password', class: 'mitch-e2ee-input', autocomplete: 'off', spellcheck: 'false', 'aria-label': 'Passphrase or recovery key', required: true });
    const submit = el('button', { type: 'submit', class: 'mitch-e2ee-btn primary', text: 'Restore' });
    m.body.append(el('form', { class: 'mitch-e2ee-form', onsubmit: async e => {
      e.preventDefault();
      err.textContent = '';
      submit.disabled = true;
      submit.textContent = 'Restoring…';
      try {
        await restoreWithSecret(input.value, p => { submit.textContent = `Restoring ${p.successes || 0} of ${p.total || '?'}…`; });
        m.close();
        banner.hide();
        flash('Messages restored.');
      } catch (ex) {
        err.textContent = ex && ex.message ? ex.message : 'Couldn\'t restore.';
        submit.disabled = false;
        submit.textContent = 'Restore';
      }
    } }, [
      el('p', { text: intro || 'Enter your backup passphrase or recovery key.' }),
      input,
      err,
      el('div', { class: 'mitch-e2ee-actions' }, [
        el('button', { type: 'button', class: 'mitch-e2ee-link', text: 'I don\'t have either', onclick: () => { m.close(); openSetup({ reset: true }); } }),
        button('Cancel', () => m.close()),
        submit,
      ]),
    ]));
    input.focus();
  }

  function flash(text) {
    const c = card('flash');
    c.set(el('p', { text }));
    setTimeout(() => c.close(), 6000);
  }

  // ── New device side ─────────────────────────────────────────────

  async function requestTransfer() {
    const { userId, deviceId } = ownIds();
    const last = store.getJson(k('outreq'), null);
    const now = Date.now();
    if (last && last.device_id === deviceId && now - last.sent < RESEND_AFTER_MS) return last.request_id;
    const request_id = Array.from(crypto.getRandomValues(new Uint8Array(12)), b => b.toString(16).padStart(2, '0')).join('');
    store.setJson(k('outreq'), { request_id, device_id: deviceId, sent: now });
    const content = { request_id, device_id: deviceId, client_ts: now, summary: describeBrowser(navigator.userAgent) };
    await client.sendToDevice(TYPE_REQUEST, new Map([[userId, new Map([['*', content]])]]));
    return request_id;
  }

  const receiving = { total: 0, done: 0, chunks: new Set(), chunkCount: 0 };

  async function handleKeys(ev) {
    const content = ev.getContent();
    const outreq = store.getJson(k('outreq'), null);
    if (!outreq || !content || content.request_id !== outreq.request_id) return;
    const api = cryptoApi();
    const sessions = Array.isArray(content.sessions) ? content.sessions : [];
    const index = Number(content.chunk_index);
    const count = Number(content.chunk_count);
    if (!Number.isInteger(index) || !Number.isInteger(count) || count < 1 || index < 0 || index >= count || receiving.chunks.has(index)) return;

    receiving.chunks.add(index);
    receiving.chunkCount = count;
    receiving.total = Number(content.total_sessions) || receiving.total;
    const c = card('banner');
    try {
      if (content.backup && content.backup.key && content.backup.version) {
        await api.storeSessionBackupPrivateKey(base64ToBytes(content.backup.key), String(content.backup.version));
        await api.checkKeyBackupAndEnable();
      }
      if (sessions.length) await api.importRoomKeysAsJson(JSON.stringify(sessions));
      receiving.done += sessions.length;
    } catch (e) {
      console.error('[e2ee] import failed', e);
      receiving.chunks.delete(index);
      c.set(el('p', { text: 'Couldn\'t import some messages from your other device.' }));
      return;
    }
    if (receiving.chunks.size < receiving.chunkCount) {
      c.set(el('p', { text: `Restoring messages from your other device… ${receiving.chunks.size} of ${receiving.chunkCount}` }));
      return;
    }
    store.del(k('outreq'));
    c.close();
    flash(receiving.done ? `Restored ${receiving.done} message keys from your other device.` : 'This device is now verified.');
    try { await api.restoreKeyBackup(); } catch (_) { /* no backup key, or nothing more to fetch */ }
  }

  function handleResponse(ev) {
    const content = ev.getContent();
    const outreq = store.getJson(k('outreq'), null);
    if (!outreq || !content || content.request_id !== outreq.request_id) return;
    if (content.status === 'declined') {
      store.del(k('outreq'));
      banner.show('Your other device declined the transfer.', [resetButton(), button('Use passphrase or recovery key', () => openUnlock())], 'declined');
    }
  }

  async function runNewDevice() {
    const others = await otherDevices();
    if (!others.length) {
      banner.show('Enter your passphrase or recovery key to read your encrypted messages on this device.', [resetButton(), button('Enter it', () => openUnlock())], 'no-other');
      if (!store.get(k('unlock_prompted_' + ownIds().deviceId))) {
        store.set(k('unlock_prompted_' + ownIds().deviceId), '1');
        openUnlock('Enter your backup passphrase or recovery key to read your encrypted messages on this device.');
      }
      return;
    }
    await requestTransfer();
    banner.show('Waiting for your other device to approve this one.', [resetButton(), button('Use passphrase or recovery key', () => openUnlock())], 'waiting');
  }

  // ── Existing device side ────────────────────────────────────────

  function pendingList() { return store.getJson(k('pending'), []); }
  function savePending(list) { store.setJson(k('pending'), list.slice(-20)); }
  function handledList() { return store.getJson(k('handled'), []); }
  function markHandled(id) { store.setJson(k('handled'), handledList().concat(id).slice(-60)); }

  function onRequestEvent(ev) {
    const { userId, deviceId } = ownIds();
    if (ev.getSender() !== userId) return;
    const req = validateRequest(ev.getContent(), deviceId, Date.now());
    if (!req || handledList().includes(req.request_id)) return;
    // One pending card per requesting device; the newest request wins.
    savePending(pendingList().filter(p => p.device_id !== req.device_id && p.request_id !== req.request_id).concat(req));
    showRequestCard(req);
  }

  async function showRequestCard(req) {
    const c = card('req-' + req.request_id);
    const lines = [el('p', { class: 'mitch-e2ee-title', text: 'Transfer messages to a new device?' })];
    let info = null;
    try { info = await client.getDevice(req.device_id); } catch (_) { /* device info is optional */ }
    lines.push(el('p', { text: `${req.summary}, requested ${timeAgo(req.client_ts)}.` }));
    const details = [];
    if (info && info.display_name) details.push(`Name: ${cleanText(info.display_name, 80)}`);
    if (info && info.last_seen_ip) details.push(`IP: ${cleanText(info.last_seen_ip, 64)}`);
    if (details.length) lines.push(el('p', { class: 'mitch-e2ee-detail', text: details.join(' · ') }));
    lines.push(el('p', { class: 'mitch-e2ee-detail', text: 'The browser name comes from the new device. If this wasn\'t you, decline and remove the device in Settings → Devices.' }));
    const approve = button('Transfer messages', () => approveTransfer(req, c), 'primary');
    const decline = button('Decline', () => declineTransfer(req, c));
    lines.push(el('div', { class: 'mitch-e2ee-actions' }, [decline, approve]));
    c.set(lines);
  }

  function finishRequest(req) {
    markHandled(req.request_id);
    savePending(pendingList().filter(p => p.request_id !== req.request_id));
  }

  async function declineTransfer(req, c) {
    finishRequest(req);
    c.close();
    try {
      await client.sendToDevice(TYPE_RESPONSE, new Map([[client.getUserId(), new Map([[req.device_id, { request_id: req.request_id, status: 'declined' }]])]]));
    } catch (_) { /* best effort */ }
  }

  async function approveTransfer(req, c) {
    const api = cryptoApi();
    const { userId } = ownIds();
    c.set(el('p', { text: 'Preparing transfer…' }));
    try {
      const devices = await api.getUserDeviceInfo([userId], true);
      if (!(devices.get(userId) || new Map()).has(req.device_id)) throw new Error('That device is no longer signed in.');

      let verified = false;
      try { await api.crossSignDevice(req.device_id); verified = true; } catch (e) { console.warn('[e2ee] could not cross-sign new device', e); }

      const exported = JSON.parse(await api.exportRoomKeysAsJson());
      const chunks = chunkSessions(exported, CHUNK_BYTES);
      let backup = null;
      try {
        const key = await api.getSessionBackupPrivateKey();
        const version = await api.getActiveSessionBackupVersion();
        if (key && version) backup = { version, key: bytesToBase64(key) };
      } catch (_) { /* no backup key on this device */ }

      const target = [{ userId, deviceId: req.device_id }];
      const total = chunks.length || 1;
      for (let i = 0; i < total; i++) {
        c.set(el('p', { text: `Sending messages… ${i + 1} of ${total}` }));
        await client.encryptAndSendToDevice(TYPE_KEYS, target, {
          request_id: req.request_id,
          chunk_index: i,
          chunk_count: total,
          total_sessions: exported.length,
          sessions: chunks[i] || [],
          backup: i === 0 ? backup : undefined,
        });
      }
      finishRequest(req);
      c.set([el('p', { text: verified ? 'Transferred. The new device is verified.' : 'Transferred. The new device is not verified yet.' })]);
      setTimeout(() => c.close(), 6000);
    } catch (e) {
      console.error('[e2ee] transfer failed', e);
      c.set([
        el('p', { text: 'Couldn\'t transfer messages. ' + (e && e.message ? e.message : '') }),
        el('div', { class: 'mitch-e2ee-actions' }, [button('Close', () => c.close())]),
      ]);
    }
  }

  // ── Startup ─────────────────────────────────────────────────────

  /** A backup made without a passphrase (e.g. by Cinny's own setup) gets a prompt to add one. */
  async function checkPassphrase() {
    const tuple = await client.secretStorage.getKey(null);
    if (!tuple || tuple[1].passphrase) return;
    banner.show('Your backup has no passphrase.', [button('Add one', () => openAddPassphrase())], 'no-passphrase', 'passphrase');
    const snoozed = Number(store.get(k('passphrase_snooze'))) || 0;
    if (Date.now() - snoozed > SETUP_SNOOZE_MS) openAddPassphrase();
  }

  async function run() {
    try {
      const state = await assess();
      if (!state) return;
      if (!state.hasSecretStorage) {
        const snoozed = Number(store.get(k('setup_snooze'))) || 0;
        banner.show('Encrypted message backup is off.', [button('Set up', () => openSetup())], 'setup');
        if (Date.now() - snoozed > SETUP_SNOOZE_MS) openSetup();
        return;
      }
      await checkPassphrase();
      if (!state.verified) { await runNewDevice(); return; }
      store.del(k('outreq'));
      if (!state.backupVersion) {
        try { await cryptoApi().checkKeyBackupAndEnable(); } catch (_) {}
        if (!(await cryptoApi().getActiveSessionBackupVersion())) {
          banner.show('Message backup is not running on this device.', [button('Turn on', () => openUnlock('Enter your passphrase or recovery key to turn message backup on.'))], 'backup-off');
        }
      }
    } catch (e) {
      console.error('[e2ee] startup check failed', e);
    }
  }

  function attach(c) {
    client = c;
    installKeyCallback();
    c.on('toDeviceEvent', ev => {
      try {
        const type = ev.getType();
        if (type === TYPE_REQUEST) onRequestEvent(ev);
        else if (type === TYPE_RESPONSE) handleResponse(ev);
        else if (type === TYPE_KEYS && ev.getSender() === c.getUserId() && ev.isEncrypted()) handleKeys(ev);
      } catch (e) { console.error('[e2ee] to-device handler failed', e); }
    });
    const start = () => {
      for (const req of pendingList()) {
        const fresh = validateRequest(req, c.getDeviceId(), Date.now());
        if (fresh && !handledList().includes(fresh.request_id)) showRequestCard(fresh);
      }
      run();
    };
    const state = c.getSyncState && c.getSyncState();
    if (state === 'PREPARED' || state === 'SYNCING') start();
    else {
      const onSync = s => { if (s === 'PREPARED' || s === 'SYNCING') { c.removeListener('sync', onSync); start(); } };
      c.on('sync', onSync);
    }
  }

  // The bundle assigns the client right before it starts syncing; hook the
  // assignment so no to-device event from the first sync is missed.
  let existing = root.__mxClient;
  Object.defineProperty(root, '__mxClient', {
    configurable: true,
    get() { return existing; },
    set(v) { existing = v; if (v && v !== client) { try { attach(v); } catch (e) { console.error('[e2ee] attach failed', e); } } },
  });
  if (existing) attach(existing);

  root.mitchE2ee = { openSetup, openUnlock, openAddPassphrase };
})(typeof window !== 'undefined' ? window : globalThis);
